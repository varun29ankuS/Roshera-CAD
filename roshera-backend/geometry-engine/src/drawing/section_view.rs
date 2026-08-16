//! Section (cross-section) views for engineering drawings.
//!
//! A section view cuts the solid on a plane and draws the resulting filled
//! cross-section: the cut OUTLINE (boundary of the sectioned material) plus
//! 45° HATCHING over the solid region — the ISO 128 convention that
//! distinguishes "cut solid material" from voids/holes. Unlike the projected
//! orthographic views, a section reveals INTERNAL features (bores, counterbores,
//! webs) that hidden lines only hint at.
//!
//! The cut itself reuses the kernel's [`section_solid_by_plane`], which returns
//! the cross-section as triangulated [`SectionCap`]s lying on the plane. We
//! project those into the plane's own 2D frame, trace the boundary for the
//! outline, and scan-fill the triangles for the hatch.

use std::collections::{HashMap, HashSet};

use crate::math::{Point3, Tolerance, Vector3};
use crate::operations::section::{section_solid_by_plane, SectionCap};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::Cylinder;
use crate::primitives::topology_builder::BRepModel;

use super::centerlines::{dedup_centerlines, extend_segment, face_axial_extent, Centerline};
use super::projection::DEFAULT_CURVE_SAMPLES;
use super::types::{
    Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, ViewExtent, ViewSource,
};

/// Hatch line spacing in model units (pre-scale). Lands at a sensible on-sheet
/// pitch (~2–3 mm) for the small/medium parts the drawing module targets.
///
/// `pub(crate)`: the D1 harness invariant in `drawing/verify.rs`
/// (`section_shows_only_hatch`) reuses this exact constant as its tolerance
/// margin so the check stays honest about what "beyond the hatched footprint"
/// means in THIS renderer, rather than duplicating the literal.
pub(crate) const HATCH_SPACING: f64 = 4.0;

