//! Section comprehension (campaign #55 Slice 3) — "what does SECTION A-A cut
//! through?", answered against the LIVE model.
//!
//! Slice 1 stored the world cutting plane on the [`Drawing`](super::types::Drawing)
//! (`SectionSemantics { origin, normal, .. }`); Slice 2's certificate re-cuts it
//! to detect a plane that no longer intersects material. This slice derives the
//! ORDERED cut-through list the plane produces: for every face the plane passes
//! through, WHAT it is (a bore — with its hole-table tag, an outer wall, or an
//! interior web) and WHERE it lands along the section's reading axis, in order.
//!
//! ## Doctrine (spec §3.6): analytic plane∩face classification, never the cap
//!
//! The kernel's [`section_solid_by_plane`](crate::operations::section) produces
//! the section CAP meshes but carries NO per-face provenance (a `SectionCap` is a
//! triangulated polygon + its source solid + the plane — Slice 1-2 finding). So
//! the cut-through list is derived by classifying each B-Rep face directly:
//! a face is CUT when the plane's signed distance changes sign across the face's
//! edge-curve samples (analytic curve geometry, never the tessellation). The
//! per-face crossing points are projected into the section's in-plane frame — the
//! SAME `u = v × n`, world-up-preserving frame [`section_view`](super::section_view)
//! draws in — so `span` and ordering read consistently with the drawn section.
//!
//! ## Honesty
//!
//! The list is re-derived live on every call, so a re-drilled or moved bore
//! changes the answer by construction — it is never a memorised cut list. A face
//! that merely TOUCHES the plane at a single vertex (no genuine sign change) is
//! not reported. A plane that no longer cuts the solid yields an empty list.

use serde::{Deserialize, Serialize};

use crate::math::{Point3, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cylinder, Plane};
use crate::primitives::topology_builder::BRepModel;
use crate::readable::bore_face_ids;

use super::hole_table::HoleSite;
use super::types::SectionSemantics;

/// Distance tolerance (kernel mm) for the "does the plane cross this face?"
/// sign-change test and the on-outer-boundary classification. Comfortably above
/// edge-sample round-off, well below any feature a section would distinguish.
const CUT_EPS: f64 = 1e-4;

/// Edge samples per face edge when locating plane crossings. Dense enough that a
/// bore rim circle's two generator crossings land within a few µm of the true
/// generator (the chord-interp error at the crossing is O(r·δ²), δ = 2π/SAMPLES).
const EDGE_SAMPLES: usize = 128;

/// What a single face the section plane passes through IS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionCutKind {
    /// A bore wall: a concave (material-enclosing) cylindrical face — the cut
    /// reveals the hole. Carries the hole-table tag when one matches.
    Bore,
    /// An outer wall: a face on the part's outer skin (a planar face coincident
    /// with the part's bounding extent on its normal axis, or a convex/other
    /// face). The cut's solid boundary.
    Wall,
    /// An interior web: a planar face NOT on the part's outer boundary — a
    /// counterbore floor, blind-bore bottom, rib, or step inside the material.
    Web,
}

impl SectionCutKind {
    /// Lower-case name for compact readback lines.
    pub fn label(self) -> &'static str {
        match self {
            SectionCutKind::Bore => "bore",
            SectionCutKind::Wall => "wall",
            SectionCutKind::Web => "web",
        }
    }
}

/// One face (or one merged bore) the section plane passes through.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionCut {
    /// What this cut is.
    pub kind: SectionCutKind,
    /// The B-Rep face id(s) this cut spans. Usually one; a bore whose lateral
    /// wall is seam-split into several faces merges into one cut naming all of
    /// them.
    pub face_ids: Vec<u32>,
    /// Hole-table tag (`"A1"`, `"B2"`, …) when this cut is a bore matched to a
    /// hole site; `None` for walls, webs, and un-tabled bores.
    pub hole_tag: Option<String>,
    /// `[min, max]` position of the cut along the section's in-plane reading
    /// axis (`u`, kernel mm). A wall seen edge-on has a degenerate span (min ==
    /// max); a bore spans its diameter footprint.
    pub span: [f64; 2],
}

