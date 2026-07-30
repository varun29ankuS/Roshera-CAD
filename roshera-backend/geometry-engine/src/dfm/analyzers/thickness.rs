//! `pair_thickness` — wall thickness between two OPPOSING B-Rep faces that
//! provably bound the same slab of material (spec §3.1 / §3.2 `fdm.min_wall`).
//!
//! ## The hard part is pairing, not distance
//!
//! Measuring a distance between two faces is trivial; measuring the
//! distance between two faces that do NOT actually bound a wall is the
//! exact "kernel can lie" defect this analyzer exists to prevent — the
//! same silent-wrong-answer class S2's draft-angle V-shape bug belongs to.
//! A "wall pair" is defined precisely below for the two surface-kind
//! combinations this slice supports; every other combination is an honest
//! refusal, never a nearest-face guess.
//!
//! ### Parallel plane pair
//!
//! Two PLANE faces `(A, B)` form a wall pair iff:
//!
//! 1. **Opposing outward normals through material**: `n_A · n_B ≈ -1`,
//!    where `n = face.orientation.sign() * plane.normal` is the face's
//!    OUTWARD normal (the same convention `orientation.rs` derives from
//!    `FaceOrientation::sign()` — see that module's docs). Two faces
//!    facing the SAME way (e.g. two top faces at different heights, both
//!    `n = +Z`) are never a pair: their material does not sit between
//!    them, it sits on the SAME side of both.
//! 2. **Positive separation**: `d = (originB − originA) · n_A > 0` (up to
//!    a tight epsilon) — coincident planes are not a wall.
//! 3. **Overlapping trimmed regions, projected along the shared normal**
//!    — see "The conservative-refusal line" below for exactly how much of
//!    this is proven exactly versus refused.
//!
//! `thickness = d` (exact, since it is a plain linear separation between
//! two planes with a shared normal direction — no approximation needed
//! once (1)-(3) hold).
//!
//! ### Coaxial cylinder pair (bore wall)
//!
//! Two CYLINDER faces `(A, B)` form a wall pair iff:
//!
//! 1. **Same axis LINE**: their axis directions are parallel
//!    (`|axis_A · axis_B| ≈ 1`) AND colinear — the perpendicular offset
//!    between the two axis lines (`(originB − originA)` minus its
//!    component along `axis_A`) is ≈ 0. This is a floating-point
//!    ROUND-OFF tolerance (`CYLINDER_AXIS_COLINEAR_TOL`), not a physical
//!    fit tolerance: two cylinders sharing a nominal axis by construction
//!    are expected to be numerically identical up to a few ULPs, and this
//!    analyzer does not attempt to reason about a deliberately-eccentric
//!    bore (that is a different, harder question this slice does not
//!    answer).
//! 2. **Opposing radial orientation**: `face.orientation.sign()` differs
//!    in sign between the two faces. A cylinder's carrier `normal` (from
//!    `Cylinder::evaluate_full`) is always the geometric radial direction
//!    pointing AWAY from the axis; `face.orientation.sign() == +1`
//!    (`Forward`) therefore means the face's OUTWARD normal points away
//!    from the axis — material is INSIDE (a shaft/boss's outer surface).
//!    `sign == -1` (`Backward`) means the outward normal points TOWARD the
//!    axis — material is OUTSIDE (a bore/hole's wall, void toward the
//!    axis). A wall pair needs one of each: `sign_A * sign_B < 0`. Two
//!    faces with the SAME sign (both outer, or both inner) never bound a
//!    single annulus of material between them.
//! 3. **Distinct radii** (`|r_A − r_B| > 0`, an equal-radius pair is
//!    degenerate — the same surface, not a wall) and **overlapping
//!    TRIMMED axial extents** — see below.
//!
//! `thickness = |r_A − r_B|` exactly.
//!
//! ## The conservative-refusal line (read this before touching the overlap
//! logic)
//!
//! Exact overlap of two arbitrary trimmed loops is hard; this analyzer
//! computes it EXACTLY only for the shapes it can, and REFUSES — never
//! guesses — everywhere else. The line sits at exactly these points:
//!
//! - **Planar loops**: [`rectangle_from_outer_loop`] classifies a face's
//!   outer loop as an exact axis-aligned-in-its-own-plane RECTANGLE only
//!   when it has exactly 4 straight (`Line`) edges, no inner loops, and
//!   consecutive edges meet at a right angle (which — combined with
//!   closure — forces a true rectangle, not just any quadrilateral).
//!   When BOTH faces of a candidate pair classify this way AND their
//!   in-plane axes align (parallel edge directions, checked by
//!   [`rectangles_axis_aligned`]), the overlap of the two projected
//!   rectangles is computed EXACTLY (`try_plane_pair`'s first branch) —
//!   this is the only path that can produce a plane [`FacePair`].
//!   Otherwise (non-rectangular loop, an inner loop, a curved boundary
//!   edge, or two rectangles whose edges are NOT parallel to a shared
//!   frame), this analyzer falls back to a NECESSARY-ONLY bound: the raw
//!   axis-aligned bounding box of every straight-edge vertex, projected
//!   into a canonical frame perpendicular to the shared normal
//!   ([`canonical_planar_extent`]). This bound can PROVE absence (if the
//!   loose boxes don't even overlap, the exact regions provably don't
//!   either — a `Pair` is never in question, so this is safe to skip
//!   silently), but it can never prove PRESENCE (an overlapping loose box
//!   says nothing about whether the exact shapes actually overlap) — when
//!   it overlaps, or when even this loose bound cannot be computed (a
//!   curved boundary edge), the analyzer reports `Unverifiable` for BOTH
//!   candidate faces rather than fabricate a pair. This is exactly the
//!   spec's instruction: axis-aligned bounds as a NECESSARY condition,
//!   exact computation for the shapes this slice supports, honest refusal
//!   everywhere the necessary condition is met but the exact answer is not
//!   provable.
//! - **Cylindrical (axial) extents**: [`axial_extent`] derives a face's
//!   occupied axial range the same way `orientation.rs` derives an
//!   angular range — from the OUTER LOOP's boundary, never the carrier
//!   surface's own (possibly stale, post-boolean) `height_limits` — by
//!   reading every boundary edge: a `Line` contributes its two endpoints'
//!   projections onto the axis; an `Arc` contributes a single projected
//!   value (its center's axial position) ONLY when the arc's own plane is
//!   perpendicular to the axis (an actual rim, checked via the same
//!   `RIM_PERPENDICULAR_TOL` `orientation.rs` uses for the analogous
//!   angular case) — any other boundary curve (a non-rim arc, a NURBS
//!   intersection curve, an inner loop) makes the axial extent
//!   unreconstructable in closed form, and the pair refuses (both faces)
//!   rather than trust the carrier's own limits.
//!
//! ## Multiplicity
//!
//! One face may participate in MULTIPLE pairs (e.g. a U-channel's outer
//! face pairs with two separate inner faces). [`pair_thickness`] performs
//! an exhaustive pairwise check over every candidate face and returns every
//! proven pair — a face is never excluded from a second pair because it
//! already matched a first one.
//!
//! ## Named v1 non-goals (absence ≠ oversight)
//!
//! - Circular/annular PLANAR loops (a full-disk face) are not classified
//!   exactly — only straight-edged rectangles are. A circular planar loop
//!   falls through to the necessary-only bound above (or an immediate
//!   refusal if it has a curved boundary edge and the rectangle
//!   classification already failed).
//! - Rotated (non-axis-aligned-to-each-other) rectangle pairs are not
//!   resolved exactly — `rectangles_axis_aligned` refuses them to the
//!   necessary-only fallback.
//! - Cone/Sphere/Torus/BSpline/NURBS/Offset/Ruled/SurfaceOfRevolution
//!   faces are unconditionally `Unverifiable{UnsupportedSurface}` — this
//!   analyzer never attempts to pair them (spec §3.1's support table:
//!   `pair_thickness` is exact only for parallel-plane and coaxial-
//!   cylinder pairs).

