//! B-Rep structural-integrity harness — pinpoints *where* a solid is malformed.
//!
//! The universal [`watertight`](crate::harness::watertight) oracle answers "is
//! the tessellated mesh a closed oriented manifold?" — a yes/no verdict on the
//! *output*. This harness answers the *why*: it inspects the B-Rep itself (loops,
//! edges, vertices, the shell adjacency graph) and reports the first class of
//! invariant that breaks, with the offending ids. It is the tool for debugging an
//! operation step by step: run it after each pipeline stage and the stage that
//! first reports a violation is the one that introduced the bug.
//!
//! Invariants checked, in increasing subtlety:
//!
//! 1. **Loop closure** — every loop's edges form a single closed cycle: walking
//!    edge `i` in its stored orientation lands on the start vertex of edge `i+1`,
//!    and the last closes back to the first.
//! 2. **Edge→face usage** — in a closed 2-manifold shell every edge is used by
//!    *exactly two* face loops. `used==1` is an open boundary; `used>=3` is
//!    non-manifold.
//! 3. **Unmerged vertices** — no two *distinct* vertex ids occupy the same
//!    position (within tolerance). Duplicates are the classic cause of a
//!    topologically-"closed" shell whose faces nonetheless meet at coincident but
//!    distinct points, so the seam never welds.
//! 4. **Coincident edges** — no two *distinct* edges share the same endpoint
//!    positions. Two faces stitched to a *pair* of coincident edges (instead of
//!    one shared edge) read as "every edge used twice" yet leave an unweldable
//!    seam — exactly the failure mode static analysis keeps mis-reading.
//! 5. **Adjacency-vs-geometry** — for each edge used by two faces, both endpoints
//!    must lie on both faces' loops (they do, by construction, when the edge is
//!    genuinely shared) — surfaced here as the shell adjacency graph so a caller
//!    can see *which* face types meet at each seam.

use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::r#loop::LoopId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::primitives::vertex::VertexId;
use std::collections::BTreeMap;

/// Quantise a position to an integer lattice at spacing `eps` for coincidence
/// grouping (mirrors the tessellation weld key).
fn key(p: [f64; 3], eps: f64) -> (i64, i64, i64) {
    (
        (p[0] / eps).round() as i64,
        (p[1] / eps).round() as i64,
        (p[2] / eps).round() as i64,
    )
}

/// One edge's adjacency record: the face loops that reference it and the face
/// types behind them.
#[derive(Debug, Clone)]
pub struct EdgeAdjacency {
    pub edge: EdgeId,
    pub start: VertexId,
    pub end: VertexId,
    pub chord_len: f64,
    /// (face id, surface type name) for every face loop referencing this edge.
    pub faces: Vec<(FaceId, &'static str)>,
}

/// Structural integrity report for one solid's outer + inner shells.
#[derive(Debug, Clone, Default)]
pub struct BRepIntegrityReport {
    pub faces: usize,
    pub edges_in_loops: usize,
    pub vertices: usize,

    /// Loops whose edge cycle does not close (open chain or broken adjacency).
    pub open_loops: Vec<LoopId>,
    /// Edges referenced by exactly one face loop (B-Rep boundary — open shell).
    pub edges_used_once: Vec<EdgeId>,
    /// Edges referenced by three or more face loops (non-manifold).
    pub edges_used_3plus: Vec<(EdgeId, usize)>,
    /// Groups of distinct vertex ids sharing one position (unmerged duplicates).
    pub duplicate_vertex_groups: Vec<Vec<VertexId>>,
    /// Groups of distinct edges sharing the same endpoint positions
    /// (coincident-but-distinct edges — the unweldable-seam culprit).
    pub coincident_edge_groups: Vec<Vec<EdgeId>>,
    /// Full per-edge adjacency (every edge in a shell loop → its faces).
    pub adjacency: Vec<EdgeAdjacency>,
}

impl BRepIntegrityReport {
    /// The B-Rep is a structurally valid closed 2-manifold shell.
    pub fn is_clean(&self) -> bool {
        self.open_loops.is_empty()
            && self.edges_used_once.is_empty()
            && self.edges_used_3plus.is_empty()
            && self.duplicate_vertex_groups.is_empty()
            && self.coincident_edge_groups.is_empty()
    }

