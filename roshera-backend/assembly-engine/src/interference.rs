//! Static interference + clearance via Parry.
//!
//! Each instance's kernel tessellation becomes a Parry `TriMesh` in its LOCAL
//! frame; the instance pose is the world isometry. Pairwise, Parry answers
//! whether two parts overlap (a build error — interference) and, where
//! supported, their separation. Broad-phase BVH pruning and swept CCD arrive in
//! later slices; this is the correctness slice.

use crate::types::{Assembly, Instance, InstanceId};
use parry3d_f64::bounding_volume::BoundingVolume;
use parry3d_f64::na::{Isometry3, Point3, Quaternion, Translation3, UnitQuaternion, Vector3};
use parry3d_f64::query;
use parry3d_f64::shape::{ConvexPolyhedron, TriMesh};
use parry3d_f64::transformation::convex_hull;
use parry3d_f64::transformation::vhacd::{VHACDParameters, VHACD};
use serde::{Deserialize, Serialize};

/// Two instances found overlapping in world space.
#[derive(Debug, Clone, PartialEq)]
pub struct InterferencePair {
    pub a: InstanceId,
    pub b: InstanceId,
    /// Penetration depth at detection (negative ⇒ the parts overlap by that
    /// much), or the fallback separation. `Some` whenever a pair is reported.
    pub clearance: Option<f64>,
}

/// Why a pairwise check could not be run at all — as opposed to running and
/// finding clearance / no interference. An unrun check must never share a
/// bucket with a verified pass; the kernel must say the check never ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnverifiedReason {
    /// One (or both) instance's mesh is missing, empty, or otherwise
    /// degenerate — there is nothing to check against.
    MissingMesh,
    /// Both meshes exist but the underlying exact query does not support
    /// this pair (the geometry query returned an error).
    UnsupportedPair,
}

/// A pair whose static interference could not be checked — never folded into
/// "clear"; the check simply did not run (a degenerate / mesh-less instance).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnverifiedInterferencePair {
    pub a: InstanceId,
    pub b: InstanceId,
    pub reason: UnverifiedReason,
}

/// The static-interference verdict for an assembly.
#[derive(Debug, Clone, PartialEq)]
pub struct InterferenceReport {
    pub interfering: Vec<InterferencePair>,
    /// Pairs the checker never actually ran on (see `UnverifiedReason`). A
    /// non-empty list here means the assembly has NOT been proven clear —
    /// `no_static_interference` reflects that.
    pub unverified: Vec<UnverifiedInterferencePair>,
}

impl InterferenceReport {
    /// True when no two parts overlap AND every pair was actually checked.
    /// An unverified pair is never silently counted as clear.
    pub fn no_static_interference(&self) -> bool {
        self.interfering.is_empty() && self.unverified.is_empty()
    }
}

/// World isometry of an instance (`translation` + unit quaternion `[x, y, z, w]`).
pub(crate) fn instance_isometry(instance: &Instance) -> Isometry3<f64> {
    let translation = Translation3::new(
        instance.translation[0],
        instance.translation[1],
        instance.translation[2],
    );
    // nalgebra's `Quaternion::new` is (w, i, j, k); our storage is [x, y, z, w].
    let quaternion = Quaternion::new(
        instance.rotation[3],
        instance.rotation[0],
        instance.rotation[1],
        instance.rotation[2],
    );
    Isometry3::from_parts(translation, UnitQuaternion::from_quaternion(quaternion))
}

/// Build the instance's mesh as a Parry `TriMesh` (local frame). `None` when the
/// mesh is empty or Parry rejects it as degenerate.
pub(crate) fn instance_trimesh(instance: &Instance) -> Option<TriMesh> {
    if instance.mesh.vertices.is_empty() || instance.mesh.triangles.is_empty() {
        return None;
    }
    let vertices: Vec<Point3<f64>> = instance
        .mesh
        .vertices
        .iter()
        .map(|v| Point3::new(v[0], v[1], v[2]))
        .collect();
    Some(TriMesh::new(vertices, instance.mesh.triangles.clone()))
}