use std::collections::BTreeMap;

use crate::dfm::analyzers::orientation::{to_surface_kind, RIM_PERPENDICULAR_TOL};
use crate::dfm::report::{
    Derivation, DfmError, DfmValue, FaceRef, SurfaceKind, UnverifiableReason,
};
use crate::math::{Point3, Vector3};
use crate::primitives::curve::CurveStore;
use crate::primitives::curve::{Arc, Line};
use crate::primitives::edge::EdgeStore;
use crate::primitives::face::{Face, FaceId, FaceStore};
use crate::primitives::r#loop::LoopStore;
use crate::primitives::surface::{Cylinder, Plane, SurfaceStore, SurfaceType};

/// One proven wall pair (spec §3.1): two faces whose separation is exact
/// (see module docs for the pairing definitions). `face_a < face_b`
/// always (ascending, analyzer-defined order — mirrors
/// `orientation.rs`/`fdm.rs`'s witness-ordering convention).
#[derive(Debug, Clone, PartialEq)]
pub struct FacePair {
    pub face_a: FaceId,
    pub face_b: FaceId,
    pub thickness: DfmValue,
}

/// A candidate wall face whose participation in ANY wall pair could not
/// be established exactly (module docs: "the conservative-refusal line").
/// Not itself a Pass/Violation decision — a rule (e.g. `fdm.min_wall`)
/// folds this into the pack-level honesty theorem.
#[derive(Debug, Clone, PartialEq)]
pub struct UnpairedRegion {
    pub face: FaceRef,
    pub reason: UnverifiableReason,
}

/// The full result of one `pair_thickness` call: every proven pair, plus
/// every face that could not be resolved into a definite yes/no (spec §4:
/// a refusal is a value, never an error — [`DfmError`] is reserved for
/// malformed input such as a dangling face reference).
#[derive(Debug, Clone, PartialEq)]
pub struct PairThicknessOutcome {
    pub pairs: Vec<FacePair>,
    pub unverifiable: Vec<UnpairedRegion>,
}

/// A candidate face's pairing-relevant geometry, classified once up front
/// so the O(n²) pairwise scan below never re-touches the B-Rep stores for
/// data that does not depend on the OTHER face in a candidate pair.
enum Classified {
    Plane {
        /// The face's OUTWARD normal (`face.orientation.sign() *
        /// plane.normal`), unit length.
        normal: Vector3,
        /// Any point on the plane (its stored origin) — enough to derive
        /// the signed separation between two parallel planes.
        offset_point: Point3,
    },
    Cylinder {
        axis_point: Point3,
        axis_dir: Vector3,
        radius: f64,
        /// `face.orientation.sign()` — see module docs' "opposing radial
        /// orientation" for what `+1`/`-1` mean physically.
        sign: f64,
    },
    /// Cone/Sphere/Torus/BSpline/NURBS/Offset/Ruled/SurfaceOfRevolution —
    /// this analyzer never attempts to pair these (module docs' named
    /// non-goals). Already recorded in the caller's `unverifiable` map at
    /// classification time.
    Unsupported,
}

/// An exact axis-aligned (in its OWN plane, relative to its own edges — not
/// necessarily aligned to global axes) rectangle, derived from a face's
/// outer loop by [`rectangle_from_outer_loop`]. `origin` is one corner;
/// `u_dir`/`v_dir` are the two (unit, perpendicular) edge directions out of
/// that corner; the rectangle occupies `[0, s_len] × [0, t_len]` in that
/// local frame.
#[derive(Debug, Clone, Copy)]
pub(super) struct RectangleShape {
    pub(super) origin: Point3,
    pub(super) u_dir: Vector3,
    pub(super) v_dir: Vector3,
    pub(super) s_len: f64,
    pub(super) t_len: f64,
}

// ---- Tolerances (each documented at its use site in the module docs
// above; restated here as the single source of the literal value) ----

/// Two candidate wall faces' outward normals dot to `-1` within this to
/// count as "opposing" (module docs, parallel-plane pair condition 1).
const NORMAL_OPPOSING_TOL: f64 = 1e-9;
/// Minimum separation between two parallel candidate planes to count as a
/// real (non-coincident) wall.
const SEPARATION_EPS: f64 = 1e-9;
/// Minimum positive 1D overlap length (in either projected axis, or along
/// a cylinder pair's shared axis) to count as "material shared" rather
/// than floating-point noise at a touching boundary.
const OVERLAP_EPS: f64 = 1e-9;
/// Two rectangles' in-plane edge directions must dot to `±1` within this
/// tolerance to be treated as sharing a common frame (module docs: the
/// EXACT rectangle-overlap path only fires when this holds).
const AXES_ALIGN_TOL: f64 = 1e-6;
/// Vertices closer than this count as the same point when walking a
/// candidate rectangle's edge cycle.
const VERTEX_COINCIDENT_TOL: f64 = 1e-9;
/// Consecutive rectangle-candidate edges must dot to (near) zero, scaled
/// by their lengths, to count as a right angle.
const RIGHT_ANGLE_REL_TOL: f64 = 1e-9;
/// Opposite sides of a rectangle candidate must match in length within
/// this relative tolerance.
const SIDE_LENGTH_REL_TOL: f64 = 1e-6;
/// Two cylinder axis directions must dot to `±1` within this to count as
/// parallel.
const CYLINDER_AXIS_PARALLEL_TOL: f64 = 1e-9;
/// The perpendicular offset between two (parallel) cylinder axis LINES
/// must be below this to count as colinear — a floating-point round-off
/// tolerance, not a physical fit tolerance (module docs).
const CYLINDER_AXIS_COLINEAR_TOL: f64 = 1e-6;
/// Two cylinder radii must differ by more than this to count as a real
/// (non-degenerate) bore/shaft pair.
const RADIUS_DISTINCT_EPS: f64 = 1e-9;