    /// Human-readable summary; lists the first few offenders of each failing
    /// class with their face-type context.
    pub fn render(&self, model: &BRepModel) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "BRep integrity: faces={} loop-edges={} vertices={} clean={}\n",
            self.faces,
            self.edges_in_loops,
            self.vertices,
            self.is_clean()
        ));
        if !self.open_loops.is_empty() {
            s.push_str(&format!(
                "  OPEN LOOPS ({}): {:?}\n",
                self.open_loops.len(),
                &self.open_loops[..self.open_loops.len().min(8)]
            ));
        }
        if !self.edges_used_once.is_empty() {
            s.push_str(&format!(
                "  EDGES USED ONCE — open shell ({}): {:?}\n",
                self.edges_used_once.len(),
                &self.edges_used_once[..self.edges_used_once.len().min(12)]
            ));
        }
        if !self.edges_used_3plus.is_empty() {
            s.push_str(&format!(
                "  EDGES USED 3+ — non-manifold ({}): {:?}\n",
                self.edges_used_3plus.len(),
                &self.edges_used_3plus[..self.edges_used_3plus.len().min(8)]
            ));
        }
        if !self.duplicate_vertex_groups.is_empty() {
            s.push_str(&format!(
                "  UNMERGED VERTICES ({} groups):\n",
                self.duplicate_vertex_groups.len()
            ));
            for g in self.duplicate_vertex_groups.iter().take(6) {
                let p = g
                    .first()
                    .and_then(|&v| model.vertices.get(v))
                    .map(|v| v.position)
                    .unwrap_or([0.0; 3]);
                s.push_str(&format!(
                    "    {:?} @ ({:.3},{:.3},{:.3})\n",
                    g, p[0], p[1], p[2]
                ));
            }
        }
        if !self.coincident_edge_groups.is_empty() {
            s.push_str(&format!(
                "  COINCIDENT EDGES ({} groups):\n",
                self.coincident_edge_groups.len()
            ));
            for g in self.coincident_edge_groups.iter().take(8) {
                let tys: Vec<String> = g
                    .iter()
                    .map(|&e| {
                        let faces: Vec<&'static str> = self
                            .adjacency
                            .iter()
                            .find(|a| a.edge == e)
                            .map(|a| a.faces.iter().map(|(_, t)| *t).collect())
                            .unwrap_or_default();
                        format!("e{e}{faces:?}")
                    })
                    .collect();
                s.push_str(&format!("    {tys:?}\n"));
            }
        }
        s
    }
}

