//! Topology surgery for edge-blend operations (fillet, chamfer).
//!
//! Both fillet and chamfer remove a manifold edge `E` between vertices
//! `V0` and `V1`, replace it with a new four-sided blend face whose
//! boundary is two **trim edges** (one on each adjacent face) plus two
//! **cap edges** (one at each end vertex). For the resulting B-Rep to
//! be watertight, the surrounding topology has to be re-stitched:
//!
//! * `F1` (the face on the first side of `E`) must replace `E` in its
//!   loop with `trim1`, and the F1 edges incident to `V0`/`V1` must be
//!   re-vertexed to terminate at the new trim endpoints.
//! * `F2` (the face on the other side of `E`) does the symmetric thing
//!   with `trim2`.
//! * `F3` (the face perpendicular to `E` at `V0`) — i.e. the face that
//!   shares both the F1-side and F2-side edges meeting at `V0` — must
//!   have `cap_v0` inserted into its loop, bridging the two newly
//!   re-vertexed neighbours.
//! * `F4` (the corresponding face at `V1`) gets `cap_v1` inserted.
//! * `E`, `V0`, and `V1` become orphaned and are removed.
//!
//! Both blend operations produce identical surgery data — they differ
//! only in surface geometry — so the surgery is shared in a single
//! production helper called from `fillet::update_adjacent_faces` and
//! `chamfer::update_adjacent_faces_for_chamfer`.
//!
//! ## Manifold assumption
//!
//! Each surgery assumes:
//! 1. `E` is shared by exactly two faces (F1, F2). Enforced by
//!    `get_adjacent_faces` upstream.
//! 2. `V0` is shared by exactly three edges (E, the F1-side neighbour,
//!    the F2-side neighbour) — i.e. a 3-valent corner. Box and
//!    extruded-prism corners satisfy this. Higher-valence corners
//!    (where 4+ faces meet at one vertex, e.g. octahedral corners or
//!    points where multiple edges have already been blended) require a
//!    corner-blend patch, not a simple cap insertion; we surface that
//!    case as `OperationError::NotImplemented` rather than emit invalid
//!    topology.
//! 3. The original edge appears in F1 and F2's **outer** loops only,
//!    not inner (hole) loops. Standard primitive and extrude output
//!    satisfies this; if a future caller needs hole-edge filleting it
//!    will trip the explicit error.

#![allow(clippy::indexing_slicing)] // loop indices bounded by edges.len()

use super::trim::{TrimCurve, TrimSide};
use super::{OperationError, OperationResult};
use crate::math::Tolerance;
use crate::primitives::{
    curve::{CurveId, ParameterRange},
    edge::{EdgeId, EdgeOrientation},
    face::FaceId,
    r#loop::LoopId,
    solid::SolidId,
    topology_builder::BRepModel,
    vertex::VertexId,
};
use std::collections::{HashMap, HashSet};

/// Data captured at blend-face construction time, consumed by
/// `splice_blend_edge` to re-stitch the surrounding topology.
///
/// All four trim/cap edges, all four new vertices, and the parent face
/// IDs are recorded as the blend face is being built. The surgery
/// helper has zero geometric responsibilities — it only walks loops
/// and rewires references.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlendEdgeSurgery {
    /// The original edge being blended away.
    pub original_edge: EdgeId,
    /// Original endpoints of the edge — removed at the end of surgery.
    pub original_v0: VertexId,
    pub original_v1: VertexId,
    /// The two faces previously sharing `original_edge`.
    pub face1: FaceId,
    pub face2: FaceId,
    /// New trim edge on F1 (start = `v_t1_start`, end = `v_t1_end`).
    pub trim1_edge: EdgeId,
    /// New trim edge on F2 (start = `v_t2_start`, end = `v_t2_end`).
    pub trim2_edge: EdgeId,
    /// Curve referenced by `trim1_edge`. Captured at surgery-build
    /// time so the F7-γ imprint pass can materialise a [`TrimCurve`]
    /// for F1 directly from the surgery, without re-looking-up the
    /// curve via the edge store. See [`Self::to_trim_curves`].
    pub trim1_curve: CurveId,
    /// Curve referenced by `trim2_edge`. Same rationale as
    /// [`Self::trim1_curve`].
    pub trim2_curve: CurveId,
    /// Cap edge at V0 (start = `v_t2_start`, end = `v_t1_start`). The
    /// blend-face producer is required to construct cap_v0 with this
    /// orientation so the F3 cap insertion can pick a deterministic
    /// loop direction.
    pub cap_v0_edge: EdgeId,
    /// Cap edge at V1 (start = `v_t1_end`, end = `v_t2_end`). Same
    /// orientation contract as `cap_v0_edge`.
    pub cap_v1_edge: EdgeId,
    /// New vertex on F1 near V0 (= start of trim1, end of cap_v0).
    pub v_t1_start: VertexId,
    /// New vertex on F1 near V1 (= end of trim1, start of cap_v1).
    pub v_t1_end: VertexId,
    /// New vertex on F2 near V0 (= start of trim2, start of cap_v0).
    pub v_t2_start: VertexId,
    /// New vertex on F2 near V1 (= end of trim2, end of cap_v1).
    pub v_t2_end: VertexId,
    /// F5-α.2 — `original_v0` is shared with two other blend edges at
    /// a `ConvexCorner { degree: 3 }` apex. When set, [`splice_blend_edge`]
    /// skips the V0-side topology surgery — the pred/succ edge rewire
    /// at V0, the third-face lookup at V0, the cap_v0 insertion, and
    /// the V0 vertex removal — because:
    /// 1. The pred/succ edges at V0 are *themselves* corner-fillet
    ///    edges whose own splice will replace them in the F1/F2 loop;
    ///    rewiring their V0-terminal pre-emptively would either be a
    ///    no-op (the new vertex is already P_a/P_b/P_c thanks to
    ///    `add_or_find`'s tolerance dedup) or it would break the next
    ///    splice's `current_at_terminal == expected` check.
    /// 2. No third face perpendicular at V0 exists in the live shell —
    ///    every face incident to V0 is one of {face1, face2} for *some*
    ///    edge in the corner; none survives as a non-blended cap host.
    /// 3. The cap_v0 arc is shared between the fillet face and the
    ///    apex sphere face (not a third face). The fillet-face loop
    ///    already references it from one side; the sphere face's loop
    ///    references it from the other side after
    ///    `apply_apex_sphere_corner` runs.
    /// 4. V0 itself is removed by `apply_apex_sphere_corner` once the
    ///    sphere face's loop is committed, not here.
    pub original_v0_corner_shared: bool,
    /// F5-α.2 — symmetric flag for `original_v1`. See
    /// [`Self::original_v0_corner_shared`].
    pub original_v1_corner_shared: bool,
}