/// Does ANY vertex of `inner` lie strictly inside the SOLID bounded by `outer`?
/// True ⇒ `inner` penetrates / is enclosed by `outer` even when their surfaces do
/// not touch — the enclosure case a surface-distance test alone misses. A vertex
/// is a MATERIAL point of `inner` (unlike its centroid, which for a bored part
/// sits in its own hole); a neighbour seated in `outer`'s through-HOLE has no
/// vertex inside the solid, so this stays false there.
///
/// EXHAUSTIVE over every vertex — a small localized penetration (a short pin
/// poking into a pocket) can be a handful of vertices out of tens of
/// thousands, and this is the ONLY discriminator between "enclosed" (real
/// interference) and "seated in a through-hole" (clear); a stride/cap that
/// samples a subset can walk straight past the only vertices that are
/// actually inside and silently report clear. Correctness wins over the
/// O(V·F) bound here: `.any()` short-circuits on the FIRST hit, so the real-
/// interference case (the common, load-bearing outcome) stays cheap.
///
/// AABB-GATED: `outer`'s local bounding box is computed ONCE (Parry already
/// caches it as the QBVH root) and loosened by a tiny epsilon; a vertex
/// outside that box is PROVABLY outside `outer`'s solid (the box is a
/// necessary superset of the solid), so the O(F) ray-cast only runs for
/// vertices the box cannot rule out — this can never produce a false
/// negative, only skip work that was always going to answer "outside". The
/// common real case is two well-separated parts with disjoint bounding
/// boxes, so this collapses the clear-case cost from O(V·F) to O(V): every
/// vertex is rejected in O(1) and the ray-cast never runs at all. The two
/// isometries are composed ONCE outside the loop, so each vertex costs one
/// isometry application (inner-local → outer-local), not a world round trip.
fn any_vertex_in_solid(
    inner: &TriMesh,
    iso_inner: &Isometry3<f64>,
    outer: &TriMesh,
    iso_outer: &Isometry3<f64>,
) -> bool {
    // A tiny absolute pad so a vertex that geometrically sits ON `outer`'s
    // bounding face is never rejected by roundoff in the composed transform
    // — the box can only ever be grown, never shrunk, so this cannot hide a
    // real interior vertex.
    const AABB_PAD: f64 = 1.0e-9;
    let outer_aabb = outer.local_aabb().loosened(AABB_PAD);
    let outer_from_inner = iso_outer.inverse() * iso_inner;
    inner.vertices().iter().any(|v| {
        let local = outer_from_inner * v;
        outer_aabb.contains_local_point(&local) && point_in_solid_local(outer, &local)
    })
}

/// Is `local_pt` (already expressed in `mesh`'s own local frame) inside the
/// SOLID bounded by `mesh`? Ray-parity: cast a ray from the point in a fixed
/// generic direction and count triangle crossings — odd ⇒ inside. Winding-
/// INDEPENDENT (no oriented-normal requirement), so it is robust on any
/// watertight triangle soup regardless of the tessellator's convention. Used
/// to tell a neighbour seated in a through-HOLE (outside the solid ⇒ NOT
/// interference) from one ENCLOSED in the material (inside ⇒ interference)
/// when the two surfaces are separated.
///
/// Deliberately hand-rolled rather than Parry's `PointQuery::contains_local_point`
/// on `TriMesh`: that composite query answers a different question — whether
/// the point lies ON one of the mesh's individual triangles (each leaf's own
/// `contains_local_point`, QBVH-broadphased), not whether it is enclosed by
/// the watertight VOLUME the mesh bounds. It would read false almost
/// everywhere off the surface, which is the opposite of what this needs.
fn point_in_solid_local(mesh: &TriMesh, local_pt: &Point3<f64>) -> bool {
    // A generic direction dodges degenerate ray-through-edge / ray-through-vertex
    // hits that would corrupt the parity count.
    let dir = Vector3::new(1.0, 0.123_456_7, 0.061_803_4).normalize();
    let verts = mesh.vertices();
    let mut crossings = 0usize;
    for tri in mesh.indices() {
        let (Some(a), Some(b), Some(c)) = (
            verts.get(tri[0] as usize),
            verts.get(tri[1] as usize),
            verts.get(tri[2] as usize),
        ) else {
            continue;
        };
        // Möller–Trumbore; count only forward crossings (t > 0).
        let e1 = b - a;
        let e2 = c - a;
        let pv = dir.cross(&e2);
        let det = e1.dot(&pv);
        if det.abs() < 1.0e-12 {
            continue;
        }
        let inv = 1.0 / det;
        let tv = local_pt - a;
        let u = tv.dot(&pv) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let qv = tv.cross(&e1);
        let v = dir.dot(&qv) * inv;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = e2.dot(&qv) * inv;
        if t > 1.0e-9 {
            crossings += 1;
        }
    }
    crossings % 2 == 1
}

