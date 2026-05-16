//! Closed-shell manifold validation.
//!
//! A topologically valid closed 2-manifold shell satisfies the
//! edge-use invariant: **every edge appears in exactly two oriented
//! face-uses across the shell, once forward and once backward**.
//! Any deviation indicates a corruption that downstream operations
//! (boolean intersect/union/difference, mass-properties, mesh-for-
//! print) will silently mishandle.
//!
//! Concretely, after building a `CLOSED_SHELL` from a STEP file we
//! walk every face's outer loop and inner loops, accumulating a
//! `HashMap<EdgeId, (forward_uses, backward_uses)>`. The classifier:
//!
//!   - `forward_uses + backward_uses == 0`: unreachable (edge not in
//!     any face — would not have been collected).
//!   - `total == 1`: **dangling edge** — the shell has a free boundary
//!     so is not closed. Reported as
//!     [`ManifoldKind::DanglingEdge`].
//!   - `total > 2`: **non-manifold edge** — three or more faces share
//!     this edge. Reported as [`ManifoldKind::NonManifoldEdge`].
//!   - `total == 2` with `forward_uses == 2` or `backward_uses == 2`:
//!     **orientation mismatch** — two faces share the edge but with
//!     consistent rather than opposing orientations, so their normals
//!     point inconsistently across the seam. Reported as
//!     [`ManifoldKind::OrientationMismatch`].
//!   - `total == 2` with one forward + one backward: manifold. Healthy.
//!
//! The function returns a [`ManifoldReport`] aggregating the failure
//! buckets. The caller decides whether to surface each as a
//! [`ManifoldWarning`] on the import report; the convenience helper
//! [`emit_manifold_warnings`] does that in the common case.

use std::collections::HashMap;

use geometry_engine::primitives::{
    edge::EdgeId,
    face::FaceId,
    r#loop::LoopId,
    shell::ShellId,
    topology_builder::BRepModel,
};

use crate::formats::step::{
    context::ImportContext,
    diagnostics::{ManifoldKind, ManifoldWarning},
};

/// Aggregated outcome of a manifold check on one shell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifoldReport {
    /// Edges used by exactly one face — the shell has free boundary.
    pub dangling_edges: Vec<EdgeId>,
    /// Edges used by three or more face-uses.
    pub non_manifold_edges: Vec<EdgeId>,
    /// Edges used by exactly two faces but with consistent
    /// orientation along the edge (both forward or both backward).
    pub orientation_mismatches: Vec<EdgeId>,
}

impl ManifoldReport {
    /// `true` when every edge in the shell is used in exactly one
    /// forward + one backward face-use.
    pub fn is_manifold(&self) -> bool {
        self.dangling_edges.is_empty()
            && self.non_manifold_edges.is_empty()
            && self.orientation_mismatches.is_empty()
    }
}

/// Walk `shell_id` in `model` and classify every edge by its face-use
/// pattern. Returns the buckets; no side effects.
pub fn validate_closed_shell(model: &BRepModel, shell_id: ShellId) -> Option<ManifoldReport> {
    let shell = model.shells.get(shell_id)?;
    let mut counts: HashMap<EdgeId, (u32, u32)> = HashMap::new();

    for &face_id in &shell.faces {
        accumulate_face_edges(model, face_id, &mut counts);
    }

    let mut report = ManifoldReport::default();
    for (edge_id, (fwd, bwd)) in counts {
        let total = fwd + bwd;
        if total == 1 {
            report.dangling_edges.push(edge_id);
        } else if total > 2 {
            report.non_manifold_edges.push(edge_id);
        } else if total == 2 && (fwd == 2 || bwd == 2) {
            report.orientation_mismatches.push(edge_id);
        }
    }

    // Deterministic order so test assertions are stable.
    report.dangling_edges.sort_unstable();
    report.non_manifold_edges.sort_unstable();
    report.orientation_mismatches.sort_unstable();

    Some(report)
}

/// Push every edge of `face_id`'s outer loop and inner loops into
/// `counts`, tallying forward vs. backward orientation.
fn accumulate_face_edges(
    model: &BRepModel,
    face_id: FaceId,
    counts: &mut HashMap<EdgeId, (u32, u32)>,
) {
    let Some(face) = model.faces.get(face_id) else {
        return;
    };
    accumulate_loop_edges(model, face.outer_loop, counts);
    for &lid in &face.inner_loops {
        accumulate_loop_edges(model, lid, counts);
    }
}