impl BlendEdgeSurgery {
    /// Emit the two [`TrimCurve`]s implied by the rail edges, one
    /// per adjacent face. Both are `TrimSide::Discard` full-range:
    ///
    /// * Discard, because the rolling-ball (fillet) / chamfer-
    ///   bisector sweep covers the partition of F1 / F2 *between*
    ///   the rail and the original edge — that is the partition
    ///   F7-γ will detach from the shell and replace with the
    ///   blend face.
    /// * Full-range, because the rail curve is constructed to span
    ///   exactly the trim arc — there are no entry/exit re-entry
    ///   pairs to enumerate (cf. boolean intersection curves).
    ///
    /// This is the typed input that `operations::imprint::
    /// imprint_curves_on_face` (the F7-γ entry point) will consume.
    pub(crate) fn to_trim_curves(&self) -> [TrimCurve; 2] {
        [
            TrimCurve::full_range(self.trim1_curve, self.face1, TrimSide::Discard),
            TrimCurve::full_range(self.trim2_curve, self.face2, TrimSide::Discard),
        ]
    }

    /// Verify that every ID the surgery references still resolves
    /// in `model`. Called immediately before [`splice_blend_edge`]
    /// mutates state so a stale / dangling reference surfaces as a
    /// clean error rather than a partial topology mutation.
    ///
    /// Covers: the four faces (`face1`, `face2`, plus the two
    /// perpendicular faces resolved via [`find_third_face_at_vertex`]
    /// — that resolution happens inside `splice_blend_edge` so we
    /// do not duplicate it here), the four trim/cap edges, the two
    /// rail curves, the original edge and its two endpoints, and
    /// the four new boundary vertices.
    pub(crate) fn validate_surgery(&self, model: &BRepModel) -> OperationResult<()> {
        // Curves.
        if model.curves.get(self.trim1_curve).is_none() {
            return Err(OperationError::InvalidGeometry(format!(
                "BlendEdgeSurgery trim1_curve {} missing from model",
                self.trim1_curve
            )));
        }
        if model.curves.get(self.trim2_curve).is_none() {
            return Err(OperationError::InvalidGeometry(format!(
                "BlendEdgeSurgery trim2_curve {} missing from model",
                self.trim2_curve
            )));
        }
        // Faces (the two adjacent faces being trimmed).
        if model.faces.get(self.face1).is_none() {
            return Err(OperationError::InvalidGeometry(format!(
                "BlendEdgeSurgery face1 {} missing from model",
                self.face1
            )));
        }
        if model.faces.get(self.face2).is_none() {
            return Err(OperationError::InvalidGeometry(format!(
                "BlendEdgeSurgery face2 {} missing from model",
                self.face2
            )));
        }
        // Edges (original + four new blend boundary edges).
        for (label, edge_id) in [
            ("original_edge", self.original_edge),
            ("trim1_edge", self.trim1_edge),
            ("trim2_edge", self.trim2_edge),
            ("cap_v0_edge", self.cap_v0_edge),
            ("cap_v1_edge", self.cap_v1_edge),
        ] {
            if model.edges.get(edge_id).is_none() {
                return Err(OperationError::InvalidGeometry(format!(
                    "BlendEdgeSurgery {} {} missing from model",
                    label, edge_id
                )));
            }
        }
        // Vertices (original endpoints + four new boundary vertices).
        for (label, vertex_id) in [
            ("original_v0", self.original_v0),
            ("original_v1", self.original_v1),
            ("v_t1_start", self.v_t1_start),
            ("v_t1_end", self.v_t1_end),
            ("v_t2_start", self.v_t2_start),
            ("v_t2_end", self.v_t2_end),
        ] {
            if model.vertices.get(vertex_id).is_none() {
                return Err(OperationError::InvalidGeometry(format!(
                    "BlendEdgeSurgery {} {} missing from model",
                    label, vertex_id
                )));
            }
        }
        // Rail-edge curve refs must match the captured curve IDs.
        // Catches the case where a future blend-face builder forgets
        // to keep the surgery's curve fields in sync with the actual
        // edge.curve_id after re-creating an edge.
        let edge_trim1 = model.edges.get(self.trim1_edge).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "BlendEdgeSurgery trim1_edge {} missing",
                self.trim1_edge
            ))
        })?;
        if edge_trim1.curve_id != self.trim1_curve {
            return Err(OperationError::InvalidGeometry(format!(
                "BlendEdgeSurgery trim1_curve {} disagrees with trim1_edge.curve_id {}",
                self.trim1_curve, edge_trim1.curve_id
            )));
        }
        let edge_trim2 = model.edges.get(self.trim2_edge).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "BlendEdgeSurgery trim2_edge {} missing",
                self.trim2_edge
            ))
        })?;
        if edge_trim2.curve_id != self.trim2_curve {
            return Err(OperationError::InvalidGeometry(format!(
                "BlendEdgeSurgery trim2_curve {} disagrees with trim2_edge.curve_id {}",
                self.trim2_curve, edge_trim2.curve_id
            )));
        }
        Ok(())
    }
}