/// Derive a canonical, deterministic in-plane `(u, v)` orthonormal basis
/// perpendicular to `normal` — used ONLY by the necessary-only fallback
/// bound (module docs), never by the exact rectangle path (which derives
/// its own basis from the rectangle's real edges).
fn canonical_in_plane_basis(normal: Vector3) -> (Vector3, Vector3) {
    let u = normal.perpendicular();
    let v = normal.cross(&u);
    (u, v)
}

/// Classify a face's outer loop as an exact rectangle (module docs: "the
/// conservative-refusal line"). Returns `None` (never a fabricated shape)
/// when the loop is not exactly 4 straight edges forming a closed
/// rectangle, when it has an inner loop, or when any referenced
/// edge/curve/vertex fails to resolve.
///
/// `pub(super)` (not private) since S5:
/// [`crate::dfm::analyzers::internal_voids`] reuses this EXACT
/// classification for its own all-planar-rectangular-faces exact-volume
/// path, rather than re-deriving the same 4-edge/right-angle walk a
/// second time (the no-copies discipline — same promotion precedent as
/// [`axial_extent`]'s S4 reuse by `bore.rs`).
pub(super) fn rectangle_from_outer_loop(
    face: &Face,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
) -> Option<RectangleShape> {
    if !face.inner_loops.is_empty() {
        return None;
    }
    let outer = loop_store.get(face.outer_loop)?;
    if outer.edges.len() != 4 {
        return None;
    }

    let mut segments: Vec<(Point3, Point3)> = Vec::with_capacity(4);
    for &edge_id in &outer.edges {
        let edge = edge_store.get(edge_id)?;
        let curve = curve_store.get(edge.curve_id)?;
        let line = curve.as_any().downcast_ref::<Line>()?;
        segments.push((line.start, line.end));
    }

    let close = |a: Point3, b: Point3| (a - b).magnitude() < VERTEX_COINCIDENT_TOL;

    // Walk the cycle starting from segments[0] — the stored edge ORDER is
    // not assumed to already be a traversal-consistent cycle (this
    // module builds its own fixtures the same way `orientation.rs`'s test
    // module does, from a bare `Vec` of curves; the walk below is robust
    // to whatever order the caller's edges happen to be stored in).
    let mut used = [true, false, false, false];
    let (start, mut current) = segments[0];
    let mut verts = vec![start, current];
    for _ in 0..2 {
        let mut advanced = false;
        for (idx, &(p, q)) in segments.iter().enumerate() {
            if used[idx] {
                continue;
            }
            if close(p, current) {
                current = q;
                used[idx] = true;
                verts.push(current);
                advanced = true;
                break;
            } else if close(q, current) {
                current = p;
                used[idx] = true;
                verts.push(current);
                advanced = true;
                break;
            }
        }
        if !advanced {
            return None;
        }
    }
    let last_idx = (0..4).find(|&i| !used[i])?;
    let (p, q) = segments[last_idx];
    let closes = (close(p, current) && close(q, start)) || (close(q, current) && close(p, start));
    if !closes {
        return None;
    }
    if verts.len() != 4 {
        return None;
    }

    let v0 = verts[0];
    let v1 = verts[1];
    let v2 = verts[2];
    let v3 = verts[3];

    let e0 = v1 - v0;
    let e1 = v2 - v1;
    let e2 = v3 - v2;
    let e3 = v0 - v3;

    let len0 = e0.magnitude();
    let len1 = e1.magnitude();
    if len0 < VERTEX_COINCIDENT_TOL || len1 < VERTEX_COINCIDENT_TOL {
        return None;
    }

    let right_angle = |a: Vector3, b: Vector3| {
        let scale = (a.magnitude() * b.magnitude()).max(1.0);
        a.dot(&b).abs() < RIGHT_ANGLE_REL_TOL * scale
    };
    if !right_angle(e0, e1) || !right_angle(e1, e2) || !right_angle(e2, e3) {
        return None;
    }
    if (e2.magnitude() - len0).abs() > SIDE_LENGTH_REL_TOL * len0.max(1.0) {
        return None;
    }
    if (e3.magnitude() - len1).abs() > SIDE_LENGTH_REL_TOL * len1.max(1.0) {
        return None;
    }

    let u_dir = e0.normalize().ok()?;
    let v_dir = e1.normalize().ok()?;

    Some(RectangleShape {
        origin: v0,
        u_dir,
        v_dir,
        s_len: len0,
        t_len: len1,
    })
}

/// Whether two rectangle candidates share a common in-plane frame (module
/// docs: only then is the EXACT overlap path taken).
fn rectangles_axis_aligned(a: &RectangleShape, b: &RectangleShape) -> bool {
    let uu = a.u_dir.dot(&b.u_dir).abs();
    let uv = a.u_dir.dot(&b.v_dir).abs();
    (uu - 1.0).abs() < AXES_ALIGN_TOL || (uv - 1.0).abs() < AXES_ALIGN_TOL
}

/// The NECESSARY-ONLY fallback bound (module docs): the raw axis-aligned
/// bounding box, in the caller-supplied canonical `(u_dir, v_dir)` frame,
/// of every straight-edge vertex on `face`'s outer loop. Returns `None`
/// (never a fabricated bound) if the face has an inner loop, an empty
/// outer loop, or any boundary edge that is not a `Line` — a curved
/// boundary's true extent is not conservatively bounded in v1 (module
/// docs' named non-goal).
fn canonical_planar_extent(
    face: &Face,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    origin: Point3,
    u_dir: Vector3,
    v_dir: Vector3,
) -> Option<(f64, f64, f64, f64)> {
    if !face.inner_loops.is_empty() {
        return None;
    }
    let outer = loop_store.get(face.outer_loop)?;
    if outer.edges.is_empty() {
        return None;
    }

    let mut s_min = f64::INFINITY;
    let mut s_max = f64::NEG_INFINITY;
    let mut t_min = f64::INFINITY;
    let mut t_max = f64::NEG_INFINITY;
    for &edge_id in &outer.edges {
        let edge = edge_store.get(edge_id)?;
        let curve = curve_store.get(edge.curve_id)?;
        let line = curve.as_any().downcast_ref::<Line>()?;
        for p in [line.start, line.end] {
            let rel = p - origin;
            let s = rel.dot(&u_dir);
            let t = rel.dot(&v_dir);
            s_min = s_min.min(s);
            s_max = s_max.max(s);
            t_min = t_min.min(t);
            t_max = t_max.max(t);
        }
    }
    Some((s_min, s_max, t_min, t_max))
}

/// Record BOTH faces of a candidate pair whose exact overlap could not be
/// established (module docs' conservative-refusal line) — never just one,
/// since neither face alone is "the" problem.
fn mark_ambiguous_pair(
    face_i: FaceId,
    face_j: FaceId,
    unverifiable: &mut BTreeMap<FaceId, UnverifiableReason>,
) {
    let detail = format!(
        "candidate wall pair (faces {face_i}, {face_j}): projected bounds overlap (or cannot \
         be bounded conservatively) but the loop shape/alignment does not admit an exact \
         overlap determination in v1 (only axis-aligned rectangle pairs are computed exactly)"
    );
    unverifiable
        .entry(face_i)
        .or_insert_with(|| UnverifiableReason::UnsupportedTopology {
            detail: detail.clone(),
        });
    unverifiable
        .entry(face_j)
        .or_insert_with(|| UnverifiableReason::UnsupportedTopology { detail });
}

