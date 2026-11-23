//! Merging the faces a boolean leaves split on one plane.
//!
//! A boolean rebuilds its result from the operands' face fragments, so two
//! boxes joined into an L keep every fragment: the base's bottom and the
//! upright's bottom as two faces on one plane, the base's side and the
//! upright's side likewise, joined by edges that bound nothing. Those
//! redundant edges are real defects downstream: a fillet on the L's inside
//! corner met a vertex with three non-blend faces where the true corner has
//! one, and refused ("higher-valence corner blends require corner-patch
//! synthesis"); a drawing inks the seams as lines. `BooleanOptions::
//! merge_coincident` promised this merge and was read by nothing.
//!
//! A group of faces merges when they lie on one plane, face the same way,
//! and are joined through shared edges. The merged face is built the way
//! the boolean builds every face (loops from oriented edge uses, the largest
//! loop outer, `measure_and_set_face_uv_bounds`), on the surface and
//! orientation of the group's largest face, whose persistent id it takes
//! over. The absorbed faces and the inner edges are only DETACHED from the
//! shell: the boolean's orphan sweep (`prune_boolean_orphan_topology`)
//! removes them and purges their sidecars, the same path every other
//! boolean leftover takes.
//!
//! Honest limits: planar faces only (`Plane` carriers); a group whose
//! boundary pinches (a vertex where two boundary loops touch) or does not
//! close is left as it is -- the pre-merge result, still sound, never a
//! guessed loop.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::{Face, FaceId};
use crate::primitives::r#loop::{Loop, LoopType};
use crate::primitives::shell::ShellId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::Plane;
use crate::primitives::topology_builder::BRepModel;
use crate::primitives::vertex::VertexId;

/// Merge the split coplanar faces of `solid`'s shells. Returns how many
/// faces were absorbed into another.
pub(crate) fn merge_coplanar_faces(
    model: &mut BRepModel,
    solid: SolidId,
    tolerance: &Tolerance,
) -> usize {
    let Some(s) = model.solids.get(solid) else {
        return 0;
    };
    let shells: Vec<ShellId> = std::iter::once(s.outer_shell)
        .chain(s.inner_shells.iter().copied())
        .chain(s.peer_shells.iter().copied())
        .collect();
    shells
        .into_iter()
        .map(|sh| merge_in_shell(model, sh, tolerance))
        .sum()
}

/// A planar face's plane: a point on it and its OUTWARD normal (the
/// surface normal turned by the face's orientation).
fn outward_plane(model: &BRepModel, face: FaceId) -> Option<(Point3, Vector3)> {
    let f = model.faces.get(face)?;
    let surface = model.surfaces.get(f.surface_id)?;
    let plane = surface.as_any().downcast_ref::<Plane>()?;
    let n = plane.normal.normalize().ok()? * f.orientation.sign();
    Some((plane.origin, n))
}

/// Every (edge, forward) use of a face, outer loop first.
fn edge_uses(model: &BRepModel, face: FaceId) -> Vec<(EdgeId, bool)> {
    let Some(f) = model.faces.get(face) else {
        return Vec::new();
    };
    std::iter::once(f.outer_loop)
        .chain(f.inner_loops.iter().copied())
        .filter_map(|l| model.loops.get(l))
        .flat_map(|l| l.edges.iter().copied().zip(l.orientations.iter().copied()))
        .collect()
}

fn find(parent: &mut HashMap<FaceId, FaceId>, f: FaceId) -> FaceId {
    let p = *parent.get(&f).unwrap_or(&f);
    if p == f {
        return f;
    }
    let root = find(parent, p);
    parent.insert(f, root);
    root
}

fn merge_in_shell(model: &mut BRepModel, shell: ShellId, tolerance: &Tolerance) -> usize {
    let Some(sh) = model.shells.get(shell) else {
        return 0;
    };
    let faces: Vec<FaceId> = sh.faces.clone();
    let planes: HashMap<FaceId, (Point3, Vector3)> = faces
        .iter()
        .filter_map(|&f| outward_plane(model, f).map(|p| (f, p)))
        .collect();
    let mut users: BTreeMap<EdgeId, BTreeSet<FaceId>> = BTreeMap::new();
    for &f in &faces {
        for (e, _) in edge_uses(model, f) {
            users.entry(e).or_default().insert(f);
        }
    }
    let same_plane = |a: FaceId, b: FaceId| -> bool {
        let (Some(&(pa, na)), Some(&(pb, nb))) = (planes.get(&a), planes.get(&b)) else {
            return false;
        };
        na.dot(&nb) >= tolerance.angle().cos() && na.dot(&(pb - pa)).abs() <= tolerance.distance()
    };
    let mut parent: HashMap<FaceId, FaceId> = HashMap::new();
    for fs in users.values() {
        if fs.len() != 2 {
            continue;
        }
        let mut it = fs.iter();
        let (Some(&a), Some(&b)) = (it.next(), it.next()) else {
            continue;
        };
        if same_plane(a, b) {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent.insert(ra, rb);
            }
        }
    }
    let mut groups: BTreeMap<FaceId, Vec<FaceId>> = BTreeMap::new();
    for &f in &faces {
        if planes.contains_key(&f) {
            let r = find(&mut parent, f);
            groups.entry(r).or_default().push(f);
        }
    }
    groups
        .into_values()
        .filter(|g| g.len() > 1)
        .map(|g| {
            if merge_group(model, shell, &g, &planes) {
                g.len() - 1
            } else {
                0
            }
        })
        .sum()
}