// `validate_no_shared_corners` was the historical blanket
// `NotImplemented` reject for any shared-corner blend selection.
// It has been superseded by `operations::lifecycle::validate_can_apply`'s
// F2-γ.1 setback-aware corner compatibility check, which now produces
// a *specific-reason* rejection (mixed convexity, cliff, rank-deficient
// axes, or "setback feasible but corner-patch synthesis missing") and
// runs uniformly as part of every fillet/chamfer pre-flight.

/// Splice the four-face neighbourhood of a freshly created blend face
/// back into a watertight B-Rep.
///
/// The function is idempotent only over its inputs — calling it twice
/// with the same surgery would fail on the second call because
/// `original_edge` has already been removed.
pub(crate) fn splice_blend_edge(
    model: &mut BRepModel,
    solid_id: SolidId,
    surgery: &BlendEdgeSurgery,
) -> OperationResult<()> {
    // F7-β pre-flight: verify every ID the surgery references is alive
    // in `model`, and that the captured rail-curve IDs agree with the
    // trim edges' actual curve refs. Failing this surfaces a stale-ID
    // bug as a clean error before any topology mutation runs.
    surgery.validate_surgery(model)?;

    // Resolve F3 / F4 BEFORE touching F1/F2 — once we re-vertex the
    // shared edges, V0 and V1 stop being incidence keys.
    //
    // F5-α.2: at a `ConvexCorner { degree: 3 }` apex, no third face
    // perpendicular to the blend exists — every face incident to the
    // corner is one of {face1, face2} for some edge in the corner. The
    // cap arc on that side belongs to the apex sphere face (built by
    // `apply_apex_sphere_corner` after every per-edge splice runs),
    // not to a third face here. Skip the F3/F4 lookup *and* the cap
    // insertion when the corresponding vertex is corner-shared.
    let face3 = if surgery.original_v0_corner_shared {
        None
    } else {
        Some(find_third_face_at_vertex(
            model,
            solid_id,
            surgery.original_v0,
            &[surgery.face1, surgery.face2],
        )?)
    };
    let face4 = if surgery.original_v1_corner_shared {
        None
    } else {
        Some(find_third_face_at_vertex(
            model,
            solid_id,
            surgery.original_v1,
            &[surgery.face1, surgery.face2],
        )?)
    };

    // Splice F1: replace E with trim1, re-vertex the V0/V1 neighbours
    // to terminate at v_t1_start / v_t1_end.
    splice_face_along_edge(
        model,
        surgery.face1,
        surgery.original_edge,
        surgery.trim1_edge,
        surgery.original_v0,
        surgery.original_v1,
        surgery.v_t1_start,
        surgery.v_t1_end,
        surgery.original_v0_corner_shared,
        surgery.original_v1_corner_shared,
    )?;

    // Splice F2 with trim2, re-vertexing to v_t2_start / v_t2_end.
    splice_face_along_edge(
        model,
        surgery.face2,
        surgery.original_edge,
        surgery.trim2_edge,
        surgery.original_v0,
        surgery.original_v1,
        surgery.v_t2_start,
        surgery.v_t2_end,
        surgery.original_v0_corner_shared,
        surgery.original_v1_corner_shared,
    )?;

    // F3: bridge the gap that opened at V0 between the F1/F2
    // neighbours (now ending/starting at v_t1_start / v_t2_start).
    if let Some(face3) = face3 {
        insert_cap_into_face_loop(
            model,
            face3,
            surgery.cap_v0_edge,
            surgery.v_t1_start,
            surgery.v_t2_start,
        )?;
    }

    // F4: same bridge at V1, between v_t1_end / v_t2_end.
    if let Some(face4) = face4 {
        insert_cap_into_face_loop(
            model,
            face4,
            surgery.cap_v1_edge,
            surgery.v_t1_end,
            surgery.v_t2_end,
        )?;
    }

    // Original edge no longer referenced by any face loop — drop it.
    // Original endpoints: corner-shared vertices stay alive for the
    // apex-sphere step (`fillet::apply_apex_sphere_corner`) to remove
    // once it has stitched the sphere face into the shell. Non-corner
    // endpoints are unreferenced after the splice and drop here as
    // before.
    model.edges.remove(surgery.original_edge);
    if !surgery.original_v0_corner_shared {
        model.vertices.remove(surgery.original_v0);
    }
    if !surgery.original_v1_corner_shared {
        model.vertices.remove(surgery.original_v1);
    }

    Ok(())
}

