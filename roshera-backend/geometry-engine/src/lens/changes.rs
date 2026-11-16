//! What an operation changed, by persistent id.
//!
//! An agent that has just run an operation needs to know which faces and
//! solids it created, which it consumed and which it reshaped -- named by the
//! ids that survive replays, so it can act on them next. [`snapshot`] records
//! every face of every live solid; [`changes`] compares two snapshots.
//!
//! A face that keeps its persistent id but whose boundary changed (a fillet
//! trimming its neighbours) is MODIFIED; one whose id appears is CREATED; one
//! whose id is gone is DELETED. Faces with no persistent id are counted, never
//! guessed at.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::primitives::face::FaceId;
use crate::primitives::persistent_id::PersistentId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// One face as the snapshot saw it.
#[derive(Debug, Clone, PartialEq)]
struct FaceRecord {
    face: FaceId,
    solid: SolidId,
    surface: &'static str,
    /// Loop count, edge count and the boundary's bounding box (to
    /// [`BOUNDARY_QUANTUM`]): equal for an untouched face, different for one an
    /// operation trimmed or extended.
    boundary: (usize, usize, [i64; 6]),
}

/// Every face and solid of a model, by persistent id, at one moment.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    faces: BTreeMap<PersistentId, FaceRecord>,
    solids: BTreeMap<PersistentId, SolidId>,
    unidentified_faces: usize,
}

/// A face in a [`ChangeSet`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FaceChange {
    #[serde(serialize_with = "hex")]
    pub pid: PersistentId,
    /// The face's numeric id at the time (after the operation for a created
    /// or modified face, before it for a deleted one).
    pub face: FaceId,
    pub solid: SolidId,
    pub surface: String,
}

/// A solid in a [`ChangeSet`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SolidChange {
    #[serde(serialize_with = "hex")]
    pub pid: PersistentId,
    pub solid: SolidId,
}

/// What changed between two snapshots.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ChangeSet {
    /// `false` when there was no earlier snapshot to compare with (the first
    /// operation after a document was opened): the lists are then empty, not
    /// "everything is new".
    pub baseline: bool,
    pub solids_created: Vec<SolidChange>,
    pub solids_deleted: Vec<SolidChange>,
    /// Solids that kept their persistent id but gained, lost or reshaped
    /// faces -- the body a hole was cut in is still that body.
    pub solids_modified: Vec<SolidChange>,
    pub faces_created: Vec<FaceChange>,
    pub faces_deleted: Vec<FaceChange>,
    pub faces_modified: Vec<FaceChange>,
    /// Faces of live solids that carry no persistent id, after the operation.
    pub unidentified_faces: usize,
}

impl ChangeSet {
    /// Nothing created, deleted or modified.
    pub fn is_empty(&self) -> bool {
        self.solids_created.is_empty()
            && self.solids_deleted.is_empty()
            && self.solids_modified.is_empty()
            && self.faces_created.is_empty()
            && self.faces_deleted.is_empty()
            && self.faces_modified.is_empty()
    }
}

fn hex<S: serde::Serializer>(pid: &PersistentId, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!("{:032x}", pid.0))
}

/// Every face of every live solid in `model`, by persistent id.
pub fn snapshot(model: &BRepModel) -> Snapshot {
    let mut snap = Snapshot::default();
    for (sid, solid) in model.solids.iter() {
        if let Some(pid) = model.solid_pid(sid) {
            snap.solids.insert(pid, sid);
        }
        let shells = std::iter::once(solid.outer_shell)
            .chain(solid.inner_shells.iter().copied())
            .chain(solid.peer_shells.iter().copied());
        for sh in shells {
            let Some(shell) = model.shells.get(sh) else {
                continue;
            };
            for &fid in &shell.faces {
                let Some(pid) = model.face_pid(fid) else {
                    snap.unidentified_faces += 1;
                    continue;
                };
                let Some(face) = model.faces.get(fid) else {
                    continue;
                };
                let surface = model
                    .surfaces
                    .get(face.surface_id)
                    .map(|s| s.type_name())
                    .unwrap_or("?");
                snap.faces.insert(
                    pid,
                    FaceRecord {
                        face: fid,
                        solid: sid,
                        surface,
                        boundary: boundary_signature(model, face),
                    },
                );
            }
        }
    }
    snap
}