impl SectionCut {
    /// Centre of the cut along the reading axis — the ordering key.
    fn center(&self) -> f64 {
        0.5 * (self.span[0] + self.span[1])
    }
}

/// The ordered answer to "what does SECTION A-A cut through?".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionCutThrough {
    /// The faces the plane passes through, ordered along the section's reading
    /// axis (left to right in the drawn section).
    pub cuts: Vec<SectionCut>,
    /// Direction of sight (unit world vector): SECTION A-A is what you see
    /// looking along this direction. The stored plane normal points OUT toward
    /// the viewer, so this is `−normal` — consistent with `section_view`'s
    /// `u × v = n` orientation invariant.
    pub view_dir: [f64; 3],
}

impl SectionCutThrough {
    /// True when the plane no longer passes through any material.
    pub fn is_empty(&self) -> bool {
        self.cuts.is_empty()
    }

    /// The bore cut (if any) whose hole tag matches `tag`.
    pub fn bore_with_tag(&self, tag: &str) -> Option<&SectionCut> {
        self.cuts
            .iter()
            .find(|c| c.kind == SectionCutKind::Bore && c.hole_tag.as_deref() == Some(tag))
    }
}

/// Signed distance of `p` from the plane `(origin, normal)` (normal assumed
/// unit); positive on the `+normal` side.
#[inline]
fn signed_dist(p: Point3, origin: Point3, normal: Vector3) -> f64 {
    (p - origin).dot(&normal)
}

/// The section's in-plane frame `(u, v)`, matching [`section_view`]: `v` is
/// world-Z projected into the plane (world-Y when the plane is horizontal), and
/// `u = v × n`. `u` is the reading (left→right) axis of the drawn section.
fn in_plane_u(normal: Vector3) -> Option<Vector3> {
    let n = normal.normalize().ok()?;
    let world_up = Vector3::new(0.0, 0.0, 1.0);
    let proj = world_up - n * n.dot(&world_up);
    let v = match proj.normalize() {
        Ok(p) => p,
        Err(_) => {
            let alt = Vector3::new(0.0, 1.0, 0.0);
            (alt - n * n.dot(&alt)).normalize().ok()?
        }
    };
    Some(v.cross(&n))
}

/// Per-axis AABB of the solid from exact edge-curve samples, as `(min, max)`.
/// Used only to classify a planar face as outer-boundary (wall) vs interior
/// (web); `None` when the solid has no sampleable edges.
fn part_bounds(model: &BRepModel, solid_id: SolidId) -> Option<([f64; 3], [f64; 3])> {
    let solid = model.solids.get(solid_id)?;
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut any = false;
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                let Some(lp) = model.loops.get(lid) else {
                    continue;
                };
                for &eid in &lp.edges {
                    let Some(edge) = model.edges.get(eid) else {
                        continue;
                    };
                    for k in 0..=12 {
                        let s = k as f64 / 12.0;
                        if let Ok(p) = edge.evaluate(s, &model.curves) {
                            for i in 0..3 {
                                let c = [p.x, p.y, p.z][i];
                                if c < min[i] {
                                    min[i] = c;
                                }
                                if c > max[i] {
                                    max[i] = c;
                                }
                            }
                            any = true;
                        }
                    }
                }
            }
        }
    }
    if any {
        Some((min, max))
    } else {
        None
    }
}