/// Walk the outer shell of `solid_id` and collect every face whose
/// outer loop references `vertex` and that is not in `exclude`.
///
/// Shared by [`find_third_face_at_vertex`] (which requires exactly one
/// match to close a single-edge fillet cap) and
/// [`third_face_candidate_count`] (the read-only pre-flight the all-edges
/// graceful skip uses to detect a termination surgery could not close).
pub(crate) fn outer_shell_faces_at_vertex(
    model: &BRepModel,
    solid_id: SolidId,
    vertex: VertexId,
    exclude: &[FaceId],
) -> OperationResult<Vec<FaceId>> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Solid {} not found", solid_id)))?;
    let shell = model.shells.get(solid.outer_shell).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("Outer shell {} not found", solid.outer_shell))
    })?;

    let mut matches = Vec::with_capacity(2);
    for &face_id in &shell.faces {
        if exclude.contains(&face_id) {
            continue;
        }
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {} missing", face_id)))?;
        let lp = model.loops.get(face.outer_loop).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "Loop {} missing on face {}",
                face.outer_loop, face_id
            ))
        })?;
        for &edge_id in &lp.edges {
            let edge = model.edges.get(edge_id).ok_or_else(|| {
                OperationError::InvalidGeometry(format!("Edge {} missing", edge_id))
            })?;
            if edge.start_vertex == vertex || edge.end_vertex == vertex {
                matches.push(face_id);
                break;
            }
        }
    }
    Ok(matches)
}

/// Count the outer-shell faces incident to `vertex` other than those in
/// `exclude` (typically a blend edge's two adjacent faces). Used by the
/// all-edges graceful skip to decide, WITHOUT mutating topology, whether a
/// single-edge fillet termination at `vertex` could be closed by surgery:
/// [`find_third_face_at_vertex`] succeeds iff this count is exactly `1`
/// (a clean 3-valent corner). Any other count — `0` ("no perpendicular
/// face") or `≥2` (higher-valence, unsynthesised) — is a surgery crash the
/// pre-filter must pre-empt by skipping the edge.
pub(crate) fn third_face_candidate_count(
    model: &BRepModel,
    solid_id: SolidId,
    vertex: VertexId,
    exclude: &[FaceId],
) -> OperationResult<usize> {
    Ok(outer_shell_faces_at_vertex(model, solid_id, vertex, exclude)?.len())
}

/// Walk the outer shell of `solid_id` and return the first face that
/// references `vertex` in its outer loop and is not in `exclude`.
///
/// In a 3-valent corner exactly one such face exists; if zero or two+
/// match we error out rather than guess.
fn find_third_face_at_vertex(
    model: &BRepModel,
    solid_id: SolidId,
    vertex: VertexId,
    exclude: &[FaceId],
) -> OperationResult<FaceId> {
    let matches = outer_shell_faces_at_vertex(model, solid_id, vertex, exclude)?;

    match matches.as_slice() {
        [only] => Ok(*only),
        [] => Err(OperationError::InvalidGeometry(format!(
            "No face perpendicular to blend at vertex {}; \
             surgery requires a 3-valent corner",
            vertex
        ))),
        many => Err(OperationError::NotImplemented(format!(
            "Vertex {} has {} non-blend faces ({:?}); \
             higher-valence corner blends require corner-patch synthesis",
            vertex,
            many.len(),
            many
        ))),
    }
}