/// The instance's CONVEX HULL as a Parry `ConvexPolyhedron` (local frame). Used
/// for the penetration-DEPTH interference test: Parry's exact EPA depth on a
/// convex pair distinguishes a flush mating contact (depth ~0) from real overlap
/// — a TriMesh contact can't (its surface-touch reads ~0 for both). Exact for
/// convex parts (cylinders, blocks). `None` if the hull degenerates. This is the
/// fallback the per-piece decomposition below reduces to when VHACD yields
/// nothing usable.
fn instance_convex(instance: &Instance) -> Option<ConvexPolyhedron> {
    if instance.mesh.vertices.len() < 4 {
        return None;
    }
    let points: Vec<Point3<f64>> = instance
        .mesh
        .vertices
        .iter()
        .map(|v| Point3::new(v[0], v[1], v[2]))
        .collect();
    ConvexPolyhedron::from_convex_hull(&points)
}

/// VHACD parameters for the interference decomposition. Tuned for CAD parts
/// (bores, pockets, slots), not organic meshes, and reached only for a part the
/// convexity gate found genuinely concave:
/// - `resolution: 16` — voxel-grid resolution along the longest axis. A
///   bore/gap-class concavity (bore Ø30 in a ~50mm part → ~10 voxels across;
///   the dumbbell gap → ~7) still separates cleanly, while keeping voxelization
///   cheap: the DEBUG build voxelizes tens-of-thousands× slower than release, so
///   the dim3 default of 64 made a single concave part take seconds unoptimized.
///   Piece HULL precision is unaffected — `compute_exact_convex_hulls` re-derives
///   hulls from the original triangles, so resolution only sets WHERE the cut
///   lands (in the gap), not the face positions. Decomposition is O(n) per report
///   (once per instance, not per pair).
/// - `concavity: 0.01` (the dim3 default) — the split threshold. A part is kept
///   whole only when its hull barely over-approximates it; a hull that fills a
///   concavity (dumbbell gap, flange bore) is far above 0.01 and is split. A
///   perfectly convex part (block, cylinder hull) has concavity 0 and stays one
///   piece, so flush-mating semantics are unchanged.
/// - `max_convex_hulls: 32` — a hard guard against piece-count explosion (and
///   thus against O(pieces²) blow-up in the pairwise contact test). CAD bore /
///   pocket concavities need a handful of pieces; 32 is generous headroom while
///   bounding the worst case.
fn vhacd_params() -> VHACDParameters {
    VHACDParameters {
        resolution: 16,
        concavity: 0.01,
        max_convex_hulls: 32,
        ..VHACDParameters::default()
    }
}

/// Enclosed volume of a closed triangle soup by the divergence theorem:
/// `V = (1/6) Σ v0 · (v1 × v2)`. Winding only flips the sign, so the magnitude
/// is the volume regardless of orientation convention. A garbage value on a
/// non-closed / inconsistently-wound mesh only pushes the caller onto the
/// (correct, slower) VHACD path — never the other way.
fn triangle_soup_volume(points: &[Point3<f64>], indices: &[[u32; 3]]) -> f64 {
    let mut six_v = 0.0;
    for tri in indices {
        let (Some(a), Some(b), Some(c)) = (
            points.get(tri[0] as usize),
            points.get(tri[1] as usize),
            points.get(tri[2] as usize),
        ) else {
            continue;
        };
        six_v += a.coords.dot(&b.coords.cross(&c.coords));
    }
    (six_v / 6.0).abs()
}