/// Build a SECTION view: cut `solid_id` on the plane `(origin, normal)` and
/// return a [`ProjectedView`] whose polylines are the cut outline plus 45°
/// hatching, placed at `pos` (sheet mm) with the given `scale`.
///
/// The view's 2D frame is the plane's in-plane basis `(u, v)` with `u =
/// normal.perpendicular()`; world point `p` maps to `(p−origin)·u, (p−origin)·v`.
/// Returns `Ok(None)` when the plane misses the solid (no material to section).
pub fn section_view(
    model: &BRepModel,
    solid_id: SolidId,
    part_id: uuid::Uuid,
    plane_origin: Point3,
    plane_normal: Vector3,
    name: &str,
    pos: [f64; 2],
    scale: f64,
) -> Option<ProjectedView> {
    let caps = section_solid_by_plane(
        model,
        solid_id,
        plane_origin,
        plane_normal,
        Tolerance::default(),
    )
    .ok()?;
    if caps.is_empty() {
        return None;
    }

    let n = plane_normal.normalize().ok()?;
    // In-plane frame (u = view right, v = view up). Keep the WORLD-VERTICAL
    // axis vertical on paper: v = world-Z projected into the plane. The
    // previous arbitrary `n.perpendicular()` frame could land the part's
    // long axis VERTICAL in the view — a tall sliver that starves the
    // layout solver of scale (live ring-plate sheet: 10% utilization with
    // a 12×60 upright section in the ISO slot).
    //
    // Orientation invariant (load-bearing for the cutting-plane arrows):
    // u × v = n, i.e. n points OUT of the drawn section toward its viewer,
    // so SECTION A-A is what you see looking along −n. With v ⊥ n unit,
    // u = v × n gives u × v = (v×n)×v = n(v·v) − v(v·n) = n. ✓
    let v = {
        let world_up = Vector3::new(0.0, 0.0, 1.0);
        let proj = world_up - n * n.dot(&world_up);
        match proj.normalize() {
            Ok(p) => p,
            Err(_) => {
                // n ∥ Z (horizontal section plane): world Y plays "up".
                let alt = Vector3::new(0.0, 1.0, 0.0);
                let p = alt - n * n.dot(&alt);
                p.normalize().ok()?
            }
        }
    };
    let u = v.cross(&n);
    let to2d = |p: Point3| -> [f64; 2] {
        let d = p - plane_origin;
        [d.dot(&u), d.dot(&v)]
    };

    // Project every cap's triangles into the plane frame (2D), keeping triangle
    // connectivity for the hatch scan-fill and counting undirected edges for the
    // boundary outline.
    let mut tris2d: Vec<[[f64; 2]; 3]> = Vec::new();
    let mut edge_count: std::collections::HashMap<(i64, i64, i64, i64), ([f64; 2], [f64; 2], u32)> =
        std::collections::HashMap::new();
    let q = |a: f64| (a * 1e4).round() as i64;
    for cap in &caps {
        push_cap(cap, &to2d, &mut tris2d, &mut edge_count, &q);
    }
    if tris2d.is_empty() {
        return None;
    }

    // The cut OUTLINE (boundary of sectioned material) and the 45° HATCH
    // (material texture) are kept in SEPARATE vecs (campaign #55 Slice 1): the
    // outline is geometry a readback can answer about; the hatch is *evidence
    // of material*, answered as such, never as geometry.
    let mut polylines: Vec<Polyline2d> = Vec::new();
    let mut hatch: Vec<Polyline2d> = Vec::new();
    // Outline = edges used by exactly one triangle (the section boundary).
    for (a, b, c) in edge_count.values() {
        if *c == 1 {
            polylines.push(Polyline2d::from_points(vec![*a, *b]));
        }
    }

    // 2D extent of the CUT ITSELF (the sectioned triangles only) — this
    // deliberately stays scoped to the material actually cut, because it
    // also bounds the 45° hatch sweep below; widening it to the back-of-
    // plane outline would waste sweep lines outside any triangle (harmless,
    // since `tri_hatch_span` still filters against `tris2d`, but pointless).
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for t in &tris2d {
        for p in t {
            min_x = min_x.min(p[0]);
            min_y = min_y.min(p[1]);
            max_x = max_x.max(p[0]);
            max_y = max_y.max(p[1]);
        }
    }

    // D1 (2026-08-16 finding): the cut cross-section alone reads as
    // disconnected hatched confetti — nothing ties the bands together into a
    // part. Add the REAL geometry a section reveals beyond the cut: every
    // solid edge behind the plane (the far bore wall, the far side of a
    // bolt hole, the outer profile), clipped at the plane. See
    // `back_of_plane_outline`'s doc for what this deliberately does not
    // attempt (occlusion among the kept material itself).
    polylines.extend(back_of_plane_outline(
        model,
        solid_id,
        plane_origin,
        n,
        u,
        v,
    ));
    let centerlines = section_centerlines(model, solid_id, plane_origin, n, u, v);

    // The view's DECLARED extent additionally folds in the back-of-plane
    // OUTLINE just added, so nothing the caller places on the sheet is
    // silently clipped out of the view's own reported bounds (the hatch
    // sweep above already ran against the tighter tris2d-only range).
    //
    // Deliberately NOT folding in `centerlines`: a chain-line's endpoints
    // are `extend_segment`'s ISO-128 OVERSHOOT past the feature it marks —
    // annotation, not physical cross-section geometry. This view's extent
    // is a measurement of the cut ("the section spans 60×12"), read by
    // `ring_plate_section_shows_bore_voids` and by the layout solver; a
    // section through a feature whose axis runs parallel to `v` (as every
    // bore here does) would otherwise report itself several mm taller than
    // the material actually is, purely because of a drafting-convention
    // annotation extending past it. Confirmed empirically on the ring-plate
    // fixture: folding centerlines in reported 12mm of true thickness as
    // 17.4mm (0 − 2.7 to 12 + 2.7, exactly `extend_segment`'s
    // `len·0.1 + 1.5` overshoot at each end of a 12mm span).
    let (mut ext_min_x, mut ext_min_y, mut ext_max_x, mut ext_max_y) = (min_x, min_y, max_x, max_y);
    for pl in &polylines {
        for p in &pl.points {
            ext_min_x = ext_min_x.min(p[0]);
            ext_min_y = ext_min_y.min(p[1]);
            ext_max_x = ext_max_x.max(p[0]);
            ext_max_y = ext_max_y.max(p[1]);
        }
    }

    // 45° hatch (direction (1,1)): lines of constant c = y − x. For each line,
    // clip against every triangle and draw the covered intervals — using the
    // triangles (not even-odd) so holes/bores stay correctly UN-hatched.
    let c_lo = min_y - max_x;
    let c_hi = max_y - min_x;
    let mut c = (c_lo / HATCH_SPACING).ceil() * HATCH_SPACING;
    while c < c_hi {
        let mut spans: Vec<(f64, f64)> = Vec::new();
        for t in &tris2d {
            if let Some((s0, s1)) = tri_hatch_span(t, c) {
                spans.push((s0.min(s1), s0.max(s1)));
            }
        }
        // Merge overlapping spans (parametrised by x along the line y = x + c).
        spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut cur: Option<(f64, f64)> = None;
        for (s0, s1) in spans {
            match cur {
                Some((b0, e1)) if s0 <= e1 + 1e-9 => {
                    cur = Some((b0, e1.max(s1)));
                }
                Some((b0, b1)) => {
                    hatch.push(Polyline2d::from_points(vec![[b0, b0 + c], [b1, b1 + c]]));
                    cur = Some((s0, s1));
                }
                None => cur = Some((s0, s1)),
            }
        }
        if let Some((b0, b1)) = cur {
            hatch.push(Polyline2d::from_points(vec![[b0, b0 + c], [b1, b1 + c]]));
        }
        c += HATCH_SPACING;
    }

    Some(ProjectedView {
        id: ProjectedViewId::new(),
        name: name.to_string(),
        projection: ProjectionType::Custom {
            rotation: [u.x, u.y, u.z, v.x, v.y, v.z, n.x, n.y, n.z],
        },
        source: ViewSource::Part { part_id, solid_id },
        position_mm: pos,
        scale,
        polylines,
        extent: ViewExtent {
            min_x: ext_min_x,
            min_y: ext_min_y,
            max_x: ext_max_x,
            max_y: ext_max_y,
        },
        dimensions: Vec::new(),
        centerlines,
        hidden_polylines: Vec::new(),
        circles: Vec::new(),
        hidden_circles: Vec::new(),
        ellipses: Vec::new(),
        hidden_ellipses: Vec::new(),
        shaded_raster: None,
        hatch_polylines: hatch,
        // The section outline carries no per-edge provenance (`SectionCap` has
        // no per-face lineage — Slice 1-2 finding); empty → readback refuses.
        polyline_sources: Vec::new(),
    })
}