/// Replace `old_edge` with `new_edge` in `face_id`'s outer loop and
/// re-vertex the loop neighbours so loop continuity is preserved.
///
/// Pre-conditions
/// * `face_id`'s outer loop contains `old_edge` exactly once.
/// * `old_v0`/`old_v1` are the endpoints of `old_edge`.
/// * `new_edge` was created with vertex pair `(new_v0, new_v1)` going
///   in the same canonical direction as `old_edge` did
///   (`old_v0 → old_v1` ⇒ `new_v0 → new_v1`). The function preserves
///   the loop's orientation flag at the splice site, so passing a
///   `new_edge` whose forward direction matches `old_edge`'s gives the
///   correct loop traversal.
fn splice_face_along_edge(
    model: &mut BRepModel,
    face_id: FaceId,
    old_edge: EdgeId,
    new_edge: EdgeId,
    old_v0: VertexId,
    old_v1: VertexId,
    new_v0: VertexId,
    new_v1: VertexId,
    old_v0_corner_shared: bool,
    old_v1_corner_shared: bool,
) -> OperationResult<()> {
    let loop_id = {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {} missing", face_id)))?;
        face.outer_loop
    };

    // Locate old_edge in the loop.
    let (idx, old_orient, pred_edge, pred_orient, succ_edge, succ_orient) = {
        let lp = model
            .loops
            .get(loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry(format!("Loop {} missing", loop_id)))?;
        let n = lp.edges.len();
        if n < 3 {
            return Err(OperationError::InvalidGeometry(format!(
                "Face {} loop has only {} edges; blend surgery needs ≥3",
                face_id, n
            )));
        }
        let idx = lp
            .edges
            .iter()
            .position(|&e| e == old_edge)
            .ok_or_else(|| {
                OperationError::InvalidGeometry(format!(
                    "Face {} outer loop does not contain edge {}",
                    face_id, old_edge
                ))
            })?;
        let pred_idx = (idx + n - 1) % n;
        let succ_idx = (idx + 1) % n;
        (
            idx,
            lp.orientations[idx],
            lp.edges[pred_idx],
            lp.orientations[pred_idx],
            lp.edges[succ_idx],
            lp.orientations[succ_idx],
        )
    };

    // Determine which original endpoint sits between pred_edge and
    // old_edge versus between old_edge and succ_edge.
    //
    // Loop traversal of old_edge in old_orient direction:
    //   pred → (entry_vertex) → old_edge → (exit_vertex) → succ
    //   forward old_orient: entry = old_v0, exit = old_v1
    //   backward old_orient: entry = old_v1, exit = old_v0
    let (entry_old, exit_old, entry_new, exit_new, entry_corner_shared, exit_corner_shared) =
        if old_orient {
            (
                old_v0,
                old_v1,
                new_v0,
                new_v1,
                old_v0_corner_shared,
                old_v1_corner_shared,
            )
        } else {
            (
                old_v1,
                old_v0,
                new_v1,
                new_v0,
                old_v1_corner_shared,
                old_v0_corner_shared,
            )
        };

    // F5-α.2: at a corner-shared terminal, `pred_edge` / `succ_edge`
    // are themselves blend edges scheduled for splice in this pass.
    // Rewiring them pre-emptively either no-ops (their corner endpoint
    // has already been dedup-merged onto the cap-arc P_a/P_b/P_c by
    // `VertexStore::add_or_find`) or actively breaks the next splice's
    // `current_at_terminal == expected` guard. Skip the rewire on the
    // corner-shared side; the sibling fillet's own splice will handle
    // the trim curve's vertex contract from its own face's perspective.
    //
    // Re-vertex pred_edge: its trailing endpoint (in pred_orient) was
    // entry_old; rewrite it to entry_new (unless entry is corner-shared).
    if !entry_corner_shared {
        rewire_edge_vertex(model, pred_edge, entry_old, entry_new, true, pred_orient)?;
    }
    // Re-vertex succ_edge: its leading endpoint (in succ_orient) was
    // exit_old; rewrite it to exit_new (unless exit is corner-shared).
    if !exit_corner_shared {
        rewire_edge_vertex(model, succ_edge, exit_old, exit_new, false, succ_orient)?;
    }

    // Replace old_edge with new_edge at the splice site, preserving
    // the orientation flag so loop traversal still goes
    // (entry_new) → (exit_new).
    {
        let lp = model
            .loops
            .get_mut(loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry(format!("Loop {} missing", loop_id)))?;
        lp.edges[idx] = new_edge;
        // orientations[idx] is already correct by construction of
        // new_edge: blend producers always create new_edge as
        // new_v0 → new_v1, mirroring old_edge's canonical direction.
    }

    Ok(())
}

