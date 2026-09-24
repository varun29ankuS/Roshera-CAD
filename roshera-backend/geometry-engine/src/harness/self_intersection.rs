//! Mesh self-intersection oracle (PILLAR 1 certificate gap + PILLAR 2 invariant).
//!
//! A B-Rep can be topologically valid AND watertight yet be geometrically
//! SELF-OVERLAPPING — two faces that aren't topological neighbours pass through
//! each other (e.g. a chamfer cut across an existing fillet, #70, or a loft that
//! folds back on itself). No other kernel check catches this. Here we detect it
//! on the tessellated mesh: any pair of triangles that do NOT share a (welded)
//! vertex and whose interiors cross is a self-intersection.
//!
//! The cross test is segment-vs-triangle (Möller–Trumbore) over all six edges of
//! the two triangles — two surface triangles intersect iff an edge of one
//! pierces the interior of the other. Strict interior tolerances exclude the
//! legitimate edge/vertex contact of adjacent faces (which we also skip by
//! shared-vertex pruning), so a clean watertight solid reports `false`.

use crate::math::{Point3, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::edge_cache::EdgeSampleCache;
use crate::tessellation::mesh::TriangleMesh;
use crate::tessellation::{tessellate_solid, TessellationParams};
use std::collections::HashMap;

const EPS: f64 = 1.0e-9;

/// Does the segment `p0→p1` pierce the INTERIOR of triangle `abc`? Strictly
/// interior in both the barycentric coords and the segment parameter, so shared
/// edges/vertices (touch, not cross) return `false`.
fn segment_pierces_triangle(p0: Point3, p1: Point3, a: Point3, b: Point3, c: Point3) -> bool {
    let dir: Vector3 = p1 - p0;
    let e1: Vector3 = b - a;
    let e2: Vector3 = c - a;
    let pvec = dir.cross(&e2);
    let det = e1.dot(&pvec);
    if det.abs() < EPS {
        return false; // segment parallel to the triangle plane
    }
    let inv = 1.0 / det;
    let tvec: Vector3 = p0 - a;
    let u = tvec.dot(&pvec) * inv;
    if u <= EPS || u >= 1.0 - EPS {
        return false;
    }
    let qvec = tvec.cross(&e1);
    let v = dir.dot(&qvec) * inv;
    if v <= EPS || u + v >= 1.0 - EPS {
        return false;
    }
    let t = e2.dot(&qvec) * inv;
    // Strictly inside the segment → a genuine crossing, not an endpoint touch.
    t > EPS && t < 1.0 - EPS
}

/// Do two triangles cross each other's interior? (six segment-triangle tests).
pub fn triangles_intersect(a: [Point3; 3], b: [Point3; 3]) -> bool {
    for k in 0..3 {
        let (p0, p1) = (a[k], a[(k + 1) % 3]);
        if segment_pierces_triangle(p0, p1, b[0], b[1], b[2]) {
            return true;
        }
    }
    for k in 0..3 {
        let (p0, p1) = (b[k], b[(k + 1) % 3]);
        if segment_pierces_triangle(p0, p1, a[0], a[1], a[2]) {
            return true;
        }
    }
    false
}

/// Axis-aligned bounds of a triangle (for broad-phase pruning).
fn tri_aabb(t: &[Point3; 3]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in t {
        let c = [p.x, p.y, p.z];
        for i in 0..3 {
            lo[i] = lo[i].min(c[i]);
            hi[i] = hi[i].max(c[i]);
        }
    }
    (lo, hi)
}
fn aabb_disjoint(a: &([f64; 3], [f64; 3]), b: &([f64; 3], [f64; 3])) -> bool {
    for i in 0..3 {
        if a.1[i] < b.0[i] - EPS || b.1[i] < a.0[i] - EPS {
            return true;
        }
    }
    false
}

/// Test whether a pre-built [`TriangleMesh`] self-intersects — the shared core
/// of [`mesh_self_intersects`] (which tessellates first) and any caller that
/// already holds a mesh (e.g. the injected-defect benchmark that mutates a
/// tessellated mesh and re-certifies it without touching the B-Rep).
///
/// Uses the same uniform spatial-hash broad-phase and segment/triangle
/// narrow-phase as [`mesh_self_intersects`]. Pairs sharing a welded vertex
/// (topological neighbours) are excluded — they touch, not cross. Returns
/// `false` for meshes with fewer than 2 triangles.
pub fn mesh_self_intersects_mesh(mesh: &TriangleMesh) -> bool {
    !crossing_pairs(mesh, 1).is_empty()
}

/// The crossing triangle pairs of `mesh` (indices into `mesh.triangles`),
/// stopping once `cap` have been found. The analysis behind
/// [`mesh_self_intersects_mesh`].
fn crossing_pairs(mesh: &TriangleMesh, cap: usize) -> Vec<(usize, usize)> {
    let mut found: Vec<(usize, usize)> = Vec::new();
    let mut seen: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    if mesh.triangles.len() < 2 || cap == 0 {
        return found;
    }

    // Weld vertices by quantised position so adjacent triangles share canonical
    // indices (and are skipped — they touch, not cross).
    //
    // Slice 5 (tolerance authority): the grid spacing is
    // `authority::SELF_INTERSECTION_WELD_GRID` (= 10·τ_weld = 1e-5) — coarser
    // than the vertex weld so a pair the kernel welds can never read as a
    // self-touch here, and derived from the same source scale instead of the
    // former free-standing `Q = 1e5` reciprocal.
    const Q: f64 = 1.0 / crate::math::tolerance::authority::SELF_INTERSECTION_WELD_GRID;
    let key = |p: &Point3| -> (i64, i64, i64) {
        (
            (p.x * Q).round() as i64,
            (p.y * Q).round() as i64,
            (p.z * Q).round() as i64,
        )
    };
    let mut canon: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut next = 0u32;
    let welded: Vec<u32> = mesh
        .vertices
        .iter()
        .map(|v| {
            *canon.entry(key(&v.position)).or_insert_with(|| {
                let id = next;
                next += 1;
                id
            })
        })
        .collect();

    let tris: Vec<[Point3; 3]> = mesh
        .triangles
        .iter()
        .map(|t| {
            [
                mesh.vertices[t[0] as usize].position,
                mesh.vertices[t[1] as usize].position,
                mesh.vertices[t[2] as usize].position,
            ]
        })
        .collect();
    let wtri: Vec<[u32; 3]> = mesh
        .triangles
        .iter()
        .map(|t| {
            [
                welded[t[0] as usize],
                welded[t[1] as usize],
                welded[t[2] as usize],
            ]
        })
        .collect();
    let aabbs: Vec<([f64; 3], [f64; 3])> = tris.iter().map(tri_aabb).collect();

    let n = tris.len();

    // BROAD PHASE — uniform spatial-hash grid. The legacy all-pairs loop was
    // O(n²); a curved-Boolean part tessellates to tens of thousands of
    // triangles, so that scan dominated the per-op certificate cost (and was
    // the reason the full cert was pulled off the hot path). Binning each
    // triangle into the grid cells its AABB overlaps and testing only
    // same-cell pairs makes the broad phase ~O(n) on the spatially-coherent
    // meshes a solid produces, while remaining EXACT — the grid only prunes
    // pairs whose AABBs cannot overlap, never a real crossing (a genuine
    // self-intersection forces overlapping AABBs, hence a shared cell).
    //
    // Cell size = mean triangle-AABB diagonal (a standard sizing heuristic):
    // big enough that a triangle spans only a handful of cells, small enough
    // that few unrelated triangles share one. Degenerate (zero-size) meshes
    // fall back to a single cell, recovering the all-pairs scan safely.
    let mut diag_sum = 0.0f64;
    let mut scene_lo = [f64::INFINITY; 3];
    let mut scene_hi = [f64::NEG_INFINITY; 3];
    for (lo, hi) in &aabbs {
        let mut d2 = 0.0;
        for i in 0..3 {
            let e = hi[i] - lo[i];
            d2 += e * e;
            scene_lo[i] = scene_lo[i].min(lo[i]);
            scene_hi[i] = scene_hi[i].max(hi[i]);
        }
        diag_sum += d2.sqrt();
    }
    let mut cell = diag_sum / n as f64;
    if !(cell.is_finite() && cell > 0.0) {
        // All-degenerate mesh (no extent): one bucket, exact all-pairs.
        let mut max_ext = 0.0f64;
        for i in 0..3 {
            max_ext = max_ext.max(scene_hi[i] - scene_lo[i]);
        }
        cell = if max_ext.is_finite() && max_ext > 0.0 {
            max_ext
        } else {
            1.0
        };
    }
    let inv_cell = 1.0 / cell;
    let cell_of = |c: f64, origin: f64| -> i64 { ((c - origin) * inv_cell).floor() as i64 };

    // Map cell → triangle indices. A triangle is inserted into every cell its
    // AABB spans; the per-pair shared-welded-vertex and exact AABB tests below
    // reject the false candidates a coarse cell admits, so correctness is
    // independent of the cell size — only speed depends on it.
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    for (idx, (lo, hi)) in aabbs.iter().enumerate() {
        let (cx0, cy0, cz0) = (
            cell_of(lo[0], scene_lo[0]),
            cell_of(lo[1], scene_lo[1]),
            cell_of(lo[2], scene_lo[2]),
        );
        let (cx1, cy1, cz1) = (
            cell_of(hi[0], scene_lo[0]),
            cell_of(hi[1], scene_lo[1]),
            cell_of(hi[2], scene_lo[2]),
        );
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                for cz in cz0..=cz1 {
                    grid.entry((cx, cy, cz)).or_default().push(idx as u32);
                }
            }
        }
    }

    // NARROW PHASE — only triangles co-located in a cell are candidates. A pair
    // whose AABBs span multiple shared cells may be revisited in more than one
    // bucket; the triangle/triangle test is symmetric, idempotent, and returns
    // immediately on a hit, so a redundant test of a NON-crossing pair only
    // costs a handful of flops (cheaper than maintaining a visited-set). The
    // exact per-pair AABB-disjoint test below is what guarantees the grid never
    // turns a true non-crossing into a false positive; the grid only ever
    // PRUNES pairs that cannot overlap.
    let _ = n; // `n` is the (now-bypassed) all-pairs bound; kept for clarity above.
    for bucket in grid.values() {
        let m = bucket.len();
        for a in 0..m {
            let i = bucket[a] as usize;
            for &jb in bucket.iter().skip(a + 1) {
                let j = jb as usize;
                // Skip topological neighbours (any shared welded vertex).
                let wi = wtri[i];
                let wj = wtri[j];
                if wi.iter().any(|x| wj.contains(x)) {
                    continue;
                }
                if aabb_disjoint(&aabbs[i], &aabbs[j]) {
                    continue;
                }
                if triangles_intersect(tris[i], tris[j]) {
                    let pair = (i.min(j), i.max(j));
                    if seen.insert(pair) {
                        found.push(pair);
                        if found.len() >= cap {
                            return found;
                        }
                    }
                }
            }
        }
    }
    found
}