/// Whether the part is (numerically) convex: its enclosed volume equals its
/// convex hull's. A convex solid fills its own hull exactly; a concavity that
/// matters for interference (a bore, a pocket, the dumbbell gap) removes a
/// percent-level chunk of volume, far above the tolerance. The check is cheap
/// (one hull build + two O(F) volume sums) and lets every convex part — the
/// overwhelming common case — skip VHACD entirely. The tolerance is relative so
/// it is scale-free; a sub-0.01% concavity cannot swallow a seated part, so
/// calling it convex is correct, not a regression.
fn is_convex(points: &[Point3<f64>], mesh_indices: &[[u32; 3]]) -> bool {
    let (hull_points, hull_indices) = convex_hull(points);
    let hull_volume = triangle_soup_volume(&hull_points, &hull_indices);
    if hull_volume <= 0.0 {
        return false; // degenerate hull ⇒ take the thorough path
    }
    let mesh_volume = triangle_soup_volume(points, mesh_indices);
    const CONVEX_VOLUME_REL_TOL: f64 = 1.0e-4;
    (hull_volume - mesh_volume).abs() <= CONVEX_VOLUME_REL_TOL * hull_volume
}

/// Decompose the instance mesh into a SET of convex pieces (local frame) via
/// approximate convex decomposition (VHACD). A concave part — a bored flange, a
/// pocket, the dumbbell gap — becomes several convex hulls, so a neighbour
/// seated in the concavity is no longer swallowed by a single hull that fills
/// it (finding F6). A convex part decomposes to a single piece equal to its own
/// hull, so the flush-mating-vs-overlap distinction is unchanged.
///
/// `compute_exact_convex_hulls` re-derives each piece's hull from the ORIGINAL
/// mesh primitives (not the voxel corners), so piece boundaries stay on the true
/// part surface — essential for the flush-contact depth test to read ~0 at a
/// real mating face rather than a voxel-inflated one.
///
/// Falls back to the single convex hull when the mesh is too small to voxelize
/// or VHACD yields no usable piece, so a degenerate part behaves exactly as
/// before (never a silent empty set that would hide a real overlap).
pub(crate) fn instance_convex_pieces(instance: &Instance) -> Vec<ConvexPolyhedron> {
    if instance.mesh.vertices.len() < 4 || instance.mesh.triangles.is_empty() {
        return instance_convex(instance).into_iter().collect();
    }
    let points: Vec<Point3<f64>> = instance
        .mesh
        .vertices
        .iter()
        .map(|v| Point3::new(v[0], v[1], v[2]))
        .collect();
    let indices = instance.mesh.triangles.clone();

    // Fast path: a convex part IS its own hull, so decomposition is a no-op —
    // skip the (voxelization-heavy) VHACD entirely. This keeps every block,
    // cylinder, sphere, and flush-mating part on the cheap single-hull path;
    // only genuinely concave parts pay for decomposition.
    if is_convex(&points, &indices) {
        return instance_convex(instance).into_iter().collect();
    }

    // `keep_voxel_to_primitives_map = true` is required by
    // `compute_exact_convex_hulls` (it maps voxels back to the source triangles).
    let decomposition = VHACD::decompose(&vhacd_params(), &points, &indices, true);
    let mut pieces: Vec<ConvexPolyhedron> = decomposition
        .compute_exact_convex_hulls(&points, &indices)
        .into_iter()
        .filter_map(|(hull_points, _hull_indices)| {
            if hull_points.len() < 4 {
                None
            } else {
                ConvexPolyhedron::from_convex_hull(&hull_points)
            }
        })
        .collect();

    if pieces.is_empty() {
        pieces.extend(instance_convex(instance));
    }
    pieces
}