/// Rewire one endpoint of an edge from `expected` to `replacement`,
/// **and re-trim the edge's parameter range** so the underlying
/// curve geometry actually terminates at the new vertex's coordinate.
///
/// `which_terminal` selects which loop-traversal terminal of the
/// edge we're touching (`true` = "trailing/end-of-traversal";
/// `false` = "leading/start-of-traversal"). The actual struct field
/// (`start_vertex` vs `end_vertex`) is then chosen by combining
/// `which_terminal` with the edge's orientation in the loop.
///
/// ## Why we re-trim `param_range`
///
/// `Edge` stores `(curve_id, param_range, start_vertex, end_vertex)`.
/// The vertex IDs are labels into the vertex store; the *geometric*
/// trace of the edge — what tessellation samples — is
/// `curve.evaluate(t)` for `t ∈ param_range`.
///
/// Swapping vertex IDs alone leaves the curve and its parameter range
/// unchanged, so the tessellator still draws the **original** segment
/// from corner to corner even though the topology now claims the edge
/// terminates at the trim point. Visually: the original sharp edge
/// persists alongside the new fillet face. We project the replacement
/// vertex's 3D coordinate onto the curve via `Curve::closest_point`
/// and update the corresponding endpoint of `param_range`.
fn rewire_edge_vertex(
    model: &mut BRepModel,
    edge_id: EdgeId,
    expected: VertexId,
    replacement: VertexId,
    which_terminal_is_trailing: bool,
    edge_loop_orient: bool,
) -> OperationResult<()> {
    // Snapshot the edge's curve + orientation + current range BEFORE we
    // mutate, so the curve projection runs against the unchanged geometry.
    let (curve_id, edge_orientation, mut param_range, current_at_terminal) = {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry(format!("Edge {} missing", edge_id)))?;
        // Pick the struct field:
        //   trailing terminal  + forward orient → end_vertex
        //   trailing terminal  + backward orient → start_vertex
        //   leading terminal   + forward orient → start_vertex
        //   leading terminal   + backward orient → end_vertex
        let touch_end = which_terminal_is_trailing == edge_loop_orient;
        let current = if touch_end {
            edge.end_vertex
        } else {
            edge.start_vertex
        };
        (edge.curve_id, edge.orientation, edge.param_range, current)
    };

    if current_at_terminal != expected {
        return Err(OperationError::InvalidGeometry(format!(
            "Edge {} terminal mismatch during blend splice: expected vertex {}, found {}",
            edge_id, expected, current_at_terminal
        )));
    }

    let touch_end = which_terminal_is_trailing == edge_loop_orient;

    // Resolve the replacement vertex's world-space coordinate.
    let replacement_position = {
        let v = model.vertices.get(replacement).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "Replacement vertex {} missing during blend splice",
                replacement
            ))
        })?;
        crate::math::Point3::new(v.position[0], v.position[1], v.position[2])
    };

    // Project the new vertex onto the underlying curve to find the new
    // parameter. For straight-line edges this is exact; for NURBS / arc
    // curves it uses each curve type's closest_point implementation.
    let new_param = {
        let curve = model.curves.get(curve_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "Edge {} references missing curve {}",
                edge_id, curve_id
            ))
        })?;
        let (t, _projected) = curve
            .closest_point(&replacement_position, Tolerance::default())
            .map_err(|e| {
                OperationError::InvalidGeometry(format!(
                    "Curve {} closest_point failed during blend retrim: {:?}",
                    curve_id, e
                ))
            })?;
        t
    };

    // Decide which endpoint of `param_range` corresponds to the vertex
    // we just rewired:
    //   Forward  + touch_end   → end_vertex   ↔ param_range.end
    //   Forward  + touch_start → start_vertex ↔ param_range.start
    //   Backward + touch_end   → end_vertex   ↔ param_range.start
    //   Backward + touch_start → start_vertex ↔ param_range.end
    let updates_high_param = match (touch_end, edge_orientation) {
        (true, EdgeOrientation::Forward) => true,
        (false, EdgeOrientation::Forward) => false,
        (true, EdgeOrientation::Backward) => false,
        (false, EdgeOrientation::Backward) => true,
    };
    if updates_high_param {
        param_range.end = new_param;
    } else {
        param_range.start = new_param;
    }
    // Maintain the ParameterRange invariant `start <= end`. A blend
    // trim that lands on the wrong side of the surviving endpoint is a
    // geometric error, not just a numerical hiccup, so we surface it
    // explicitly.
    if param_range.start > param_range.end {
        return Err(OperationError::InvalidGeometry(format!(
            "Edge {} retrim produced inverted parameter range \
             [{}, {}] after projecting vertex {} onto curve {}",
            edge_id, param_range.start, param_range.end, replacement, curve_id
        )));
    }
    let new_range = ParameterRange::new(param_range.start, param_range.end);

    // Apply the mutation: vertex ID, parameter range, invalidate cache.
    let edge = model
        .edges
        .get_mut(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Edge {} missing", edge_id)))?;
    if touch_end {
        edge.end_vertex = replacement;
    } else {
        edge.start_vertex = replacement;
    }
    edge.param_range = new_range;
    edge.invalidate_length_cache();
    Ok(())
}

/// Insert `cap_edge` into `face_id`'s outer loop at the unique site
/// where the loop has just opened up between `vertex_a` and `vertex_b`.
///
/// After `splice_face_along_edge` has run on F1 and F2, the
/// perpendicular face F3 (or F4) has two consecutive loop edges whose
/// shared traversal vertex no longer matches: one edge ends at
/// `vertex_a`, the next starts at `vertex_b` (or vice versa). We find
/// that gap, work out which orientation closes it, and insert the cap.
fn insert_cap_into_face_loop(
    model: &mut BRepModel,
    face_id: FaceId,
    cap_edge: EdgeId,
    vertex_a: VertexId,
    vertex_b: VertexId,
) -> OperationResult<()> {
    let loop_id = {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {} missing", face_id)))?;
        face.outer_loop
    };

    let (cap_start, cap_end) = {
        let cap = model.edges.get(cap_edge).ok_or_else(|| {
            OperationError::InvalidGeometry(format!("Cap edge {} missing", cap_edge))
        })?;
        (cap.start_vertex, cap.end_vertex)
    };

    // Find the discontinuity site: consecutive loop entries (i, i+1)
    // where the trailing-traversal vertex of i and the leading-
    // traversal vertex of i+1 are the (vertex_a, vertex_b) pair.
    let (insert_at, cap_orient) = {
        let lp = model
            .loops
            .get(loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry(format!("Loop {} missing", loop_id)))?;
        let n = lp.edges.len();
        if n < 3 {
            return Err(OperationError::InvalidGeometry(format!(
                "Face {} loop has only {} edges; blend cap insertion needs ≥3",
                face_id, n
            )));
        }
        let mut found: Option<(usize, bool)> = None;
        for i in 0..n {
            let j = (i + 1) % n;
            let trailing_i = traversal_terminal(model, lp.edges[i], lp.orientations[i], true)?;
            let leading_j = traversal_terminal(model, lp.edges[j], lp.orientations[j], false)?;

            let matches_forward = trailing_i == cap_start && leading_j == cap_end;
            let matches_backward = trailing_i == cap_end && leading_j == cap_start;
            let matches_pair_a = trailing_i == vertex_a && leading_j == vertex_b;
            let matches_pair_b = trailing_i == vertex_b && leading_j == vertex_a;

            if (matches_pair_a || matches_pair_b) && (matches_forward || matches_backward) {
                // Insertion site found. Insert cap with the orientation
                // that takes us from trailing_i to leading_j.
                let cap_orient = matches_forward;
                found = Some((j, cap_orient));
                break;
            }
        }
        found.ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "Face {}: no discontinuity site for cap edge {} between vertices {} and {}",
                face_id, cap_edge, vertex_a, vertex_b
            ))
        })?
    };

    let lp = model
        .loops
        .get_mut(loop_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Loop {} missing", loop_id)))?;
    // `insert_at` is the index of the post-gap edge; inserting at that
    // index pushes the post-gap edge one slot to the right and places
    // the cap in between. Wrap-around (insert_at == 0) means the gap is
    // between the last edge and the first; inserting at 0 puts the cap
    // at the start, which still closes the loop because loops are
    // cyclic.
    lp.insert_edge(insert_at, cap_edge, cap_orient);

    Ok(())
}