/// The signed area of a closed vertex walk projected on the plane normal.
fn loop_area(model: &BRepModel, walk: &[VertexId], normal: Vector3) -> f64 {
    let pts: Vec<Point3> = walk
        .iter()
        .filter_map(|&v| {
            model
                .vertices
                .get(v)
                .map(|p| Point3::new(p.position[0], p.position[1], p.position[2]))
        })
        .collect();
    let mut sum = Vector3::new(0.0, 0.0, 0.0);
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        sum = sum + Vector3::new(a.x, a.y, a.z).cross(&Vector3::new(b.x, b.y, b.z));
    }
    0.5 * sum.dot(&normal)
}

fn merge_group(
    model: &mut BRepModel,
    shell: ShellId,
    group: &[FaceId],
    planes: &HashMap<FaceId, (Point3, Vector3)>,
) -> bool {
    // Edge uses inside the group: an edge used twice is between two of its
    // faces (interior, dropped); used once, it bounds the merged face.
    let mut count: HashMap<EdgeId, usize> = HashMap::new();
    let mut all: Vec<(EdgeId, bool)> = Vec::new();
    for &f in group {
        for u in edge_uses(model, f) {
            *count.entry(u.0).or_default() += 1;
            all.push(u);
        }
    }
    let boundary: Vec<(EdgeId, bool)> = all
        .into_iter()
        .filter(|u| count.get(&u.0) == Some(&1))
        .collect();
    // Oriented ends of each boundary use; one use leaving each vertex, or the
    // boundary pinches there and is left alone.
    let mut ends: Vec<(VertexId, VertexId)> = Vec::with_capacity(boundary.len());
    let mut leaving: HashMap<VertexId, usize> = HashMap::new();
    for (i, &(e, fwd)) in boundary.iter().enumerate() {
        let Some(edge) = model.edges.get(e) else {
            return false;
        };
        let (s, t) = if fwd {
            (edge.start_vertex, edge.end_vertex)
        } else {
            (edge.end_vertex, edge.start_vertex)
        };
        if leaving.insert(s, i).is_some() {
            return false;
        }
        ends.push((s, t));
    }
    // Chain the uses into closed loops.
    let mut used = vec![false; boundary.len()];
    let mut loops: Vec<(Vec<(EdgeId, bool)>, Vec<VertexId>)> = Vec::new();
    for start in 0..boundary.len() {
        if used[start] {
            continue;
        }
        let mut walk = Vec::new();
        let mut verts = Vec::new();
        let mut i = start;
        loop {
            if used[i] {
                return false;
            }
            used[i] = true;
            walk.push(boundary[i]);
            verts.push(ends[i].0);
            let Some(&next) = leaving.get(&ends[i].1) else {
                return false;
            };
            if next == start {
                break;
            }
            i = next;
        }
        loops.push((walk, verts));
    }
    if loops.is_empty() {
        return false;
    }
    // The keeper: the group's largest face lends its surface, orientation
    // and persistent id.
    let normal = match planes.get(&group[0]) {
        Some(&(_, n)) => n,
        None => return false,
    };
    let area_of = |m: &BRepModel, f: FaceId| -> f64 {
        m.faces
            .get(f)
            .and_then(|face| m.loops.get(face.outer_loop))
            .map(|l| {
                let vs: Vec<VertexId> = l
                    .edges
                    .iter()
                    .zip(&l.orientations)
                    .filter_map(|(&e, &fw)| {
                        m.edges
                            .get(e)
                            .map(|ed| if fw { ed.start_vertex } else { ed.end_vertex })
                    })
                    .collect();
                loop_area(m, &vs, normal).abs()
            })
            .unwrap_or(0.0)
    };
    let keeper = group
        .iter()
        .copied()
        .max_by(|&a, &b| {
            area_of(model, a)
                .partial_cmp(&area_of(model, b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(group[0]);
    let Some((surface_id, orientation)) = model
        .faces
        .get(keeper)
        .map(|f| (f.surface_id, f.orientation))
    else {
        return false;
    };
    // The outer loop encloses the most area; the rest are holes.
    let outer = loops
        .iter()
        .enumerate()
        .max_by(|a, b| {
            loop_area(model, &a.1 .1, normal)
                .abs()
                .partial_cmp(&loop_area(model, &b.1 .1, normal).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mut make_loop = |walk: &[(EdgeId, bool)], kind: LoopType| {
        let mut l = Loop::new(0, kind);
        for &(e, fwd) in walk {
            l.add_edge(e, fwd);
        }
        model.loops.add(l)
    };
    let outer_id = make_loop(&loops[outer].0, LoopType::Outer);
    let inner_ids: Vec<_> = loops
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != outer)
        .map(|(_, (walk, _))| make_loop(walk, LoopType::Inner))
        .collect();
    let merged = model
        .faces
        .add(Face::new(0, surface_id, outer_id, orientation));
    if let Some(face) = model.faces.get_mut(merged) {
        for l in inner_ids {
            face.add_inner_loop(l);
        }
    }
    crate::tessellation::surface::measure_and_set_face_uv_bounds(model, merged);
    if let Some(pid) = model.face_pid(keeper) {
        model.face_pids.remove(&keeper);
        model.set_face_pid(merged, pid);
    }
    if let Some(sh) = model.shells.get_mut(shell) {
        for &f in group {
            sh.remove_face(f);
        }
        sh.add_face(merged);
    }
    true
}