/// Attempt to pair two PLANE-kind candidate faces (module docs: "Parallel
/// plane pair"). Pushes a [`FacePair`] on success, marks both faces
/// ambiguous via [`mark_ambiguous_pair`] when the pairing predicate holds
/// but exact overlap cannot be proven, and does nothing (no pair, no
/// refusal) when the coarse predicate itself proves the faces are NOT a
/// pair (non-opposing normals, coincident planes, or a proven non-
/// overlap).
#[allow(clippy::too_many_arguments)]
fn try_plane_pair(
    face_i: FaceId,
    n_i: Vector3,
    o_i: Point3,
    face_j: FaceId,
    n_j: Vector3,
    o_j: Point3,
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    pairs: &mut Vec<FacePair>,
    unverifiable: &mut BTreeMap<FaceId, UnverifiableReason>,
) -> Result<(), DfmError> {
    if (n_i.dot(&n_j) + 1.0).abs() > NORMAL_OPPOSING_TOL {
        return Ok(()); // not opposing -- proven non-candidate, no ambiguity
    }
    let separation = (o_j - o_i).dot(&n_i);
    if separation.abs() <= SEPARATION_EPS {
        return Ok(()); // coincident planes -- not a wall
    }
    let thickness_value = separation.abs();

    let face_i_ref = face_store
        .get(face_i)
        .ok_or(DfmError::DanglingFaceRef { face: face_i })?;
    let face_j_ref = face_store
        .get(face_j)
        .ok_or(DfmError::DanglingFaceRef { face: face_j })?;

    let rect_i = rectangle_from_outer_loop(face_i_ref, loop_store, edge_store, curve_store);
    let rect_j = rectangle_from_outer_loop(face_j_ref, loop_store, edge_store, curve_store);

    if let (Some(ri), Some(rj)) = (rect_i, rect_j) {
        if rectangles_axis_aligned(&ri, &rj) {
            let corners_b = [
                rj.origin,
                rj.origin + rj.u_dir * rj.s_len,
                rj.origin + rj.u_dir * rj.s_len + rj.v_dir * rj.t_len,
                rj.origin + rj.v_dir * rj.t_len,
            ];
            let mut s_min = f64::INFINITY;
            let mut s_max = f64::NEG_INFINITY;
            let mut t_min = f64::INFINITY;
            let mut t_max = f64::NEG_INFINITY;
            for c in corners_b {
                let rel = c - ri.origin;
                let s = rel.dot(&ri.u_dir);
                let t = rel.dot(&ri.v_dir);
                s_min = s_min.min(s);
                s_max = s_max.max(s);
                t_min = t_min.min(t);
                t_max = t_max.max(t);
            }
            let overlap_s = ri.s_len.min(s_max) - 0f64.max(s_min);
            let overlap_t = ri.t_len.min(t_max) - 0f64.max(t_min);
            if overlap_s > OVERLAP_EPS && overlap_t > OVERLAP_EPS {
                pairs.push(FacePair {
                    face_a: face_i.min(face_j),
                    face_b: face_i.max(face_j),
                    thickness: DfmValue::new(
                        thickness_value,
                        Derivation::Analytic {
                            surface_type: SurfaceKind::Plane,
                            method: "parallel plane pair: separation along shared outward \
                                     normal, exact overlap of trimmed rectangular loops"
                                .to_string(),
                        },
                    ),
                });
            }
            // else: exact rectangles, exact overlap test says no overlap
            // -- proven absent, not a refusal.
            return Ok(());
        }
    }

    // Fallback: necessary-only bound (module docs' conservative-refusal
    // line).
    let (canon_u, canon_v) = canonical_in_plane_basis(n_i);
    let extent_i = canonical_planar_extent(
        face_i_ref,
        loop_store,
        edge_store,
        curve_store,
        o_i,
        canon_u,
        canon_v,
    );
    let extent_j = canonical_planar_extent(
        face_j_ref,
        loop_store,
        edge_store,
        curve_store,
        o_i,
        canon_u,
        canon_v,
    );

    match (extent_i, extent_j) {
        (Some((s0i, s1i, t0i, t1i)), Some((s0j, s1j, t0j, t1j))) => {
            let overlap_s = s1i.min(s1j) - s0i.max(s0j);
            let overlap_t = t1i.min(t1j) - t0i.max(t0j);
            if overlap_s > OVERLAP_EPS && overlap_t > OVERLAP_EPS {
                mark_ambiguous_pair(face_i, face_j, unverifiable);
            }
            // else: even the loose necessary bound proves no overlap --
            // definitively not a wall, not a refusal.
        }
        _ => {
            // Cannot even bound one side conservatively (a curved or
            // otherwise-unclassified boundary on a face that also failed
            // exact rectangle classification).
            mark_ambiguous_pair(face_i, face_j, unverifiable);
        }
    }
    Ok(())
}

/// Derive a CYLINDER face's exact occupied axial range from its OUTER
/// LOOP's boundary (module docs: mirrors `orientation.rs`'s angular-domain
/// derivation, applied to the axial direction instead). `axis_point`/
/// `axis_dir` are the SHARED reference axis (the caller passes the same
/// one for both faces of a candidate pair, since they have already been
/// proven colinear) so the two faces' `v` values are directly comparable.
/// Returns `None` when the boundary is not a straight generatrix / axis-
/// perpendicular rim, has an inner loop, or is empty.
///
/// `pub(super)` (not private): [`crate::dfm::analyzers::bore::bore_metrics`]
/// (spec S4) reuses this EXACT function for a bore wall face's own trimmed
/// axial extent, rather than re-deriving the same Line/rim-Arc walk a
/// second time — the no-copies discipline applies to this helper as much
/// as to any production entry point. `bore_metrics` needs a SEPARATE,
/// laxer helper for the SOLID's own extent along the same axis (which must
/// walk every face's inner loops too, since a plate's flat faces carry the
/// bore's rim as an inner loop) — that is a different question (the
/// solid's envelope, not one face's trim) and lives in `bore.rs` itself,
/// not here.
pub(super) fn axial_extent(
    face: &Face,
    axis_point: Point3,
    axis_dir: Vector3,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
) -> Option<(f64, f64)> {
    if !face.inner_loops.is_empty() {
        return None;
    }
    let outer = loop_store.get(face.outer_loop)?;
    if outer.edges.is_empty() {
        return None;
    }

    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;
    for &edge_id in &outer.edges {
        let edge = edge_store.get(edge_id)?;
        let curve = curve_store.get(edge.curve_id)?;
        if let Some(line) = curve.as_any().downcast_ref::<Line>() {
            for p in [line.start, line.end] {
                let v = (p - axis_point).dot(&axis_dir);
                v_min = v_min.min(v);
                v_max = v_max.max(v);
            }
        } else if let Some(arc) = curve.as_any().downcast_ref::<Arc>() {
            let alignment = arc.normal.dot(&axis_dir);
            if (alignment.abs() - 1.0).abs() > RIM_PERPENDICULAR_TOL {
                return None; // not a rim -- cannot trust the single-v assumption
            }
            let v = (arc.center - axis_point).dot(&axis_dir);
            v_min = v_min.min(v);
            v_max = v_max.max(v);
        } else {
            return None; // unsupported boundary curve kind
        }
    }
    if v_min.is_finite() && v_max.is_finite() {
        Some((v_min, v_max))
    } else {
        None
    }
}