/// Return the loop-traversal endpoint of an edge.
///
/// `trailing == true`  → vertex visited as the loop leaves the edge.
/// `trailing == false` → vertex visited as the loop enters the edge.
fn traversal_terminal(
    model: &BRepModel,
    edge_id: EdgeId,
    forward: bool,
    trailing: bool,
) -> OperationResult<VertexId> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Edge {} missing", edge_id)))?;
    Ok(match (forward, trailing) {
        (true, true) => edge.end_vertex,
        (true, false) => edge.start_vertex,
        (false, true) => edge.start_vertex,
        (false, false) => edge.end_vertex,
    })
}

#[cfg(test)]
mod surgery_tests {
    use super::*;

    /// Build a synthetic `BlendEdgeSurgery` with arbitrary IDs. The
    /// struct is pure data — these tests verify field-derived helpers
    /// (`to_trim_curves`) without spinning up a `BRepModel`. The
    /// `validate_surgery` and `splice_blend_edge` integration paths
    /// are covered by the kernel-workflow regression suite and the
    /// fillet/chamfer dihedral matrix.
    fn synthetic_surgery() -> BlendEdgeSurgery {
        BlendEdgeSurgery {
            original_edge: 10,
            original_v0: 1,
            original_v1: 2,
            face1: 100,
            face2: 200,
            trim1_edge: 11,
            trim2_edge: 12,
            trim1_curve: 7000,
            trim2_curve: 7001,
            cap_v0_edge: 13,
            cap_v1_edge: 14,
            v_t1_start: 3,
            v_t1_end: 4,
            v_t2_start: 5,
            v_t2_end: 6,
            original_v0_corner_shared: false,
            original_v1_corner_shared: false,
        }
    }

    #[test]
    fn to_trim_curves_emits_one_per_face_with_correct_curve() {
        let surgery = synthetic_surgery();
        let trims = surgery.to_trim_curves();
        assert_eq!(trims.len(), 2);

        assert_eq!(trims[0].curve_id, 7000);
        assert_eq!(trims[0].on_face, 100);
        assert_eq!(trims[0].side, TrimSide::Discard);
        assert!(trims[0].is_full_range());

        assert_eq!(trims[1].curve_id, 7001);
        assert_eq!(trims[1].on_face, 200);
        assert_eq!(trims[1].side, TrimSide::Discard);
        assert!(trims[1].is_full_range());
    }

    #[test]
    fn to_trim_curves_uses_full_range_marker() {
        // Rail trims span exactly the trim arc — no entry/exit
        // sub-ranges to enumerate. `full_range` records that
        // unambiguously by leaving `ranges` empty.
        let surgery = synthetic_surgery();
        let trims = surgery.to_trim_curves();
        for trim in &trims {
            assert!(trim.is_full_range());
            assert_eq!(trim.ranges.len(), 0);
            // covered_span() is 0 for full-range trims by carrier
            // contract — callers must consult the curve directly.
            assert_eq!(trim.covered_span(), 0.0);
        }
    }

    #[test]
    fn to_trim_curves_preserves_face_curve_pairing() {
        // Swap face IDs and confirm the helper still pairs each
        // curve with the correct face — guards against a future
        // refactor accidentally crossing the wires.
        let mut surgery = synthetic_surgery();
        surgery.face1 = 999;
        surgery.face2 = 888;
        let trims = surgery.to_trim_curves();
        assert_eq!(trims[0].on_face, 999);
        assert_eq!(trims[1].on_face, 888);
        // Curves stay associated with their original face slot.
        assert_eq!(trims[0].curve_id, 7000);
        assert_eq!(trims[1].curve_id, 7001);
    }
}