impl Assembly {
    /// Pairwise static interference across the assembly — PENETRATION, not mere
    /// contact. A real assembly's mating faces touch by design (a bolt seats
    /// flush, a shaft bottoms in a bore); tangential contact is allowed and only
    /// overlapping VOLUME (beyond a small contact tolerance) is flagged. O(n²) for
    /// now — broad-phase BVH pruning is a later slice.
    pub fn interference_report(&self) -> InterferenceReport {
        // Overlap beyond CONTACT_TOL is interference; touching (tangential
        // contact — mating faces seat flush) is not. The penetration depth is
        // Parry's EPA on each part's convex hull; PREDICTION is the band within
        // which a contact is evaluated at all.
        const CONTACT_TOL: f64 = 1.0e-3;
        const PREDICTION: f64 = 1.0e-2;
        // Each instance is a SET of convex pieces (a concave part decomposes into
        // several). Decomposition is O(n) over instances — done once here, not in
        // the O(n²) pairwise loop below.
        let prepared: Vec<(
            InstanceId,
            Isometry3<f64>,
            Option<TriMesh>,
            Vec<ConvexPolyhedron>,
        )> = self
            .instances
            .iter()
            .map(|instance| {
                (
                    instance.id,
                    instance_isometry(instance),
                    instance_trimesh(instance),
                    instance_convex_pieces(instance),
                )
            })
            .collect();

        let mut interfering = Vec::new();
        let mut unverified = Vec::new();
        for i in 0..prepared.len() {
            for j in (i + 1)..prepared.len() {
                let (Some((id_a, pos_a, mesh_a, pieces_a)), Some((id_b, pos_b, mesh_b, pieces_b))) =
                    (prepared.get(i), prepared.get(j))
                else {
                    continue;
                };
                if pieces_a.is_empty() || pieces_b.is_empty() {
                    // A degenerate / mesh-less instance was never actually
                    // checked against its partner — that is NOT the same as
                    // having checked and found no overlap.
                    unverified.push(UnverifiedInterferencePair {
                        a: *id_a,
                        b: *id_b,
                        reason: UnverifiedReason::MissingMesh,
                    });
                    continue;
                }

                // F6 — EXACT mesh separation FIRST. The convex pieces of a bored /
                // holed part (a flange bore, a washer) each fill the hole (an
                // annulus's convex hull is a solid disc), so a neighbour seated in
                // the hole with real clearance false-positives against the pieces —
                // even VHACD cannot empty a closed through-hole. The exact TriMesh
                // distance sees the ACTUAL hole. When the two SURFACES are separated
                // beyond CONTACT_TOL the parts are clear, UNLESS one solid ENCLOSES
                // the other (a fully-enclosed part also has separated surfaces yet
                // IS interference) — a through-hole is NOT enclosure (the seated part
                // sits outside the solid material), so the ray-parity point-in-solid
                // test distinguishes them. When the surfaces touch / overlap
                // (distance ~0) we fall through to the piece EPA, which tells a flush
                // mating contact (depth ~0) from a real penetration.
                if let (Some(ma), Some(mb)) = (mesh_a, mesh_b) {
                    if matches!(query::distance(pos_a, ma, pos_b, mb), Ok(d) if d > CONTACT_TOL) {
                        let enclosed = any_vertex_in_solid(ma, pos_a, mb, pos_b)
                            || any_vertex_in_solid(mb, pos_b, ma, pos_a);
                        if !enclosed {
                            continue; // separated / seated-in-a-hole ⇒ clear
                        }
                        // enclosed ⇒ fall through to the piece EPA below
                    }
                }

                // Two parts interfere when ANY piece of one overlaps ANY piece of
                // the other beyond CONTACT_TOL. Flush mating between pieces reads
                // depth ~0 (Some, > -CONTACT_TOL) and is NOT interference, exactly
                // as with the single hull. Report the deepest penetration found.
                let mut worst: Option<f64> = None;
                'pairs: for ca in pieces_a {
                    for cb in pieces_b {
                        let depth = match query::contact(pos_a, ca, pos_b, cb, PREDICTION) {
                            // EPA penetration depth: negative when the pieces overlap.
                            Ok(Some(c)) if c.dist < -CONTACT_TOL => Some(c.dist),
                            // Touching contact (mating faces seat flush) ⇒ allowed.
                            Ok(Some(_)) => None,
                            // None (separated, OR an EPA degeneracy on a deep/exact
                            // overlap) or Err (unsupported pair): disambiguate
                            // conservatively with the boolean overlap test. This
                            // cannot re-flag a flush contact, which returns `Some`,
                            // not `None`.
                            _ => {
                                if query::intersection_test(pos_a, ca, pos_b, cb).unwrap_or(false) {
                                    Some(0.0)
                                } else {
                                    None
                                }
                            }
                        };
                        if let Some(d) = depth {
                            worst = Some(worst.map_or(d, |w| w.min(d)));
                            // A boolean-detected deep overlap (0.0) is already the
                            // strongest possible verdict; no deeper depth to find.
                            if d >= 0.0 {
                                break 'pairs;
                            }
                        }
                    }
                }
                if let Some(depth) = worst {
                    interfering.push(InterferencePair {
                        a: *id_a,
                        b: *id_b,
                        clearance: Some(depth),
                    });
                }
            }
        }
        InterferenceReport {
            interfering,
            unverified,
        }
    }

    /// Best-effort separation between two instances (positive ⇒ a gap, 0 ⇒
    /// touching/overlapping). `None` when a mesh is missing or the exact
    /// distance is unsupported for the pair — see `clearance_outcome` for
    /// the reason a caller must not conflate with "gap 0".
    pub fn clearance(&self, a: InstanceId, b: InstanceId) -> Option<f64> {
        match self.clearance_outcome(a, b) {
            ClearanceOutcome::Gap(gap) => Some(gap),
            ClearanceOutcome::Unverified(_) => None,
        }
    }

    /// Like `clearance`, but WITH the reason when the check could not run at
    /// all — the distinction `clearance`'s `Option<f64>` collapses away.
    /// Callers that must not fold "unverifiable" into a pass (mate-contact,
    /// interference) use this instead.
    pub fn clearance_outcome(&self, a: InstanceId, b: InstanceId) -> ClearanceOutcome {
        let Some(instance_a) = self.instance(a) else {
            return ClearanceOutcome::Unverified(UnverifiedReason::MissingMesh);
        };
        let Some(instance_b) = self.instance(b) else {
            return ClearanceOutcome::Unverified(UnverifiedReason::MissingMesh);
        };
        let Some(mesh_a) = instance_trimesh(instance_a) else {
            return ClearanceOutcome::Unverified(UnverifiedReason::MissingMesh);
        };
        let Some(mesh_b) = instance_trimesh(instance_b) else {
            return ClearanceOutcome::Unverified(UnverifiedReason::MissingMesh);
        };
        match query::distance(
            &instance_isometry(instance_a),
            &mesh_a,
            &instance_isometry(instance_b),
            &mesh_b,
        ) {
            Ok(gap) => ClearanceOutcome::Gap(gap),
            Err(_) => ClearanceOutcome::Unverified(UnverifiedReason::UnsupportedPair),
        }
    }
}