/// `true` if the solid's tessellated mesh self-intersects at chord `chord`.
/// Pairs sharing a welded vertex (topological neighbours) are skipped; a
/// UNIFORM SPATIAL-HASH GRID over the triangle AABBs supplies broad-phase
/// candidate pairs so the narrow-phase (segment/triangle) test runs only on
/// triangles that actually share a grid cell — turning the historical O(n²)
/// all-pairs scan into ~O(n) for the spatially-coherent meshes a CAD solid
/// produces. Use a COARSE chord for a fast certificate check — self-overlap is
/// a gross geometric fault, visible at low density.
///
/// A coarse crossing is then CONFIRMED on the faces that produced it
/// ([`confirm_crossings`]) before it is reported, so a chord artifact between
/// two disjoint surfaces closer than their facets' sag is not called a
/// self-intersection — and nothing else can overturn the coarse verdict.
pub fn mesh_self_intersects(model: &BRepModel, solid: SolidId, chord: f64) -> bool {
    let solid_ref = match model.solids.get(solid) {
        Some(s) => s,
        None => return false,
    };
    // AUDIT-bounded tessellation: this is a coarse self-overlap certificate, and
    // the pair scan below is O(n²) in the triangle count. `default()`'s 100-
    // segment ceiling lets a curved-Boolean arrangement-cell fragment (whose rim
    // can sample thousands of points) explode the triangle count, making the
    // quadratic scan run for tens of seconds — it was the dominant cost of the
    // per-op certificate audit. `audit()` caps segments at 24 (and the spherical-
    // fan budget scales with it), bounding `n` so the scan is fast. Gross self-
    // overlap is visible at this density (the check's stated contract), so the
    // coarser mesh does not blind it. The caller's `chord` stays the quality knob.
    let params = TessellationParams {
        chord_tolerance: chord,
        ..TessellationParams::audit()
    };
    let mesh = tessellate_solid(solid_ref, model, &params);
    let pairs = crossing_pairs(&mesh, CONFIRM_PAIR_CAP);
    if pairs.is_empty() {
        return false;
    }
    let finer = TessellationParams {
        chord_tolerance: chord / 16.0,
        max_angle_deviation: params.max_angle_deviation / 4.0,
        max_segments: params.max_segments * 4,
        ..params
    };
    confirm_crossings(model, &mesh, &pairs, &finer, &|face_id, p, cache, out| {
        if let Some(face) = model.faces.get(face_id) {
            crate::tessellation::surface::tessellate_face(face, model, p, cache, out);
        }
    })
}