/// Coalesce maximal chains of co-curve, contiguous, smoothly-joined edges that
/// meet at 2-valent (in-solid) vertices into single edges, IN PLACE.
///
/// A boolean can split one smooth curve — most commonly a drilled hole's rim
/// circle — into several co-curve arcs joined at 2-valent smooth vertices.
/// Blending that arc CHAIN per-edge cannot coordinate the shared adjacent
/// faces: one arc's splice removes a sibling arc's edge that the sibling's
/// splice still needs ("Face N outer loop does not contain edge M"), and the
/// blend pre-flight reads the 2-valent joints as shared CORNER vertices and
/// refuses. Merging the arcs back into the single canonical edge — for a
/// closed rim, the standard cylinder-lateral closed-circle edge — restores the
/// representation the closed-rim blend (fillet torus / chamfer cone) already
/// handles soundly.
///
/// Returns a map from each removed edge id to its surviving edge id so the
/// caller can remap any selection that referenced a merged-away arc. Idempotent
/// and a no-op on already-canonical topology (clean boxes, prisms, single-edge
/// rims), so it is safe to run before every fillet AND every chamfer. Shared by
/// both blend entry points (`fillet::fillet_edges`,
/// `chamfer::chamfer_edges`) so the healing stays byte-identical across kinds.
pub(crate) fn coalesce_smooth_cocurve_chains(
    model: &mut BRepModel,
    solid_id: SolidId,
) -> HashMap<EdgeId, EdgeId> {
    let mut merged: HashMap<EdgeId, EdgeId> = HashMap::new();
    loop {
        // In-solid edges = referenced by some face loop of `solid_id`.
        let mut in_solid: HashSet<EdgeId> = HashSet::new();
        if let Some(sol) = model.solids.get(solid_id) {
            let mut shells = vec![sol.outer_shell];
            shells.extend_from_slice(&sol.inner_shells);
            for sh in shells {
                let Some(shell) = model.shells.get(sh) else {
                    continue;
                };
                for &fid in &shell.faces {
                    let Some(f) = model.faces.get(fid) else {
                        continue;
                    };
                    for lid in f.all_loops() {
                        if let Some(lp) = model.loops.get(lid) {
                            for &e in &lp.edges {
                                in_solid.insert(e);
                            }
                        }
                    }
                }
            }
        }
        let mut ve: HashMap<VertexId, Vec<EdgeId>> = HashMap::new();
        for &e in &in_solid {
            if let Some(ed) = model.edges.get(e) {
                ve.entry(ed.start_vertex).or_default().push(e);
                if ed.end_vertex != ed.start_vertex {
                    ve.entry(ed.end_vertex).or_default().push(e);
                }
            }
        }
        // Find a 2-valent vertex whose two edges are co-curve and forward-
        // contiguous (one ends at v, the other starts at v, at the same param).
        let mut act = None;
        for (&v, es) in &ve {
            if es.len() != 2 {
                continue;
            }
            let (a, b) = (es[0], es[1]);
            let (ea, eb) = match (model.edges.get(a), model.edges.get(b)) {
                (Some(ea), Some(eb)) => (ea, eb),
                _ => continue,
            };
            if ea.curve_id != eb.curve_id {
                continue;
            }
            let (keep, drop) = if ea.end_vertex == v && eb.start_vertex == v {
                (a, b)
            } else if eb.end_vertex == v && ea.start_vertex == v {
                (b, a)
            } else {
                continue;
            };
            let (ek, ed) = match (model.edges.get(keep), model.edges.get(drop)) {
                (Some(ek), Some(ed)) => (ek, ed),
                _ => continue,
            };
            if (ek.param_range.end - ed.param_range.start).abs() > 1.0e-9 {
                continue;
            }
            act = Some((v, keep, drop, ed.end_vertex, ed.param_range.end));
            break;
        }
        let Some((v, keep, drop, drop_end_v, drop_end_p)) = act else {
            break;
        };
        if let Some(ek) = model.edges.get_mut(keep) {
            ek.end_vertex = drop_end_v;
            ek.param_range.end = drop_end_p;
        }
        for lid in (0..model.loops.len() as u32).map(LoopId::from) {
            if let Some(lp) = model.loops.get_mut(lid) {
                if let Some(idx) = lp.edges.iter().position(|&e| e == drop) {
                    lp.remove_edge(idx);
                }
            }
        }
        model.edges.remove(drop);
        model.vertices.remove(v);
        merged.insert(drop, keep);
    }
    merged
}

/// Resolve an edge id through the coalesce map to its final surviving edge.
pub(crate) fn resolve_coalesced(map: &HashMap<EdgeId, EdgeId>, mut e: EdgeId) -> EdgeId {
    let mut guard = 0;
    while let Some(&next) = map.get(&e) {
        e = next;
        guard += 1;
        if guard > 1024 {
            break;
        }
    }
    e
}

/// Sanity helper used by tests / callers to check a loop is closed.
#[allow(dead_code)]
pub(crate) fn loop_is_closed(model: &BRepModel, loop_id: LoopId) -> OperationResult<bool> {
    let lp = model
        .loops
        .get(loop_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Loop {} missing", loop_id)))?;
    let n = lp.edges.len();
    if n == 0 {
        return Ok(false);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let trailing = traversal_terminal(model, lp.edges[i], lp.orientations[i], true)?;
        let leading = traversal_terminal(model, lp.edges[j], lp.orientations[j], false)?;
        if trailing != leading {
            return Ok(false);
        }
    }
    Ok(true)
}