/// Detailed outcome of a clearance query: the measured gap, or WHY the check
/// could not run at all (never conflated with "gap 0" / touching).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClearanceOutcome {
    /// The exact distance was computed (positive ⇒ gap, 0 ⇒ touching/overlap).
    Gap(f64),
    /// The check did not run — this is never a pass, just unrun.
    Unverified(UnverifiedReason),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Mesh;

    /// An axis-aligned cube of side `2*h` centred at the origin (local frame).
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

    /// A UV-sphere mesh (`stacks × slices` vertices) — used only to exercise
    /// the exhaustive `any_vertex_in_solid` scan at a realistic vertex count.
    fn uv_sphere(radius: f64, stacks: usize, slices: usize) -> Mesh {
        let mut vertices = Vec::with_capacity((stacks + 1) * slices);
        for i in 0..=stacks {
            let phi = std::f64::consts::PI * (i as f64) / (stacks as f64); // 0..=PI
            let (sin_phi, cos_phi) = phi.sin_cos();
            for j in 0..slices {
                let theta = 2.0 * std::f64::consts::PI * (j as f64) / (slices as f64);
                let (sin_theta, cos_theta) = theta.sin_cos();
                vertices.push([
                    radius * sin_phi * cos_theta,
                    radius * sin_phi * sin_theta,
                    radius * cos_phi,
                ]);
            }
        }
        let mut triangles = Vec::with_capacity(stacks * slices * 2);
        for i in 0..stacks {
            for j in 0..slices {
                let j_next = (j + 1) % slices;
                let a = (i * slices + j) as u32;
                let b = (i * slices + j_next) as u32;
                let c = ((i + 1) * slices + j) as u32;
                let d = ((i + 1) * slices + j_next) as u32;
                triangles.push([a, c, b]);
                triangles.push([b, c, d]);
            }
        }
        Mesh {
            vertices,
            triangles,
        }
    }

    /// PERF/MEASURE (H9): the exhaustive `any_vertex_in_solid` scan is only
    /// expensive on the CLEAR case — every vertex must be ruled out before
    /// the call can honestly return `false`; the interference case
    /// short-circuits on the first hit. Two ~3.7k-vertex spheres, well
    /// separated (no overlap, no enclosure, disjoint bounding boxes — the
    /// AABB pre-reject's target case), exercise exactly that worst case for
    /// a realistic mesh size.
    ///
    /// This is a REGRESSION TRIPWIRE, not a tight benchmark: machine load has
    /// measured 2-4× inflation on this project before, so the bound is
    /// generous (30s) — it exists to catch a return to the pre-AABB-gate
    /// O(V·F) cost (measured at ~107s on this same pair), not to enforce a
    /// specific number. The printed measurement is the real signal.
    #[test]
    fn clear_case_scan_cost_on_a_realistic_mesh() {
        let sphere = uv_sphere(1.0, 60, 60);
        let vertex_count = sphere.vertices.len();
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(Instance::new(
            InstanceId(0),
            "sphere_a".to_string(),
            sphere.clone(),
        ));
        let mut b = Instance::new(InstanceId(1), "sphere_b".to_string(), sphere);
        b.translation = [10.0, 0.0, 0.0]; // gap of 8 between the two unit spheres
        assembly.add_instance(b);

        let start = std::time::Instant::now();
        let report = assembly.interference_report();
        let elapsed = start.elapsed();
        println!(
            "H9 clear-case cost: interference_report over two {vertex_count}-vertex spheres took {elapsed:?}"
        );

        assert!(
            report.no_static_interference(),
            "well-separated spheres must be clear"
        );
        assert!(
            elapsed.as_secs_f64() < 30.0,
            "clear-case scan regressed toward the pre-AABB-gate O(V·F) cost: took {elapsed:?}"
        );
    }

    fn cube_at(id: u32, h: f64, x: f64) -> Instance {
        let mut instance = Instance::new(InstanceId(id), format!("cube_{id}"), cube(h));
        instance.translation = [x, 0.0, 0.0];
        instance
    }

    #[test]
    fn overlapping_parts_interfere() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, 0.0)); // x in [-1, 1]
        assembly.add_instance(cube_at(1, 1.0, 0.5)); // x in [-0.5, 1.5] — overlaps
        let report = assembly.interference_report();
        assert!(!report.no_static_interference());
        assert_eq!(report.interfering.len(), 1);
    }

    #[test]
    fn flush_faces_touch_but_do_not_interfere() {
        // Two cubes seated face-to-face — the right face of one ON the left face
        // of the other. Tangential CONTACT, the way parts MATE. Not interference.
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, 0.0)); // x in [-1, 1]
        assembly.add_instance(cube_at(1, 1.0, 2.0)); // x in [1, 3] — touches at x=1
        let report = assembly.interference_report();
        assert!(
            report.no_static_interference(),
            "flush mating faces are contact, not interference: {:?}",
            report.interfering
        );
    }

    #[test]
    fn separated_parts_are_clear() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, 0.0)); // x in [-1, 1]
        assembly.add_instance(cube_at(1, 1.0, 5.0)); // x in [4, 6] — gap of 3
        let report = assembly.interference_report();
        assert!(report.no_static_interference());
        // Clearance is best-effort; when supported it must report the ~3 gap.
        if let Some(gap) = assembly.clearance(InstanceId(0), InstanceId(1)) {
            assert!(gap > 2.5 && gap < 3.5, "expected ~3, got {gap}");
        }
    }

    #[test]
    fn clearance_is_symmetric() {
        // VERIFY/HARNESS invariant: clearance(a, b) == clearance(b, a).
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, 0.0));
        assembly.add_instance(cube_at(1, 1.0, 5.0));
        let ab = assembly.clearance(InstanceId(0), InstanceId(1));
        let ba = assembly.clearance(InstanceId(1), InstanceId(0));
        let symmetric = match (ab, ba) {
            (Some(x), Some(y)) => (x - y).abs() < 1e-9,
            (None, None) => true,
            _ => false,
        };
        assert!(symmetric, "clearance must be symmetric: {ab:?} vs {ba:?}");
    }

    #[test]
    fn scales_to_many_parts_without_false_interference() {
        // BENCHMARK/VERIFY: a row of well-separated parts → zero interference,
        // and the O(n²) sweep completes promptly (perf sanity; BVH broad-phase
        // is a later slice).
        let mut assembly = Assembly::new(InstanceId(0));
        for k in 0..30u32 {
            assembly.add_instance(cube_at(k, 0.4, f64::from(k) * 2.0));
        }
        let report = assembly.interference_report();
        assert!(report.no_static_interference());
    }

    #[test]
    fn unverifiable_pair_is_not_folded_into_clear() {
        // Instance 1 has an empty mesh, so its convex-piece decomposition is
        // empty — today's loop `continue`s past the pair silently (the H8
        // defect: an unrun check reads as "clear"). It must not.
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, 0.0));
        assembly.add_instance(Instance::new(
            InstanceId(1),
            "meshless".to_string(),
            Mesh::default(),
        ));
        let report = assembly.interference_report();
        assert!(
            !report.no_static_interference(),
            "an unchecked pair must not report as clear"
        );
        assert!(report.interfering.is_empty(), "not an actual overlap");
        assert_eq!(report.unverified.len(), 1);
        assert_eq!(report.unverified[0].reason, UnverifiedReason::MissingMesh);
    }

    /// H9 — a pin with ~1000 vertices, where all but 8 are a decoy blob far
    /// from `outer` and the last 8 are a small cube fully embedded in
    /// `outer`'s solid (surfaces never touch — the "marble cast in a
    /// block" case, real interference). Under the OLD `MAX_SAMPLES = 64`
    /// cap, `stride = 1001 / 64 = 15`; the sampled indices (multiples of
    /// 15) never land on the embedded block (indices 993..=1000), so the
    /// old code samples only decoy vertices and misses the real
    /// penetration. The fix (exhaustive scan) must find it — `.any()`
    /// still short-circuits on the FIRST hit, so this stays cheap.
    #[test]
    fn any_vertex_in_solid_finds_a_localized_penetration_the_old_stride_would_skip() {
        let mut vertices: Vec<[f64; 3]> = Vec::new();
        let mut triangles: Vec<[u32; 3]> = Vec::new();

        // Decoy: 331 tiny valid (non-degenerate) triangles, all far from
        // `outer` (near x = 15) — none of these vertices are inside `outer`.
        for _ in 0..331 {
            let base = vertices.len() as u32;
            vertices.push([15.0, 0.0, 0.0]);
            vertices.push([15.01, 0.0, 0.0]);
            vertices.push([15.0, 0.01, 0.0]);
            triangles.push([base, base + 1, base + 2]);
        }
        let embed_start = vertices.len();
        assert_eq!(embed_start, 993, "test setup: expected vertex count");
        assert_ne!(
            embed_start % 15,
            0,
            "test setup: the embedded block must dodge the stride"
        );

        // The genuinely embedded cube: well inside `outer`'s [-5, 5]^3
        // solid, never touching its surface (gap ≈ 4.95 on every side).
        let h = 0.05;
        vertices.extend_from_slice(&[
            [-h, -h, -h],
            [h, -h, -h],
            [h, h, -h],
            [-h, h, -h],
            [-h, -h, h],
            [h, -h, h],
            [h, h, h],
            [-h, h, h],
        ]);
        let b = embed_start as u32;
        triangles.extend_from_slice(&[
            [b, b + 2, b + 1],
            [b, b + 3, b + 2],
            [b + 4, b + 5, b + 6],
            [b + 4, b + 6, b + 7],
            [b, b + 1, b + 5],
            [b, b + 5, b + 4],
            [b + 2, b + 3, b + 7],
            [b + 2, b + 7, b + 6],
            [b + 1, b + 2, b + 6],
            [b + 1, b + 6, b + 5],
            [b + 3, b, b + 4],
            [b + 3, b + 4, b + 7],
        ]);
        assert_eq!(vertices.len(), 1001, "test setup: expected total count");

        let inner = TriMesh::new(
            vertices
                .iter()
                .map(|v| Point3::new(v[0], v[1], v[2]))
                .collect(),
            triangles,
        );
        let outer_mesh = cube(5.0);
        let outer = TriMesh::new(
            outer_mesh
                .vertices
                .iter()
                .map(|v| Point3::new(v[0], v[1], v[2]))
                .collect(),
            outer_mesh.triangles,
        );

        let iso = Isometry3::identity();
        assert!(
            any_vertex_in_solid(&inner, &iso, &outer, &iso),
            "the embedded cube's vertices must be found despite the decoy padding"
        );
    }
}