#[allow(clippy::type_complexity)]
fn push_cap(
    cap: &SectionCap,
    to2d: &impl Fn(Point3) -> [f64; 2],
    tris2d: &mut Vec<[[f64; 2]; 3]>,
    edge_count: &mut std::collections::HashMap<(i64, i64, i64, i64), ([f64; 2], [f64; 2], u32)>,
    q: &impl Fn(f64) -> i64,
) {
    for idx in &cap.indices {
        let p: [[f64; 2]; 3] = [
            to2d(cap.vertices[idx[0] as usize]),
            to2d(cap.vertices[idx[1] as usize]),
            to2d(cap.vertices[idx[2] as usize]),
        ];
        tris2d.push(p);
        for k in 0..3 {
            let a = p[k];
            let b = p[(k + 1) % 3];
            let (ka, kb) = ((q(a[0]), q(a[1])), (q(b[0]), q(b[1])));
            let key = if ka <= kb {
                (ka.0, ka.1, kb.0, kb.1)
            } else {
                (kb.0, kb.1, ka.0, ka.1)
            };
            edge_count
                .entry(key)
                .and_modify(|e| e.2 += 1)
                .or_insert((a, b, 1));
        }
    }
}

/// Intersect the line `y = x + c` with triangle `t`; return the x-span
/// `[x0, x1]` of the covered segment, or `None` if the line misses.
fn tri_hatch_span(t: &[[f64; 2]; 3], c: f64) -> Option<(f64, f64)> {
    // f(p) = p.y − p.x − c; the line is f = 0. Collect crossings on the edges.
    let f = |p: &[f64; 2]| p[1] - p[0] - c;
    let fv = [f(&t[0]), f(&t[1]), f(&t[2])];
    let mut xs: Vec<f64> = Vec::with_capacity(2);
    for k in 0..3 {
        let (a, b) = (t[k], t[(k + 1) % 3]);
        let (fa, fb) = (fv[k], fv[(k + 1) % 3]);
        if (fa <= 0.0 && fb > 0.0) || (fa > 0.0 && fb <= 0.0) {
            let s = fa / (fa - fb);
            xs.push(a[0] + s * (b[0] - a[0]));
        }
    }
    if xs.len() == 2 {
        Some((xs[0], xs[1]))
    } else {
        None
    }
}