/// What changed from `before` to `after`. With no `before` the result has
/// `baseline: false` and empty lists.
pub fn changes(before: Option<&Snapshot>, after: &Snapshot) -> ChangeSet {
    let mut out = ChangeSet {
        unidentified_faces: after.unidentified_faces,
        ..ChangeSet::default()
    };
    let Some(before) = before else {
        return out;
    };
    out.baseline = true;
    let face_change = |pid: PersistentId, r: &FaceRecord| FaceChange {
        pid,
        face: r.face,
        solid: r.solid,
        surface: r.surface.to_string(),
    };
    for (&pid, &solid) in &after.solids {
        if !before.solids.contains_key(&pid) {
            out.solids_created.push(SolidChange { pid, solid });
        }
    }
    for (&pid, &solid) in &before.solids {
        if !after.solids.contains_key(&pid) {
            out.solids_deleted.push(SolidChange { pid, solid });
        }
    }
    for (&pid, rec) in &after.faces {
        match before.faces.get(&pid) {
            None => out.faces_created.push(face_change(pid, rec)),
            Some(old) if old.boundary != rec.boundary || old.surface != rec.surface => {
                out.faces_modified.push(face_change(pid, rec))
            }
            Some(_) => {}
        }
    }
    for (&pid, rec) in &before.faces {
        if !after.faces.contains_key(&pid) {
            out.faces_deleted.push(face_change(pid, rec));
        }
    }
    // A surviving solid is modified when any face it owns, before or after,
    // was created, deleted or modified.
    let touched: std::collections::BTreeSet<SolidId> = out
        .faces_created
        .iter()
        .chain(out.faces_modified.iter())
        .map(|f| f.solid)
        .collect();
    let touched_before: std::collections::BTreeSet<SolidId> =
        out.faces_deleted.iter().map(|f| f.solid).collect();
    for (&pid, &solid) in &after.solids {
        if let Some(&old) = before.solids.get(&pid) {
            if touched.contains(&solid) || touched_before.contains(&old) {
                out.solids_modified.push(SolidChange { pid, solid });
            }
        }
    }
    out
}

/// Coordinates are compared to this (mm), the same in every snapshot -- a
/// quantum scaled to the model would change when an operation removes a
/// large operand and read every untouched face as modified.
const BOUNDARY_QUANTUM: f64 = 1e-7;

/// A face's loop count, edge count and the bounding box of its loop
/// vertices, quantised so floating noise does not read as a change.
fn boundary_signature(
    model: &BRepModel,
    face: &crate::primitives::face::Face,
) -> (usize, usize, [i64; 6]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    let mut edges = 0usize;
    let loops: Vec<u32> = std::iter::once(face.outer_loop)
        .chain(face.inner_loops.iter().copied())
        .collect();
    for &l in &loops {
        let Some(lp) = model.loops.get(l) else {
            continue;
        };
        edges += lp.edges.len();
        for &e in &lp.edges {
            let Some(edge) = model.edges.get(e) else {
                continue;
            };
            for v in [edge.start_vertex, edge.end_vertex] {
                if let Some(vx) = model.vertices.get(v) {
                    for k in 0..3 {
                        lo[k] = lo[k].min(vx.position[k]);
                        hi[k] = hi[k].max(vx.position[k]);
                    }
                }
            }
        }
    }
    let q = |x: f64| {
        if x.is_finite() {
            (x / BOUNDARY_QUANTUM).round() as i64
        } else {
            0
        }
    };
    (
        loops.len(),
        edges,
        [q(lo[0]), q(lo[1]), q(lo[2]), q(hi[0]), q(hi[1]), q(hi[2])],
    )
}