/// Most coarse crossing pairs collected for the confirmation step.
const CONFIRM_PAIR_CAP: usize = 4096;
/// Most distinct faces a confirmation re-tessellates; a crossing spread over
/// more faces is kept as reported.
const CONFIRM_FACE_BUDGET: usize = 16;
/// Most triangles the finer re-tessellation of the involved faces may yield;
/// over budget the coarse verdict is kept.
const CONFIRM_TRIANGLE_BUDGET: usize = 200_000;

/// Re-test a coarse "self-intersecting" verdict on EXACTLY the faces whose
/// triangles crossed (`pairs`, indices into `coarse`), re-tessellated at the
/// finer `params` by `tessellate` (one face at a time into a shared mesh, so
/// their common edges share samples and weld). Returns the confirmed verdict.
///
/// A crossing between two disjoint surfaces closer to each other than their
/// facets' sagitta (a void 0.1 inside the curved wall of an R = 50 drum,
/// where a 24-segment wall sags about 0.43) is a CHORD ARTIFACT: at a finer
/// chord those faces no longer cross. A genuine self-intersection crosses at
/// every density. The coarse verdict is overturned ONLY when the finer mesh
/// of every involved face exists and those faces no longer cross; it stands
/// when the pair list reached [`CONFIRM_PAIR_CAP`] (crossings beyond it were
/// never examined), when the crossing involves more than
/// [`CONFIRM_FACE_BUDGET`] faces, when
/// the finer mesh exceeds [`CONFIRM_TRIANGLE_BUDGET`] triangles, when any
/// involved face is missing or re-tessellates to nothing, or when the finer
/// mesh still crosses.
fn confirm_crossings(
    model: &BRepModel,
    coarse: &TriangleMesh,
    pairs: &[(usize, usize)],
    params: &TessellationParams,
    tessellate: &dyn Fn(u32, &TessellationParams, &EdgeSampleCache, &mut TriangleMesh),
) -> bool {
    // A list that reached the collection cap is TRUNCATED: crossings beyond
    // it were never examined and their faces never re-tested, so clearing the
    // examined ones proves nothing about the rest. The coarse verdict stands.
    if pairs.len() >= CONFIRM_PAIR_CAP {
        return true;
    }
    let mut faces: Vec<u32> = Vec::new();
    for &(i, j) in pairs {
        for t in [i, j] {
            match coarse.face_map.get(t) {
                Some(&f) => {
                    if !faces.contains(&f) {
                        faces.push(f);
                    }
                }
                // A crossing triangle that cannot be attributed to a face
                // cannot be re-tested: the coarse verdict stands.
                None => return true,
            }
        }
        if faces.len() > CONFIRM_FACE_BUDGET {
            return true;
        }
    }
    faces.sort_unstable();

    let cache = EdgeSampleCache::new(params);
    let mut finer = TriangleMesh::new();
    for &face_id in &faces {
        if model.faces.get(face_id).is_none() {
            return true;
        }
        let before = finer.triangles.len();
        tessellate(face_id, params, &cache, &mut finer);
        let after = finer.triangles.len();
        if after == before {
            // The finer mesh lost a face the coarse crossing involved: it
            // cannot vouch for that face, so it cannot overturn the verdict.
            return true;
        }
        for _ in before..after {
            finer.face_map.push(face_id);
        }
        if after > CONFIRM_TRIANGLE_BUDGET {
            return true;
        }
    }
    mesh_self_intersects_mesh(&finer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Matrix4;
    use crate::operations::{transform_solid, TransformOptions};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    /// One solid whose two bodies GENUINELY overlap: box A (10³ at the origin)
    /// with box B (10³ centred at (5, 3.7, 2.3)) attached as a peer body. Returns
    /// the model, the solid and box B's face ids.
    fn overlapping_bodies() -> (BRepModel, SolidId, Vec<u32>) {
        let mut model = BRepModel::new();
        let mk = |m: &mut BRepModel| match TopologyBuilder::new(m).create_box_3d(10.0, 10.0, 10.0) {
            Ok(GeometryId::Solid(id)) => id,
            other => panic!("expected a solid, got {other:?}"),
        };
        let a = mk(&mut model);
        let b = mk(&mut model);
        transform_solid(
            &mut model,
            b,
            Matrix4::from_translation(&Vector3::new(5.0, 3.7, 2.3)),
            TransformOptions::default(),
        )
        .expect("place box B");
        let b_shell = model.solids.get(b).expect("solid b").outer_shell;
        let b_faces = model.shells.get(b_shell).expect("shell").faces.clone();
        model
            .solids
            .get_mut(a)
            .expect("solid a")
            .add_peer_shell(b_shell);
        (model, a, b_faces)
    }

    fn coarse_mesh_and_pairs(
        model: &BRepModel,
        id: SolidId,
    ) -> (TriangleMesh, Vec<(usize, usize)>) {
        let solid = model.solids.get(id).expect("solid");
        let params = TessellationParams {
            chord_tolerance: 0.5,
            ..TessellationParams::audit()
        };
        let mesh = tessellate_solid(solid, model, &params);
        let pairs = crossing_pairs(&mesh, CONFIRM_PAIR_CAP);
        (mesh, pairs)
    }

    /// A pair list that reached the collection cap may be truncated: the
    /// crossings beyond it were never examined. Even when every listed pair
    /// re-tests clean (here: triangles of a plain box, which cross nothing),
    /// the coarse verdict must stand.
    #[test]
    fn a_truncated_crossing_list_is_never_confirmed_away() {
        let mut model = BRepModel::new();
        let id = match TopologyBuilder::new(&mut model).create_box_3d(10.0, 10.0, 10.0) {
            Ok(GeometryId::Solid(id)) => id,
            other => panic!("expected a solid, got {other:?}"),
        };
        let solid = model.solids.get(id).expect("solid");
        let coarse = tessellate_solid(
            solid,
            &model,
            &TessellationParams {
                chord_tolerance: 0.5,
                ..TessellationParams::audit()
            },
        );
        assert!(coarse.triangles.len() >= 2, "fixture: a tessellated box");
        let finer = TessellationParams {
            chord_tolerance: 0.5 / 16.0,
            ..TessellationParams::audit()
        };
        let tess = |face_id: u32,
                    p: &TessellationParams,
                    cache: &EdgeSampleCache,
                    out: &mut TriangleMesh| {
            if let Some(face) = model.faces.get(face_id) {
                crate::tessellation::surface::tessellate_face(face, &model, p, cache, out);
            }
        };
        // Control: below the cap, clean faces DO overturn the verdict.
        assert!(
            !confirm_crossings(&model, &coarse, &[(0, 1)], &finer, &tess),
            "fixture: a box's faces re-test clean"
        );
        let at_cap = vec![(0usize, 1usize); CONFIRM_PAIR_CAP];
        assert!(
            confirm_crossings(&model, &coarse, &at_cap, &finer, &tess),
            "a crossing list at the cap is truncated and must not be confirmed away"
        );
    }

    #[test]
    fn a_genuine_overlap_survives_the_confirmation() {
        let (model, id, _) = overlapping_bodies();
        assert!(
            mesh_self_intersects(&model, id, 0.5),
            "two overlapping bodies self-intersect at every density"
        );
    }

    /// The confirmation may only overturn the coarse verdict with a finer mesh
    /// of EVERY face the coarse crossing involved. Here the finer tessellation
    /// of box B's faces is withheld (as a face the finer tessellator declines
    /// would be): the finer mesh then holds box A alone, crosses nothing, and
    /// must NOT be read as "self-intersection free".
    #[test]
    fn a_crossing_stands_when_the_finer_mesh_loses_an_involved_face() {
        let (model, id, b_faces) = overlapping_bodies();
        let (coarse, pairs) = coarse_mesh_and_pairs(&model, id);
        assert!(!pairs.is_empty(), "fixture: the coarse mesh crosses");
        let finer = TessellationParams {
            chord_tolerance: 0.5 / 16.0,
            ..TessellationParams::audit()
        };
        let verdict = confirm_crossings(
            &model,
            &coarse,
            &pairs,
            &finer,
            &|face_id, p, cache, out| {
                if b_faces.contains(&face_id) {
                    return;
                }
                if let Some(face) = model.faces.get(face_id) {
                    crate::tessellation::surface::tessellate_face(face, &model, p, cache, out);
                }
            },
        );
        assert!(
            verdict,
            "a crossing whose finer re-test lost an involved face must stand"
        );
    }

    #[test]
    fn detects_crossing_triangles_and_clears_disjoint() {
        // Two triangles that pierce each other (an X in 3D).
        let a = [
            Point3::new(-1.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
        ];
        let b = [
            Point3::new(0.0, -1.0, 0.3),
            Point3::new(0.0, 1.0, 0.3),
            Point3::new(0.0, 0.0, -1.0),
        ];
        assert!(
            triangles_intersect(a, b),
            "crossing triangles must intersect"
        );

        // Disjoint (far apart) → no intersection.
        let c = [
            Point3::new(10.0, 10.0, 10.0),
            Point3::new(11.0, 10.0, 10.0),
            Point3::new(10.0, 11.0, 10.0),
        ];
        assert!(
            !triangles_intersect(a, c),
            "far triangles must not intersect"
        );

        // Sharing an edge (touch, not cross) → no interior crossing.
        let d = [
            Point3::new(-1.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, -1.0),
        ];
        assert!(
            !triangles_intersect(a, d),
            "edge-sharing triangles do not self-intersect"
        );
    }

    /// NON-BLINDNESS AT AUDIT QUALITY: the segment ceiling was lowered to
    /// `TessellationParams::audit()` (24 segments) to bound this oracle's O(n²)
    /// pair scan. A clean curved solid must STILL report `false` (no false
    /// positive from the coarser mesh — adjacent faceting edges only touch, they
    /// don't cross), AND a sphere/cylinder tessellate to enough triangles that
    /// the scan is genuinely exercised. This proves the coarsening did not make
    /// the certificate's self-overlap check vacuous.
    #[test]
    fn clean_curved_solids_report_no_self_intersection_at_audit_quality() {
        use crate::math::vector3::Vector3;
        use crate::primitives::topology_builder::TopologyBuilder;

        let mut ms = BRepModel::new();
        TopologyBuilder::new(&mut ms)
            .create_sphere_3d(Vector3::ZERO, 5.0)
            .expect("sphere");
        let s = ms.solids.iter().last().map(|(id, _)| id).expect("sphere");
        assert!(
            !mesh_self_intersects(&ms, s, 0.5),
            "a clean sphere must not self-intersect at the audit chord"
        );

        let mut mc = BRepModel::new();
        TopologyBuilder::new(&mut mc)
            .create_cylinder_3d(Vector3::ZERO, Vector3::Z, 3.0, 8.0)
            .expect("cyl");
        let c = mc.solids.iter().last().map(|(id, _)| id).expect("cyl");
        assert!(
            !mesh_self_intersects(&mc, c, 0.5),
            "a clean cylinder must not self-intersect at the audit chord"
        );
    }
}