/// The wireframe of `solid_id`'s edges that lie BEHIND the cutting plane —
/// the geometry a real section shows beyond the hatch (D1, 2026-08-16
/// finding): the far bore wall, the far side of a bolt hole, the outer
/// profile. Sampled through the SAME `(plane_origin, u, v, n)` frame the cut
/// itself uses (never `view_matrix_for_projection`'s `ProjectionType::
/// Custom`, which carries no translation and would misplace everything
/// whenever `plane_origin` is off the world origin).
///
/// **What this deliberately does NOT do**: occlusion among the kept
/// material. Drafting convention omits hidden lines from a section (the
/// cut face is drawn solid, and what is visible through it is drawn solid
/// too), so every kept edge is inked as a plain visible line — a genuinely
/// occluded feature behind the KEPT material (e.g. a blind counterbore
/// tucked behind a boss) would draw solid where a strict hidden-line pass
/// would dash it. Out of scope for this repair; stated here rather than
/// silently assumed away.
///
/// **Approximation**: each edge is sampled at the same cadence
/// `project_solid_edges` uses everywhere else (2 points for a line,
/// [`DEFAULT_CURVE_SAMPLES`] for a curve); a plane crossing between two
/// samples is a single linear interpolation, not a re-intersection of the
/// underlying curve with the plane. Negligible against the sample cadence
/// on every fixture this module targets; never claimed exact.
///
/// **Seam edges are excluded.** A cylinder's lateral face closes its (u, v)
/// parameter rectangle with one seam edge referenced by that face alone
/// (`create_cylinder_topology`'s doc); it sits at the surface's arbitrary
/// `t=0` reference angle, nowhere near the true occluding contour, and
/// would otherwise draw a stray straight line with no drafting meaning. An
/// edge shared by only one distinct face is a seam and is skipped.
fn back_of_plane_outline(
    model: &BRepModel,
    solid_id: SolidId,
    plane_origin: Point3,
    n: Vector3,
    u: Vector3,
    v: Vector3,
) -> Vec<Polyline2d> {
    let Some(solid) = model.solids.get(solid_id) else {
        return Vec::new();
    };
    let mut shell_ids = vec![solid.outer_shell];
    shell_ids.extend_from_slice(&solid.inner_shells);
    shell_ids.extend_from_slice(&solid.peer_shells);

    // Pass 1: which DISTINCT faces reference each edge — an edge referenced
    // by exactly one face is a seam (see doc above).
    let mut edge_faces: HashMap<EdgeId, HashSet<FaceId>> = HashMap::new();
    for sh in &shell_ids {
        let Some(shell) = model.shells.get(*sh) else {
            continue;
        };
        for face_id in &shell.faces {
            let Some(face) = model.faces.get(*face_id) else {
                continue;
            };
            let loop_ids = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
            for loop_id in loop_ids {
                let Some(topo_loop) = model.loops.get(loop_id) else {
                    continue;
                };
                for edge_id in &topo_loop.edges {
                    edge_faces.entry(*edge_id).or_default().insert(*face_id);
                }
            }
        }
    }

    let tol = Tolerance::default().distance();
    let depth = |p: Point3| (p - plane_origin).dot(&n);
    let coord2d = |p: Point3| -> [f64; 2] {
        let d = p - plane_origin;
        [d.dot(&u), d.dot(&v)]
    };
    let keep = |d: f64| d <= tol;

    let mut visited: HashSet<EdgeId> = HashSet::new();
    let mut out: Vec<Polyline2d> = Vec::new();

    for sh in shell_ids {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for face_id in &shell.faces {
            let Some(face) = model.faces.get(*face_id) else {
                continue;
            };
            let loop_ids = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
            for loop_id in loop_ids {
                let Some(topo_loop) = model.loops.get(loop_id) else {
                    continue;
                };
                for edge_id in &topo_loop.edges {
                    if !visited.insert(*edge_id) {
                        continue;
                    }
                    if edge_faces.get(edge_id).map(HashSet::len).unwrap_or(0) <= 1 {
                        continue; // seam edge — see doc.
                    }
                    let Some(edge) = model.edges.get(*edge_id) else {
                        continue;
                    };
                    let Some(curve) = model.curves.get(edge.curve_id) else {
                        continue;
                    };
                    let is_linear = curve.is_linear(Tolerance::default());
                    let n_samples = if is_linear {
                        2
                    } else {
                        DEFAULT_CURVE_SAMPLES.max(2)
                    };
                    let t0 = edge.param_range.start;
                    let t1 = edge.param_range.end;
                    let mut pts3: Vec<Point3> = Vec::with_capacity(n_samples);
                    let mut ok = true;
                    for i in 0..n_samples {
                        let frac = i as f64 / (n_samples - 1) as f64;
                        let t = t0 + (t1 - t0) * frac;
                        match curve.point_at(t) {
                            Ok(p) => pts3.push(p),
                            Err(_) => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok || pts3.len() < 2 {
                        continue;
                    }

                    // Walk consecutive samples, splitting the run at every
                    // depth-zero crossing and keeping only sub-runs behind
                    // the plane — the material a section leaves standing
                    // once the front half is cut away.
                    let depths: Vec<f64> = pts3.iter().map(|&p| depth(p)).collect();
                    let coords: Vec<[f64; 2]> = pts3.iter().map(|&p| coord2d(p)).collect();
                    let mut cur: Vec<[f64; 2]> = Vec::new();
                    for i in 0..pts3.len() {
                        if keep(depths[i]) {
                            cur.push(coords[i]);
                        } else if cur.len() >= 2 {
                            out.push(Polyline2d::from_points(std::mem::take(&mut cur)));
                        } else {
                            cur.clear();
                        }
                        if i + 1 < pts3.len() {
                            let (d0, d1) = (depths[i], depths[i + 1]);
                            if keep(d0) != keep(d1) {
                                let t = d0 / (d0 - d1);
                                let cross = [
                                    coords[i][0] + t * (coords[i + 1][0] - coords[i][0]),
                                    coords[i][1] + t * (coords[i + 1][1] - coords[i][1]),
                                ];
                                cur.push(cross);
                                if keep(d0) {
                                    // The kept run ends exactly at the plane.
                                    out.push(Polyline2d::from_points(std::mem::take(&mut cur)));
                                }
                                // else: the cross point starts a new kept run.
                            }
                        }
                    }
                    if cur.len() >= 2 {
                        out.push(Polyline2d::from_points(cur));
                    }
                }
            }
        }
    }
    out
}

/// Centerlines for `solid_id` through the section's own `(plane_origin, u,
/// v, n)` frame — the same feature `super::centerlines::centerlines`
/// derives for an orthographic view (a chain axis line for a side-on
/// cylindrical feature, a center-mark cross for an end-on one), but mapped
/// through THIS function's own translation-aware projection rather than
/// [`super::projection::view_matrix_for_projection`] (see
/// `back_of_plane_outline`'s doc for why `Custom`'s missing translation
/// disqualifies it here).
///
/// A section's cutting plane is always perpendicular to the dominant bore
/// axis by construction (`choose_section_plane`), so every bore/hole in the
/// part reads side-on here — this is what puts the brief's required
/// "centerline through the bore" on the sheet. Off-plane holes sharing the
/// bore axis direction (e.g. a bolt hole the cut plane does not pass
/// through) also qualify and project to their own axis line; coincident
/// lines collapse via the SAME dedup rule an orthographic view uses.
fn section_centerlines(
    model: &BRepModel,
    solid_id: SolidId,
    plane_origin: Point3,
    n: Vector3,
    u: Vector3,
    v: Vector3,
) -> Vec<Centerline> {
    let Some(solid) = model.solids.get(solid_id) else {
        return Vec::new();
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.extend_from_slice(&solid.peer_shells);

    let to2d = |p: Point3| -> [f64; 2] {
        let d = p - plane_origin;
        [d.dot(&u), d.dot(&v)]
    };

    let mut cands: Vec<Centerline> = Vec::new();
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let Some(surf) = model.surfaces.get(face.surface_id) else {
                continue;
            };
            let Some(cyl) = surf.as_any().downcast_ref::<Cylinder>() else {
                continue;
            };
            let Ok(axis) = cyl.axis.normalize() else {
                continue;
            };
            let (lo, hi) = face_axial_extent(model, face, cyl.origin, axis)
                .or_else(|| cyl.height_limits.map(|h| (h[0], h[1])))
                .unwrap_or((0.0, 0.0));
            if (hi - lo).abs() < 1e-9 {
                continue;
            }

            // How far the axis tilts out of the section's own image plane
            // (perpendicular to `n`, the view depth): 1 → end-on, 0 →
            // side-on, mirroring `centerlines()`'s `out_of_plane` test.
            let out_of_plane = axis.dot(&n).abs();

            let cl = if out_of_plane > 0.966 {
                let mid = cyl.origin + axis * (0.5 * (lo + hi));
                let c = to2d(mid);
                let r = cyl.radius * 1.18;
                Centerline {
                    kind: "center_mark".to_string(),
                    segments: vec![
                        [c[0] - r, c[1], c[0] + r, c[1]],
                        [c[0], c[1] - r, c[0], c[1] + r],
                    ],
                    entities: vec![fid],
                }
            } else {
                let p_lo = to2d(cyl.origin + axis * lo);
                let p_hi = to2d(cyl.origin + axis * hi);
                let seg = extend_segment(p_lo, p_hi, 0.10, 1.5);
                let dx = seg[2] - seg[0];
                let dy = seg[3] - seg[1];
                if (dx * dx + dy * dy).sqrt() < 2.0 {
                    continue;
                }
                Centerline {
                    kind: "axis".to_string(),
                    segments: vec![seg],
                    entities: vec![fid],
                }
            };
            cands.push(cl);
        }
    }
    dedup_centerlines(cands)
}

#[cfg(test)]
mod back_of_plane_outline_tests {
    use super::*;
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

    fn box_sid(m: &mut BRepModel) -> SolidId {
        match TopologyBuilder::new(m)
            .create_box_3d(8.0, 8.0, 8.0)
            .expect("box")
        {
            GeometryId::Solid(s) => s,
            o => panic!("{o:?}"),
        }
    }

    /// **The keep-side sign, pinned directly.** The rendered flange fixture
    /// cannot discriminate this: its bisected features are mirror-symmetric
    /// about the cutting plane (a bore/hole rim clipped from EITHER side
    /// projects to the same span), so flipping `depth <= tol` to
    /// `depth >= tol` in `back_of_plane_outline` would render pixel-
    /// identical output on that fixture. This test uses an asymmetric setup
    /// instead: an 8×8×8 box centred at the world origin (x ∈ [−4, 4]) cut
    /// by a plane at x = 6, normal +X. Every point of the box has
    /// depth = x − 6 ∈ [−10, −2] — strictly negative, i.e. the WHOLE box is
    /// behind the plane (the material a section keeps) — so the function
    /// must return real edges, not silently drop everything.
    #[test]
    fn box_entirely_behind_the_plane_is_kept() {
        let mut m = BRepModel::new();
        let sid = box_sid(&mut m);
        let n = Vector3::new(1.0, 0.0, 0.0);
        let v = Vector3::new(0.0, 0.0, 1.0);
        let u = v.cross(&n);
        let out = back_of_plane_outline(&m, sid, Point3::new(6.0, 0.0, 0.0), n, u, v);
        assert!(
            !out.is_empty(),
            "a box entirely BEHIND the cutting plane must produce outline edges"
        );
    }

    /// The mirror of the test above: the SAME box, cut by a plane at
    /// x = −6. Every point now has depth = x − (−6) ∈ [2, 10] — strictly
    /// positive, i.e. the whole box is IN FRONT of the plane, the material
    /// a real section cuts away. `back_of_plane_outline` must return
    /// nothing; a flipped keep-side sign would make this test (and only
    /// this one, among everything else in the crate) go red.
    #[test]
    fn box_entirely_in_front_of_the_plane_is_removed() {
        let mut m = BRepModel::new();
        let sid = box_sid(&mut m);
        let n = Vector3::new(1.0, 0.0, 0.0);
        let v = Vector3::new(0.0, 0.0, 1.0);
        let u = v.cross(&n);
        let out = back_of_plane_outline(&m, sid, Point3::new(-6.0, 0.0, 0.0), n, u, v);
        assert!(
            out.is_empty(),
            "a box entirely IN FRONT of the cutting plane must produce no \
             outline edges (that material is what a section cuts away); got {out:?}"
        );
    }
}