/// Push every (edge, forward) pair of `loop_id` into `counts`.
fn accumulate_loop_edges(
    model: &BRepModel,
    loop_id: LoopId,
    counts: &mut HashMap<EdgeId, (u32, u32)>,
) {
    let Some(lp) = model.loops.get(loop_id) else {
        return;
    };
    for (i, &edge_id) in lp.edges.iter().enumerate() {
        let forward = lp.orientations.get(i).copied().unwrap_or(true);
        let entry = counts.entry(edge_id).or_insert((0, 0));
        if forward {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }
}

/// Convenience: lift each non-empty bucket from `report` into a
/// [`ManifoldWarning`] on `ctx.report.manifold_warnings`. Returns
/// `true` when at least one warning was emitted.
pub fn emit_manifold_warnings(
    shell_instance: u64,
    report: &ManifoldReport,
    ctx: &mut ImportContext<'_>,
) -> bool {
    let mut emitted = false;
    if !report.dangling_edges.is_empty() {
        ctx.report.push_manifold_warning(ManifoldWarning {
            kind: ManifoldKind::DanglingEdge,
            shell_instance,
            edge_count: report.dangling_edges.len(),
        });
        emitted = true;
    }
    if !report.non_manifold_edges.is_empty() {
        ctx.report.push_manifold_warning(ManifoldWarning {
            kind: ManifoldKind::NonManifoldEdge,
            shell_instance,
            edge_count: report.non_manifold_edges.len(),
        });
        emitted = true;
    }
    if !report.orientation_mismatches.is_empty() {
        ctx.report.push_manifold_warning(ManifoldWarning {
            kind: ManifoldKind::OrientationMismatch,
            shell_instance,
            edge_count: report.orientation_mismatches.len(),
        });
        emitted = true;
    }
    emitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use geometry_engine::primitives::{
        edge::Edge,
        face::{Face, FaceOrientation},
        r#loop::{Loop, LoopType},
        shell::{Shell, ShellType},
        topology_builder::BRepModel,
    };
    use geometry_engine::primitives::curve::{Line, ParameterRange};
    use geometry_engine::primitives::surface::Plane;
    use geometry_engine::math::{Point3, Vector3};

    /// Build a minimal closed quad: one face with one loop of four
    /// edges where every edge is used twice (once forward via this
    /// face, once backward via a partner face we synthesise from the
    /// same edges in reverse). Synthesising the partner is easier
    /// than constructing a full cube — but for the "is_manifold" test
    /// we just need each edge to be used exactly once fwd + once bwd.
    fn build_two_face_quad() -> (BRepModel, ShellId) {
        let mut model = BRepModel::new();

        // 4 vertices of a unit square.
        let v00 = model.vertices.add(0.0, 0.0, 0.0);
        let v10 = model.vertices.add(1.0, 0.0, 0.0);
        let v11 = model.vertices.add(1.0, 1.0, 0.0);
        let v01 = model.vertices.add(0.0, 1.0, 0.0);

        // 4 edge curves (lines between adjacent corners).
        let c_b = model.curves.add(Box::new(
            Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)),
        ));
        let c_r = model.curves.add(Box::new(
            Line::new(Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0)),
        ));
        let c_t = model.curves.add(Box::new(
            Line::new(Point3::new(1.0, 1.0, 0.0), Point3::new(0.0, 1.0, 0.0)),
        ));
        let c_l = model.curves.add(Box::new(
            Line::new(Point3::new(0.0, 1.0, 0.0), Point3::new(0.0, 0.0, 0.0)),
        ));

        let e_b = model.edges.add(Edge::new(
            0,
            v00,
            v10,
            c_b,
            geometry_engine::primitives::edge::EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let e_r = model.edges.add(Edge::new(
            0,
            v10,
            v11,
            c_r,
            geometry_engine::primitives::edge::EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let e_t = model.edges.add(Edge::new(
            0,
            v11,
            v01,
            c_t,
            geometry_engine::primitives::edge::EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let e_l = model.edges.add(Edge::new(
            0,
            v01,
            v00,
            c_l,
            geometry_engine::primitives::edge::EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));

        // Loop A: e_b fwd, e_r fwd, e_t fwd, e_l fwd.
        let mut loop_a = Loop::new(0, LoopType::Outer);
        loop_a.add_edge(e_b, true);
        loop_a.add_edge(e_r, true);
        loop_a.add_edge(e_t, true);
        loop_a.add_edge(e_l, true);
        let lid_a = model.loops.add(loop_a);

        // Loop B: same edges, all backward (the imaginary opposite face).
        let mut loop_b = Loop::new(0, LoopType::Outer);
        loop_b.add_edge(e_b, false);
        loop_b.add_edge(e_r, false);
        loop_b.add_edge(e_t, false);
        loop_b.add_edge(e_l, false);
        let lid_b = model.loops.add(loop_b);

        // 2 faces on the same z=0 plane (a degenerate model — we only
        // care about the edge-use topology here).
        let plane = model.surfaces.add(Box::new(
            Plane::new(
                Point3::new(0.0, 0.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
        ));
        let fa = model.faces.add(Face::new(0, plane, lid_a, FaceOrientation::Forward));
        let fb = model.faces.add(Face::new(0, plane, lid_b, FaceOrientation::Backward));

        let mut shell = Shell::new(0, ShellType::Closed);
        shell.faces.push(fa);
        shell.faces.push(fb);
        let sid = model.shells.add(shell);
        (model, sid)
    }

    #[test]
    fn closed_quad_pair_is_manifold() {
        let (model, sid) = build_two_face_quad();
        let r = validate_closed_shell(&model, sid).unwrap();
        assert!(r.is_manifold(), "got {r:?}");
    }

    #[test]
    fn unknown_shell_returns_none() {
        let model = BRepModel::new();
        // ShellId fabricated; not present in store.
        let r = validate_closed_shell(&model, 99);
        assert!(r.is_none());
    }
}