/// Build the structural-integrity report for `solid` (outer + inner shells).
/// `eps` is the coincidence tolerance for vertex/edge grouping.
pub fn brep_integrity(model: &BRepModel, solid: SolidId, eps: f64) -> BRepIntegrityReport {
    let mut r = BRepIntegrityReport::default();
    let Some(solid_ref) = model.solids.get(solid) else {
        return r;
    };

    let mut shells = vec![solid_ref.outer_shell];
    shells.extend(solid_ref.inner_shells.iter().copied());

    // edge → list of (face id, type) referencing it, via face loops.
    let mut edge_faces: BTreeMap<EdgeId, Vec<(FaceId, &'static str)>> = BTreeMap::new();
    let mut face_count = 0usize;

    for shell_id in shells {
        let Some(shell) = model.shells.get(shell_id) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            face_count += 1;
            let ty = model
                .surfaces
                .get(face.surface_id)
                .map(|s| s.type_name())
                .unwrap_or("?");
            let mut loop_ids = vec![face.outer_loop];
            loop_ids.extend(face.inner_loops.iter().copied());
            for lid in loop_ids {
                let Some(lp) = model.loops.get(lid) else {
                    continue;
                };
                // Loop closure: walking each edge in its orientation must chain.
                if !loop_closes(lp, model) {
                    r.open_loops.push(lid);
                }
                for &eid in &lp.edges {
                    edge_faces.entry(eid).or_default().push((fid, ty));
                }
            }
        }
    }
    r.faces = face_count;
    r.edges_in_loops = edge_faces.len();

    // Edge usage + adjacency.
    for (&eid, faces) in &edge_faces {
        match faces.len() {
            1 => r.edges_used_once.push(eid),
            2 => {}
            n => r.edges_used_3plus.push((eid, n)),
        }
        if let Some(edge) = model.edges.get(eid) {
            let a = model.vertices.get(edge.start_vertex).map(|v| v.position);
            let b = model.vertices.get(edge.end_vertex).map(|v| v.position);
            let chord = match (a, b) {
                (Some(a), Some(b)) => {
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
                }
                _ => 0.0,
            };
            r.adjacency.push(EdgeAdjacency {
                edge: eid,
                start: edge.start_vertex,
                end: edge.end_vertex,
                chord_len: chord,
                faces: faces.clone(),
            });
        }
    }

    // Unmerged vertices: distinct ids referenced by these edges sharing a position.
    let mut pos_groups: BTreeMap<(i64, i64, i64), Vec<VertexId>> = BTreeMap::new();
    let mut seen_v = std::collections::BTreeSet::new();
    for adj in &r.adjacency {
        for v in [adj.start, adj.end] {
            if seen_v.insert(v) {
                if let Some(vx) = model.vertices.get(v) {
                    pos_groups.entry(key(vx.position, eps)).or_default().push(v);
                }
            }
        }
    }
    r.vertices = seen_v.len();
    for g in pos_groups.into_values() {
        if g.len() > 1 {
            r.duplicate_vertex_groups.push(g);
        }
    }

    // Coincident edges: distinct edges whose endpoint POSITIONS match (as an
    // unordered pair). Two faces stitched to coincident-but-distinct edges read
    // as a closed shell yet never weld.
    let mut edge_pos_groups: BTreeMap<[(i64, i64, i64); 2], Vec<EdgeId>> = BTreeMap::new();
    for adj in &r.adjacency {
        let pa = model.vertices.get(adj.start).map(|v| v.position);
        let pb = model.vertices.get(adj.end).map(|v| v.position);
        if let (Some(pa), Some(pb)) = (pa, pb) {
            let (ka, kb) = (key(pa, eps), key(pb, eps));
            let pair = if ka <= kb { [ka, kb] } else { [kb, ka] };
            edge_pos_groups.entry(pair).or_default().push(adj.edge);
        }
    }
    for g in edge_pos_groups.into_values() {
        if g.len() > 1 {
            r.coincident_edge_groups.push(g);
        }
    }

    r
}