/// Attempt to pair two CYLINDER-kind candidate faces (module docs:
/// "Coaxial cylinder pair"). Same success/ambiguous/proven-absent
/// trichotomy as [`try_plane_pair`].
#[allow(clippy::too_many_arguments)]
fn try_cylinder_pair(
    face_i: FaceId,
    axis_point_i: Point3,
    axis_dir_i: Vector3,
    radius_i: f64,
    sign_i: f64,
    face_j: FaceId,
    axis_point_j: Point3,
    axis_dir_j: Vector3,
    radius_j: f64,
    sign_j: f64,
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    pairs: &mut Vec<FacePair>,
    unverifiable: &mut BTreeMap<FaceId, UnverifiableReason>,
) -> Result<(), DfmError> {
    let axis_dot = axis_dir_i.dot(&axis_dir_j);
    if (axis_dot.abs() - 1.0).abs() > CYLINDER_AXIS_PARALLEL_TOL {
        return Ok(()); // not even parallel -- proven non-candidate
    }
    let delta = axis_point_j - axis_point_i;
    let perp = delta - axis_dir_i * delta.dot(&axis_dir_i);
    if perp.magnitude() > CYLINDER_AXIS_COLINEAR_TOL {
        return Ok(()); // parallel but offset axes -- not coaxial
    }
    if sign_i * sign_j > 0.0 {
        return Ok(()); // same radial sense -- not a bore/shaft pair
    }
    if (radius_i - radius_j).abs() <= RADIUS_DISTINCT_EPS {
        return Ok(()); // degenerate (same radius)
    }
    let thickness_value = (radius_i - radius_j).abs();

    let face_i_ref = face_store
        .get(face_i)
        .ok_or(DfmError::DanglingFaceRef { face: face_i })?;
    let face_j_ref = face_store
        .get(face_j)
        .ok_or(DfmError::DanglingFaceRef { face: face_j })?;

    let extent_i = axial_extent(
        face_i_ref,
        axis_point_i,
        axis_dir_i,
        loop_store,
        edge_store,
        curve_store,
    );
    let extent_j = axial_extent(
        face_j_ref,
        axis_point_i,
        axis_dir_i,
        loop_store,
        edge_store,
        curve_store,
    );

    match (extent_i, extent_j) {
        (Some((vmin_i, vmax_i)), Some((vmin_j, vmax_j))) => {
            let overlap_v = vmax_i.min(vmax_j) - vmin_i.max(vmin_j);
            if overlap_v > OVERLAP_EPS {
                pairs.push(FacePair {
                    face_a: face_i.min(face_j),
                    face_b: face_i.max(face_j),
                    thickness: DfmValue::new(
                        thickness_value,
                        Derivation::Analytic {
                            surface_type: SurfaceKind::Cylinder,
                            method: "coaxial cylinder pair: radius difference, exact \
                                     axial-extent overlap"
                                .to_string(),
                        },
                    ),
                });
            }
            // else: exact axial extents, proven no overlap -- not a wall.
        }
        _ => mark_ambiguous_pair(face_i, face_j, unverifiable),
    }
    Ok(())
}