/// Derive the ordered cut-through list for the stored section plane against the
/// LIVE model. `hole_sites` are the drawing's tabulated bores, used to join a
/// bore cut to its tag (by face-id intersection — no coordinate heuristics).
///
/// Cost is bounded: one edge-sample pass per face, analytic surface downcasts,
/// O(faces·edges·SAMPLES) with no tessellation.
pub fn section_cut_through(
    model: &BRepModel,
    solid_id: SolidId,
    sec: &SectionSemantics,
    hole_sites: &[HoleSite],
) -> SectionCutThrough {
    let origin = Point3::new(sec.origin[0], sec.origin[1], sec.origin[2]);
    let normal = Vector3::new(sec.normal[0], sec.normal[1], sec.normal[2]);
    let view_dir = normal
        .normalize()
        .map(|n| [-n.x, -n.y, -n.z])
        .unwrap_or([0.0, 0.0, -1.0]);

    let Some(u_axis) = in_plane_u(normal) else {
        return SectionCutThrough {
            cuts: Vec::new(),
            view_dir,
        };
    };
    let n = match normal.normalize() {
        Ok(n) => n,
        Err(_) => {
            return SectionCutThrough {
                cuts: Vec::new(),
                view_dir,
            }
        }
    };

    let bores = bore_face_ids(model, solid_id);
    let bounds = part_bounds(model, solid_id);

    let Some(solid) = model.solids.get(solid_id) else {
        return SectionCutThrough {
            cuts: Vec::new(),
            view_dir,
        };
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);

    let mut cuts: Vec<SectionCut> = Vec::new();

    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };

            // Collect edge-crossing points + track the face's signed-distance
            // extent (both-sides test).
            let mut min_d = f64::INFINITY;
            let mut max_d = f64::NEG_INFINITY;
            let mut crossings_u: Vec<f64> = Vec::new();
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                let Some(lp) = model.loops.get(lid) else {
                    continue;
                };
                for &eid in &lp.edges {
                    let Some(edge) = model.edges.get(eid) else {
                        continue;
                    };
                    let mut prev: Option<(f64, Point3)> = None;
                    for k in 0..=EDGE_SAMPLES {
                        let s = k as f64 / EDGE_SAMPLES as f64;
                        let Ok(p) = edge.evaluate(s, &model.curves) else {
                            prev = None;
                            continue;
                        };
                        let d = signed_dist(p, origin, n);
                        if d < min_d {
                            min_d = d;
                        }
                        if d > max_d {
                            max_d = d;
                        }
                        if let Some((pd, pp)) = prev {
                            // Straddles the plane (strict opposite signs).
                            if (pd < 0.0) != (d < 0.0) && (pd - d).abs() > 1e-12 {
                                let t = pd / (pd - d);
                                let cx = pp + (p - pp) * t;
                                crossings_u.push((cx - origin).dot(&u_axis));
                            }
                        }
                        prev = Some((d, p));
                    }
                }
            }

            // The plane must genuinely pass THROUGH the face interior: material
            // on both sides AND at least one located crossing. A face grazing
            // the plane at one vertex (or coincident with it) is not "cut".
            if !(min_d < -CUT_EPS && max_d > CUT_EPS) || crossings_u.is_empty() {
                continue;
            }

            let lo = crossings_u.iter().cloned().fold(f64::INFINITY, f64::min);
            let hi = crossings_u
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max);

            let kind = classify_face(model, face, fid, &bores, bounds);
            let hole_tag = if kind == SectionCutKind::Bore {
                hole_sites
                    .iter()
                    .find(|h| h.face_entities.contains(&fid))
                    .map(|h| h.tag.clone())
            } else {
                None
            };

            cuts.push(SectionCut {
                kind,
                face_ids: vec![fid],
                hole_tag,
                span: [lo, hi],
            });
        }
    }

    // Merge bore faces that share a hole tag (a seam-split bore wall) into one
    // cut naming every face, with the union span.
    merge_tagged_bores(&mut cuts);

    // Order along the reading axis (left → right in the drawn section).
    cuts.sort_by(|a, b| {
        a.center()
            .partial_cmp(&b.center())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    SectionCutThrough { cuts, view_dir }
}

/// Classify a cut face as bore / wall / web.
fn classify_face(
    model: &BRepModel,
    face: &crate::primitives::face::Face,
    fid: u32,
    bores: &std::collections::HashSet<u32>,
    bounds: Option<([f64; 3], [f64; 3])>,
) -> SectionCutKind {
    if bores.contains(&fid) {
        return SectionCutKind::Bore;
    }
    let Some(surface) = model.surfaces.get(face.surface_id) else {
        return SectionCutKind::Wall;
    };
    // Only planar faces can be an interior web; everything else (convex
    // cylinders / bosses, spheres, cones, NURBS) is outer wall material.
    let Some(plane) = surface.as_any().downcast_ref::<Plane>() else {
        return SectionCutKind::Wall;
    };
    let Some((min, max)) = bounds else {
        return SectionCutKind::Wall;
    };
    // Outward normal (apply the face orientation sign, as the bore rule does).
    let nrm = plane.normal * face.orientation.sign();
    // Dominant world axis of the face normal.
    let a = [nrm.x.abs(), nrm.y.abs(), nrm.z.abs()];
    let k = if a[0] >= a[1] && a[0] >= a[2] {
        0
    } else if a[1] >= a[2] {
        1
    } else {
        2
    };
    // Not clearly axis-aligned → treat as outer wall (honest default).
    if a[k] < 0.9 {
        return SectionCutKind::Wall;
    }
    let coord = [plane.origin.x, plane.origin.y, plane.origin.z][k];
    let on_boundary = (coord - min[k]).abs() <= 1e-3 || (coord - max[k]).abs() <= 1e-3;
    if on_boundary {
        SectionCutKind::Wall
    } else {
        SectionCutKind::Web
    }
}

/// Merge bore cuts that share the same hole tag into one cut (union of face ids
/// and span). Untagged cuts and non-bores pass through unchanged.
#[allow(clippy::ptr_arg)]
fn merge_tagged_bores(cuts: &mut Vec<SectionCut>) {
    use std::collections::HashMap;
    let mut tag_index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<SectionCut> = Vec::with_capacity(cuts.len());
    for cut in cuts.drain(..) {
        match (cut.kind, cut.hole_tag.clone()) {
            (SectionCutKind::Bore, Some(tag)) => {
                if let Some(&idx) = tag_index.get(&tag) {
                    let existing: &mut SectionCut = &mut out[idx];
                    existing.face_ids.extend(cut.face_ids);
                    existing.face_ids.sort_unstable();
                    existing.face_ids.dedup();
                    existing.span[0] = existing.span[0].min(cut.span[0]);
                    existing.span[1] = existing.span[1].max(cut.span[1]);
                } else {
                    tag_index.insert(tag, out.len());
                    out.push(cut);
                }
            }
            _ => out.push(cut),
        }
    }
    *cuts = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vector3;
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    /// Build a 40×40×20 plate with a BLIND bore (Ø10, depth 12) drilled down the
    /// Z axis from the top face. Returns the model, the part solid id, and the
    /// bore centre `(cx, cy)` + z bounds.
    fn blind_bored_plate() -> (BRepModel, SolidId, [f64; 2], [f64; 2]) {
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 40.0, 20.0)
            .expect("plate"));
        let (min, max) = part_bounds(&m, plate).expect("plate bounds");
        let cx = 0.5 * (min[0] + max[0]);
        let cy = 0.5 * (min[1] + max[1]);
        let depth = 12.0;
        // Cutter starts at the blind-bottom station and pokes above the top.
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(
                Point3::new(cx, cy, max[2] - depth),
                Vector3::Z,
                5.0,
                depth + 5.0,
            )
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("blind bore");
        (m, part, [cx, cy], [min[2], max[2]])
    }

    fn plane_through(cx: f64, cy: f64, zmid: f64) -> SectionSemantics {
        SectionSemantics {
            origin: [cx, cy, zmid],
            normal: [1.0, 0.0, 0.0],
            section_view_idx: 0,
        }
    }

    /// The cross-section of a blind-bored plate reports the bore, an interior
    /// WEB (the blind-bore floor), and outer WALLS — analytically, from the LIVE
    /// model, never the cap mesh.
    #[test]
    fn blind_bore_section_reports_bore_web_and_walls() {
        let (m, part, [cx, cy], [zmin, zmax]) = blind_bored_plate();
        let sec = plane_through(cx, cy, 0.5 * (zmin + zmax));
        let ct = section_cut_through(&m, part, &sec, &[]);

        assert!(!ct.is_empty(), "the plane must cut material: {ct:?}");
        let bore = ct
            .cuts
            .iter()
            .find(|c| c.kind == SectionCutKind::Bore)
            .expect("a bore cut must be reported");
        // Span ≈ the bore diameter (Ø10) centred on the bore axis.
        let dia = bore.span[1] - bore.span[0];
        assert!(
            (dia - 10.0).abs() < 0.2,
            "bore span must read the Ø10 footprint: {bore:?}"
        );
        assert!(
            ct.cuts.iter().any(|c| c.kind == SectionCutKind::Web),
            "the blind-bore floor is an interior WEB: {ct:?}"
        );
        assert!(
            ct.cuts.iter().any(|c| c.kind == SectionCutKind::Wall),
            "the plate skin faces are WALLS: {ct:?}"
        );
        // Viewing direction is −normal (we look along −X for a +X-normal plane).
        assert!(
            (ct.view_dir[0] + 1.0).abs() < 1e-9,
            "view dir must be −normal: {:?}",
            ct.view_dir
        );
    }

    /// Ordering: along the reading axis the cuts come out sorted, so the near
    /// wall precedes the bore precedes the far wall.
    #[test]
    fn cuts_are_ordered_along_the_reading_axis() {
        let (m, part, [cx, cy], [zmin, zmax]) = blind_bored_plate();
        let sec = plane_through(cx, cy, 0.5 * (zmin + zmax));
        let ct = section_cut_through(&m, part, &sec, &[]);
        let centers: Vec<f64> = ct
            .cuts
            .iter()
            .map(|c| 0.5 * (c.span[0] + c.span[1]))
            .collect();
        for w in centers.windows(2) {
            assert!(
                w[0] <= w[1] + 1e-9,
                "cuts must be ordered by reading-axis centre: {centers:?}"
            );
        }
    }

    /// An OFF-AXIS plane that clears the bore does NOT list it — the answer
    /// tracks the geometry, not a memorised cut list.
    #[test]
    fn off_axis_plane_omits_the_bore() {
        let (m, part, [cx, cy], [zmin, zmax]) = blind_bored_plate();
        // Shift the plane +8 mm in X, well past the Ø10 bore's ±5 mm footprint.
        let sec = SectionSemantics {
            origin: [cx + 8.0, cy, 0.5 * (zmin + zmax)],
            normal: [1.0, 0.0, 0.0],
            section_view_idx: 0,
        };
        let ct = section_cut_through(&m, part, &sec, &[]);
        assert!(
            !ct.cuts.iter().any(|c| c.kind == SectionCutKind::Bore),
            "an off-axis plane must NOT report the bore: {ct:?}"
        );
    }

    /// Re-drilling the bore LARGER changes the live answer: the bore span grows.
    /// (No memorised diameter — the cut list is re-derived against the model.)
    #[test]
    fn re_drilled_larger_bore_widens_the_span() {
        let (m, part, [cx, cy], [zmin, zmax]) = blind_bored_plate();
        let sec = plane_through(cx, cy, 0.5 * (zmin + zmax));
        let ct0 = section_cut_through(&m, part, &sec, &[]);
        let d0 = ct0
            .cuts
            .iter()
            .find(|c| c.kind == SectionCutKind::Bore)
            .map(|c| c.span[1] - c.span[0])
            .expect("bore present");

        // A wider plate + wider bore (Ø16) → the section reads a wider bore.
        let mut m2 = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m2)
            .create_box_3d(40.0, 40.0, 20.0)
            .expect("plate"));
        let (min, max) = part_bounds(&m2, plate).expect("bounds");
        let bore = sid(TopologyBuilder::new(&mut m2)
            .create_cylinder_3d(Point3::new(cx, cy, max[2] - 12.0), Vector3::Z, 8.0, 17.0)
            .expect("bore"));
        let part2 = boolean_operation(
            &mut m2,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("wide bore");
        let sec2 = plane_through(cx, cy, 0.5 * (min[2] + max[2]));
        let ct1 = section_cut_through(&m2, part2, &sec2, &[]);
        let d1 = ct1
            .cuts
            .iter()
            .find(|c| c.kind == SectionCutKind::Bore)
            .map(|c| c.span[1] - c.span[0])
            .expect("bore present");
        assert!(
            d1 > d0 + 2.0,
            "the Ø16 bore must read wider than the Ø10 bore: {d0} → {d1}"
        );
    }
}