/// Does the loop's edge sequence form a single closed cycle? Walks each edge in
/// its stored orientation (forward → start..end, reversed → end..start) and
/// checks the head-to-tail chain closes.
fn loop_closes(lp: &crate::primitives::r#loop::Loop, model: &BRepModel) -> bool {
    let n = lp.edges.len();
    if n == 0 {
        return false;
    }
    let endpoints = |eid: EdgeId, fwd: bool| -> Option<(VertexId, VertexId)> {
        let e = model.edges.get(eid)?;
        Some(if fwd {
            (e.start_vertex, e.end_vertex)
        } else {
            (e.end_vertex, e.start_vertex)
        })
    };
    let mut prev_end: Option<VertexId> = None;
    let mut first_start: Option<VertexId> = None;
    for (i, &eid) in lp.edges.iter().enumerate() {
        let fwd = lp.orientations.get(i).copied().unwrap_or(true);
        let Some((s, e)) = endpoints(eid, fwd) else {
            return false;
        };
        if first_start.is_none() {
            first_start = Some(s);
        }
        if let Some(p) = prev_end {
            if p != s {
                return false;
            }
        }
        prev_end = Some(e);
    }
    prev_end == first_start
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
    use crate::operations::fillet::{fillet_edges, FilletOptions, FilletType};
    use crate::primitives::topology_builder::TopologyBuilder;

    fn box_all_edges(side: f64) -> (BRepModel, SolidId, Vec<EdgeId>) {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(side, side, side)
            .expect("box");
        let solid = model.solids.iter().last().map(|(id, _)| id).expect("s");
        let edges: Vec<_> = model.edges.iter().map(|(id, _)| id).collect();
        (model, solid, edges)
    }

    /// POSITIVE CONTROL: the all-12-edge fillet (fixed in #51) must be
    /// structurally clean — proves the harness does not false-positive on a
    /// known-good multi-edge corner result.
    #[test]
    fn all_edges_fillet_is_structurally_clean() {
        let (mut model, solid, edges) = box_all_edges(4.0);
        fillet_edges(
            &mut model,
            solid,
            edges,
            FilletOptions {
                fillet_type: FilletType::Constant(1.0),
                radius: 1.0,
                ..Default::default()
            },
        )
        .expect("fillet");
        let r = brep_integrity(&model, solid, 1e-6);
        assert!(
            r.is_clean(),
            "filleted box not structurally clean:\n{}",
            r.render(&model)
        );
    }

    /// The all-12-edge chamfer (fixed in #52) must be structurally clean: the
    /// degree-3 trim mitering + walk-order cap loop close every face loop. This
    /// is the harness that *found* the bug — 8 open box/cap loops — so it is the
    /// permanent guard against regressing the corner synthesis.
    #[test]
    fn all_edges_chamfer_is_structurally_clean() {
        let (mut model, solid, edges) = box_all_edges(4.0);
        chamfer_edges(
            &mut model,
            solid,
            edges,
            ChamferOptions {
                chamfer_type: ChamferType::EqualDistance(1.0),
                distance1: 1.0,
                distance2: 1.0,
                symmetric: true,
                ..Default::default()
            },
        )
        .expect("chamfer");
        let r = brep_integrity(&model, solid, 1e-6);
        assert!(
            r.is_clean(),
            "chamfered box not structurally clean:\n{}",
            r.render(&model)
        );
    }

    /// DIAGNOSTIC: dump the structural integrity report of the all-12-edge
    /// chamfer. The first failing invariant pinpoints any future defect.
    #[test]
    #[ignore = "diagnostic: structural integrity dump of the all-12 chamfer"]
    fn diag_all_edges_chamfer_integrity() {
        let (mut model, solid, edges) = box_all_edges(4.0);
        chamfer_edges(
            &mut model,
            solid,
            edges,
            ChamferOptions {
                chamfer_type: ChamferType::EqualDistance(1.0),
                distance1: 1.0,
                distance2: 1.0,
                symmetric: true,
                ..Default::default()
            },
        )
        .expect("chamfer");
        let r = brep_integrity(&model, solid, 1e-6);
        eprintln!("{}", r.render(&model));
        // Dump the edge chain of each open loop to see exactly where it breaks.
        for &lid in r.open_loops.iter().take(3) {
            if let Some(lp) = model.loops.get(lid) {
                eprintln!("  --- open loop {lid} ({} edges) ---", lp.edges.len());
                for (i, &eid) in lp.edges.iter().enumerate() {
                    let fwd = lp.orientations.get(i).copied().unwrap_or(true);
                    if let Some(e) = model.edges.get(eid) {
                        let (s, en) = if fwd {
                            (e.start_vertex, e.end_vertex)
                        } else {
                            (e.end_vertex, e.start_vertex)
                        };
                        let ps = model
                            .vertices
                            .get(s)
                            .map(|v| v.position)
                            .unwrap_or([0.0; 3]);
                        let pe = model
                            .vertices
                            .get(en)
                            .map(|v| v.position)
                            .unwrap_or([0.0; 3]);
                        eprintln!(
                            "    e{eid} fwd={fwd} v{s}({:.1},{:.1},{:.1}) -> v{en}({:.1},{:.1},{:.1})",
                            ps[0], ps[1], ps[2], pe[0], pe[1], pe[2]
                        );
                    }
                }
            }
        }
    }
}