/// Per-face wall-thickness pairing over `faces` (spec §3.1 `pair_thickness`,
/// spec §3.2 `fdm.min_wall`'s analyzer). See the module docs for the exact
/// pairing definitions and the conservative-refusal line.
///
/// Returns `Err` only for malformed input (a dangling face reference).
/// Every OTHER outcome — a proven pair, a proven absence, or an honest
/// "cannot decide" — is a VALUE inside the returned
/// [`PairThicknessOutcome`] (spec §4).
pub fn pair_thickness(
    faces: &[FaceId],
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface_store: &SurfaceStore,
) -> Result<PairThicknessOutcome, DfmError> {
    let mut classified: Vec<(FaceId, Classified)> = Vec::with_capacity(faces.len());
    let mut unverifiable: BTreeMap<FaceId, UnverifiableReason> = BTreeMap::new();

    for &face_id in faces {
        let face = face_store
            .get(face_id)
            .ok_or(DfmError::DanglingFaceRef { face: face_id })?;
        let surface = surface_store
            .get(face.surface_id)
            .ok_or(DfmError::DanglingFaceRef { face: face_id })?;

        match surface.surface_type() {
            SurfaceType::Plane => {
                // surface_type() == Plane guarantees this downcast
                // succeeds (every `Surface` impl's `surface_type()`
                // returns the constant matching its own concrete type --
                // verified by reading each impl in primitives/surface.rs,
                // same invariant `orientation.rs` relies on).
                #[allow(clippy::expect_used)]
                let plane = surface
                    .as_any()
                    .downcast_ref::<Plane>()
                    .expect("surface_type() == Plane guarantees Plane downcast");
                let normal = plane.normal * face.orientation.sign();
                classified.push((
                    face_id,
                    Classified::Plane {
                        normal,
                        offset_point: plane.origin,
                    },
                ));
            }
            SurfaceType::Cylinder => {
                #[allow(clippy::expect_used)]
                let cyl = surface
                    .as_any()
                    .downcast_ref::<Cylinder>()
                    .expect("surface_type() == Cylinder guarantees Cylinder downcast");
                classified.push((
                    face_id,
                    Classified::Cylinder {
                        axis_point: cyl.origin,
                        axis_dir: cyl.axis,
                        radius: cyl.radius,
                        sign: face.orientation.sign(),
                    },
                ));
            }
            other => {
                classified.push((face_id, Classified::Unsupported));
                unverifiable.entry(face_id).or_insert_with(|| {
                    UnverifiableReason::UnsupportedSurface {
                        surface_type: to_surface_kind(other),
                        analyzer: "pair_thickness".to_string(),
                    }
                });
            }
        }
    }

    let mut pairs: Vec<FacePair> = Vec::new();

    for i in 0..classified.len() {
        for j in (i + 1)..classified.len() {
            let (face_i, class_i) = &classified[i];
            let (face_j, class_j) = &classified[j];
            match (class_i, class_j) {
                (
                    Classified::Plane {
                        normal: n_i,
                        offset_point: o_i,
                    },
                    Classified::Plane {
                        normal: n_j,
                        offset_point: o_j,
                    },
                ) => {
                    try_plane_pair(
                        *face_i,
                        *n_i,
                        *o_i,
                        *face_j,
                        *n_j,
                        *o_j,
                        face_store,
                        loop_store,
                        edge_store,
                        curve_store,
                        &mut pairs,
                        &mut unverifiable,
                    )?;
                }
                (
                    Classified::Cylinder {
                        axis_point: ap_i,
                        axis_dir: ad_i,
                        radius: r_i,
                        sign: s_i,
                    },
                    Classified::Cylinder {
                        axis_point: ap_j,
                        axis_dir: ad_j,
                        radius: r_j,
                        sign: s_j,
                    },
                ) => {
                    try_cylinder_pair(
                        *face_i,
                        *ap_i,
                        *ad_i,
                        *r_i,
                        *s_i,
                        *face_j,
                        *ap_j,
                        *ad_j,
                        *r_j,
                        *s_j,
                        face_store,
                        loop_store,
                        edge_store,
                        curve_store,
                        &mut pairs,
                        &mut unverifiable,
                    )?;
                }
                _ => {} // Plane/Cylinder cross-kind or anything involving
                        // `Unsupported`: never a valid pair combination in
                        // v1 (module docs' named non-goals); `Unsupported`
                        // faces are already recorded above.
            }
        }
    }

    let unverifiable_regions: Vec<UnpairedRegion> = unverifiable
        .into_iter()
        .map(|(face, reason)| UnpairedRegion { face, reason })
        .collect();

    Ok(PairThicknessOutcome {
        pairs,
        unverifiable: unverifiable_regions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::curve::ParameterRange;
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::face::FaceOrientation;
    use crate::primitives::r#loop::{Loop, LoopType};

    /// Shared store bundle every fixture below builds into — mirrors the
    /// bare-store convention `orientation.rs`'s and `packs::fixtures`'s own
    /// test modules use (booleans/extrude are a `KNOWN_REDS` hazard area
    /// these tests deliberately avoid).
    struct Stores {
        surfaces: SurfaceStore,
        faces: FaceStore,
        loops: LoopStore,
        edges: EdgeStore,
        curves: CurveStore,
    }

    impl Stores {
        fn new() -> Self {
            Self {
                surfaces: SurfaceStore::new(),
                faces: FaceStore::new(),
                loops: LoopStore::new(),
                edges: EdgeStore::new(),
                curves: CurveStore::new(),
            }
        }

        /// Add a rectangular PLANE face with outward normal `normal`
        /// (unit) and corners `v0, v1, v2, v3` (in cycle order, right
        /// angles at each corner) to the shared stores.
        fn add_rectangle_face(
            &mut self,
            normal: Vector3,
            plane_point: Point3,
            v0: Point3,
            v1: Point3,
            v2: Point3,
            v3: Point3,
        ) -> FaceId {
            let plane = Plane::from_point_normal(plane_point, normal)
                .unwrap_or_else(|e| panic!("valid plane fixture: {e}"));
            let surface_id = self.surfaces.add(Box::new(plane));

            let mut loop_ = Loop::new(0, LoopType::Outer);
            for (start, end) in [(v0, v1), (v1, v2), (v2, v3), (v3, v0)] {
                let curve_id = self.curves.add(Box::new(Line::new(start, end)));
                let edge = Edge::new(
                    0,
                    0,
                    1,
                    curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::unit(),
                );
                let edge_id = self.edges.add(edge);
                loop_.add_edge(edge_id, true);
            }
            let outer_loop = self.loops.add(loop_);

            let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
            self.faces.add(face)
        }

        /// Add a CYLINDER face (full circle rims at `v = v_bottom` and
        /// `v = v_top`, no generatrix lines needed since it is a full
        /// circle) with the given `orientation` (`Forward` = outward
        /// normal points away from axis, i.e. material-inside/shaft-like;
        /// `Backward` = outward normal points toward axis, i.e. material-
        /// outside/bore-wall-like — module docs' sign convention).
        #[allow(clippy::too_many_arguments)]
        fn add_full_cylinder_face(
            &mut self,
            origin: Point3,
            axis: Vector3,
            radius: f64,
            v_bottom: f64,
            v_top: f64,
            orientation: FaceOrientation,
        ) -> FaceId {
            let cylinder = Cylinder::new(origin, axis, radius).unwrap_or_else(|e| panic!("{e}"));
            let surface_id = self.surfaces.add(Box::new(cylinder));

            let bottom = Arc::circle(origin + axis * v_bottom, axis, radius)
                .unwrap_or_else(|e| panic!("valid arc fixture: {e}"));
            let top = Arc::circle(origin + axis * v_top, axis, radius)
                .unwrap_or_else(|e| panic!("valid arc fixture: {e}"));

            let mut loop_ = Loop::new(0, LoopType::Outer);
            for curve in [
                Box::new(bottom) as Box<dyn crate::primitives::curve::Curve>,
                Box::new(top),
            ] {
                let curve_id = self.curves.add(curve);
                let edge = Edge::new(
                    0,
                    0,
                    1,
                    curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::unit(),
                );
                let edge_id = self.edges.add(edge);
                loop_.add_edge(edge_id, true);
            }
            let outer_loop = self.loops.add(loop_);

            let face = Face::new(0, surface_id, outer_loop, orientation);
            self.faces.add(face)
        }
    }

    // ----- Headline hand-computed case: a box has exactly 3 wall pairs -----

    /// A box `[0, Lx] × [0, Ly] × [0, Lz]` built as 6 independent plane
    /// faces, each with its outward normal set DIRECTLY to the box's true
    /// outward direction (the simplest, most standard box construction --
    /// no orientation-flip subtlety needed for planes, unlike the
    /// cylinder case). Exactly 3 pairs of these 6 are parallel+opposing:
    /// `{x=0, x=Lx}`, `{y=0, y=Ly}`, `{z=0, z=Lz}` — every other
    /// combination is perpendicular, never a candidate.
    #[test]
    fn box_has_exactly_three_wall_pairs_with_exact_thicknesses() {
        let (lx, ly, lz) = (3.0, 4.0, 5.0);
        let mut s = Stores::new();

        let x0 = s.add_rectangle_face(
            Vector3::new(-1.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, ly, 0.0),
            Point3::new(0.0, ly, lz),
            Point3::new(0.0, 0.0, lz),
        );
        let x1 = s.add_rectangle_face(
            Vector3::new(1.0, 0.0, 0.0),
            Point3::new(lx, 0.0, 0.0),
            Point3::new(lx, 0.0, 0.0),
            Point3::new(lx, ly, 0.0),
            Point3::new(lx, ly, lz),
            Point3::new(lx, 0.0, lz),
        );
        let y0 = s.add_rectangle_face(
            Vector3::new(0.0, -1.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(lx, 0.0, 0.0),
            Point3::new(lx, 0.0, lz),
            Point3::new(0.0, 0.0, lz),
        );
        let y1 = s.add_rectangle_face(
            Vector3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, ly, 0.0),
            Point3::new(0.0, ly, 0.0),
            Point3::new(lx, ly, 0.0),
            Point3::new(lx, ly, lz),
            Point3::new(0.0, ly, lz),
        );
        let z0 = s.add_rectangle_face(
            Vector3::new(0.0, 0.0, -1.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(lx, 0.0, 0.0),
            Point3::new(lx, ly, 0.0),
            Point3::new(0.0, ly, 0.0),
        );
        let z1 = s.add_rectangle_face(
            Vector3::new(0.0, 0.0, 1.0),
            Point3::new(0.0, 0.0, lz),
            Point3::new(0.0, 0.0, lz),
            Point3::new(lx, 0.0, lz),
            Point3::new(lx, ly, lz),
            Point3::new(0.0, ly, lz),
        );

        let outcome = pair_thickness(
            &[x0, x1, y0, y1, z0, z1],
            &s.faces,
            &s.loops,
            &s.edges,
            &s.curves,
            &s.surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.unverifiable.is_empty(),
            "a plain box must not refuse any face: {:?}",
            outcome.unverifiable
        );
        assert_eq!(
            outcome.pairs.len(),
            3,
            "expected exactly 3 wall pairs, got {:?}",
            outcome.pairs
        );

        let mut thicknesses: Vec<f64> = outcome.pairs.iter().map(|p| p.thickness.value).collect();
        thicknesses.sort_by(|a, b| a.total_cmp(b));
        let mut expected = vec![lx, ly, lz];
        expected.sort_by(|a, b| a.total_cmp(b));
        for (got, want) in thicknesses.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-9, "got {got}, want {want}");
        }
    }

    // ----- Bore wall: coaxial cylinders -----

    #[test]
    fn coaxial_cylinders_report_exact_radius_difference() {
        let mut s = Stores::new();
        let origin = Point3::new(0.0, 0.0, 0.0);
        let axis = Vector3::Z;
        let (r_outer, r_inner, height) = (5.0, 3.0, 10.0);

        // Outer (shaft-like) surface: Forward -> outward normal points
        // away from the axis (material inside).
        let outer =
            s.add_full_cylinder_face(origin, axis, r_outer, 0.0, height, FaceOrientation::Forward);
        // Inner (bore) surface: Backward -> outward normal points toward
        // the axis (material outside, void toward axis).
        let inner = s.add_full_cylinder_face(
            origin,
            axis,
            r_inner,
            0.0,
            height,
            FaceOrientation::Backward,
        );

        let outcome = pair_thickness(
            &[outer, inner],
            &s.faces,
            &s.loops,
            &s.edges,
            &s.curves,
            &s.surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(outcome.unverifiable.is_empty());
        assert_eq!(outcome.pairs.len(), 1, "expected exactly 1 bore pair");
        let pair = &outcome.pairs[0];
        assert!((pair.thickness.value - (r_outer - r_inner)).abs() < 1e-9);
        assert_eq!(pair.face_a.min(pair.face_b), outer.min(inner));
        assert_eq!(pair.face_a.max(pair.face_b), outer.max(inner));
    }

    /// Same radial sense (both `Forward`, i.e. both "outer" surfaces) must
    /// NEVER pair even though they are coaxial with distinct radii — this
    /// is the cylinder analogue of the opposing-normals requirement.
    #[test]
    fn coaxial_cylinders_with_same_radial_sense_do_not_pair() {
        let mut s = Stores::new();
        let origin = Point3::new(0.0, 0.0, 0.0);
        let axis = Vector3::Z;

        let a = s.add_full_cylinder_face(origin, axis, 5.0, 0.0, 10.0, FaceOrientation::Forward);
        let b = s.add_full_cylinder_face(origin, axis, 3.0, 0.0, 10.0, FaceOrientation::Forward);

        let outcome = pair_thickness(
            &[a, b],
            &s.faces,
            &s.loops,
            &s.edges,
            &s.curves,
            &s.surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(outcome.pairs.is_empty());
        assert!(outcome.unverifiable.is_empty());
    }

    // ----- THE ANTI-FABRICATION HEADLINE -----

    /// Two parallel, opposing-normal rectangle faces whose projections do
    /// NOT overlap (offset along the in-plane X axis by a gap) must
    /// produce NO pair — the exact "a gap between parts reads as a wall"
    /// failure mode this analyzer exists to prevent.
    #[test]
    fn non_overlapping_parallel_faces_produce_no_pair() {
        let mut s = Stores::new();

        // Face A: z = 0 plane, outward normal -Z (facing down), footprint
        // x in [0, 1], y in [0, 1].
        let face_a = s.add_rectangle_face(
            Vector3::new(0.0, 0.0, -1.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        );
        // Face B: z = -1 plane, outward normal +Z (facing up, i.e.
        // opposing A) -- but footprint x in [2, 3], y in [0, 1]: NO
        // overlap with A's footprint.
        let face_b = s.add_rectangle_face(
            Vector3::new(0.0, 0.0, 1.0),
            Point3::new(0.0, 0.0, -1.0),
            Point3::new(2.0, 0.0, -1.0),
            Point3::new(3.0, 0.0, -1.0),
            Point3::new(3.0, 1.0, -1.0),
            Point3::new(2.0, 1.0, -1.0),
        );

        let outcome = pair_thickness(
            &[face_a, face_b],
            &s.faces,
            &s.loops,
            &s.edges,
            &s.curves,
            &s.surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.pairs.is_empty(),
            "non-overlapping projections must never fabricate a wall pair: {:?}",
            outcome.pairs
        );
        assert!(
            outcome.unverifiable.is_empty(),
            "a proven non-overlap is not a refusal either: {:?}",
            outcome.unverifiable
        );
    }

    /// Mutation proof, raw before/after: a deliberately-wrong stand-in for
    /// `try_plane_pair` that OMITS the overlap check entirely (pairs any
    /// two opposing, separated planes regardless of projected overlap) —
    /// exactly the bug class the previous test exists to catch. Shown
    /// directly (not by toggling production code) per this module's own
    /// `orientation.rs` precedent for mutation-proof tests.
    #[test]
    fn mutation_proof_omitting_overlap_check_would_fabricate_a_pair() {
        let mut s = Stores::new();
        let face_a = s.add_rectangle_face(
            Vector3::new(0.0, 0.0, -1.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        );
        let face_b = s.add_rectangle_face(
            Vector3::new(0.0, 0.0, 1.0),
            Point3::new(0.0, 0.0, -1.0),
            Point3::new(2.0, 0.0, -1.0),
            Point3::new(3.0, 0.0, -1.0),
            Point3::new(3.0, 1.0, -1.0),
            Point3::new(2.0, 1.0, -1.0),
        );

        // BEFORE (mutant): only checks opposing normals + positive
        // separation, never overlap -- exactly what a "nearest opposing
        // face" heuristic would do.
        let n_a = Vector3::new(0.0, 0.0, -1.0);
        let n_b = Vector3::new(0.0, 0.0, 1.0);
        let o_a = Point3::new(0.0, 0.0, 0.0);
        let o_b = Point3::new(0.0, 0.0, -1.0);
        let mutant_would_pair = (n_a.dot(&n_b) + 1.0).abs() < NORMAL_OPPOSING_TOL
            && (o_b - o_a).dot(&n_a).abs() > SEPARATION_EPS;
        assert!(
            mutant_would_pair,
            "the mutant's coarse (overlap-free) predicate must actually fire here, or this \
             test proves nothing about the overlap check specifically"
        );

        // AFTER (real production path): the actual analyzer, which DOES
        // check overlap.
        let outcome = pair_thickness(
            &[face_a, face_b],
            &s.faces,
            &s.loops,
            &s.edges,
            &s.curves,
            &s.surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));
        assert!(
            outcome.pairs.is_empty(),
            "production path must refuse to pair non-overlapping faces even though the \
             overlap-free mutant predicate says they qualify"
        );
    }

    /// Two parallel faces facing the SAME way (both normals +Z, e.g. two
    /// "top" faces at different heights) must never pair, even though
    /// their footprints fully overlap and they are perfectly parallel —
    /// the opposing-normals requirement is independent of overlap.
    #[test]
    fn same_direction_normals_do_not_pair_even_with_full_overlap() {
        let mut s = Stores::new();
        let face_a = s.add_rectangle_face(
            Vector3::new(0.0, 0.0, 1.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        );
        let face_b = s.add_rectangle_face(
            Vector3::new(0.0, 0.0, 1.0),
            Point3::new(0.0, 0.0, 2.0),
            Point3::new(0.0, 0.0, 2.0),
            Point3::new(1.0, 0.0, 2.0),
            Point3::new(1.0, 1.0, 2.0),
            Point3::new(0.0, 1.0, 2.0),
        );

        let outcome = pair_thickness(
            &[face_a, face_b],
            &s.faces,
            &s.loops,
            &s.edges,
            &s.curves,
            &s.surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(outcome.pairs.is_empty());
        assert!(outcome.unverifiable.is_empty());
    }

    // ----- Conservative-refusal boundary: ambiguous (non-rectangular)
    // candidate pair -----

    /// A TRIANGULAR planar face (3 straight edges, not a rectangle)
    /// candidate-paired against a rectangle whose necessary bounding box
    /// DOES overlap it: this analyzer cannot compute the exact overlap of
    /// a triangle against a rectangle in v1, so it must refuse BOTH faces
    /// rather than guess — the precise "conservative-refusal line" the
    /// module docs describe, exercised directly.
    #[test]
    fn ambiguous_non_rectangular_candidate_pair_is_unverifiable_not_guessed() {
        let mut s = Stores::new();

        // Rectangle at z = 0, outward normal -Z, footprint x in [0,2], y
        // in [0,2].
        let rect = s.add_rectangle_face(
            Vector3::new(0.0, 0.0, -1.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(2.0, 2.0, 0.0),
            Point3::new(0.0, 2.0, 0.0),
        );

        // Triangle at z = -1, outward normal +Z (opposing), footprint
        // squarely inside the rectangle's necessary bounding box.
        let plane =
            Plane::from_point_normal(Point3::new(0.0, 0.0, -1.0), Vector3::new(0.0, 0.0, 1.0))
                .unwrap_or_else(|e| panic!("valid plane fixture: {e}"));
        let surface_id = s.surfaces.add(Box::new(plane));
        let tri_verts = [
            Point3::new(0.5, 0.5, -1.0),
            Point3::new(1.5, 0.5, -1.0),
            Point3::new(1.0, 1.5, -1.0),
        ];
        let mut loop_ = Loop::new(0, LoopType::Outer);
        for i in 0..3 {
            let (start, end) = (tri_verts[i], tri_verts[(i + 1) % 3]);
            let curve_id = s.curves.add(Box::new(Line::new(start, end)));
            let edge = Edge::new(
                0,
                0,
                1,
                curve_id,
                EdgeOrientation::Forward,
                ParameterRange::unit(),
            );
            let edge_id = s.edges.add(edge);
            loop_.add_edge(edge_id, true);
        }
        let outer_loop = s.loops.add(loop_);
        let tri_face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
        let tri = s.faces.add(tri_face);

        let outcome = pair_thickness(
            &[rect, tri],
            &s.faces,
            &s.loops,
            &s.edges,
            &s.curves,
            &s.surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(
            outcome.pairs.is_empty(),
            "a triangle-vs-rectangle overlap must never be fabricated into an exact pair: {:?}",
            outcome.pairs
        );
        assert_eq!(
            outcome.unverifiable.len(),
            2,
            "both faces of the ambiguous candidate pair \
                    must be reported, not silently dropped: {:?}",
            outcome.unverifiable
        );
        let flagged: Vec<FaceId> = outcome.unverifiable.iter().map(|u| u.face).collect();
        assert!(flagged.contains(&rect));
        assert!(flagged.contains(&tri));
        for region in &outcome.unverifiable {
            assert!(matches!(
                region.reason,
                UnverifiableReason::UnsupportedTopology { .. }
            ));
        }
    }

    // ----- Refusal flow-through: unsupported surface kind -----

    #[test]
    fn nurbs_face_is_always_unverifiable_regardless_of_partners() {
        let mut s = Stores::new();
        let nurbs = crate::math::nurbs::NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
                vec![Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0)],
            ],
            vec![vec![1.0, 1.0], vec![1.0, 1.0]],
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        )
        .unwrap_or_else(|e| panic!("trivial flat NURBS patch fixture: {e}"));
        let surface = crate::primitives::surface::GeneralNurbsSurface { nurbs };
        let surface_id = s.surfaces.add(Box::new(surface));
        let outer_loop = s.loops.add(Loop::new(0, LoopType::Outer));
        let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
        let face_id = s.faces.add(face);

        let outcome = pair_thickness(
            &[face_id],
            &s.faces,
            &s.loops,
            &s.edges,
            &s.curves,
            &s.surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert!(outcome.pairs.is_empty());
        assert_eq!(outcome.unverifiable.len(), 1);
        assert_eq!(outcome.unverifiable[0].face, face_id);
        match &outcome.unverifiable[0].reason {
            UnverifiableReason::UnsupportedSurface { surface_type, .. } => {
                assert_eq!(*surface_type, SurfaceKind::Nurbs)
            }
            other => panic!("expected UnsupportedSurface, got {other:?}"),
        }
    }

    #[test]
    fn dangling_face_ref_is_an_error_not_a_refusal() {
        let s = Stores::new();
        let result = pair_thickness(&[999], &s.faces, &s.loops, &s.edges, &s.curves, &s.surfaces);
        assert!(matches!(
            result,
            Err(DfmError::DanglingFaceRef { face: 999 })
        ));
    }
}
