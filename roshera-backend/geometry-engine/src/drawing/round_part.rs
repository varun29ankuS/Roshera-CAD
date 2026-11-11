//! Drafting conventions for ROUND parts — flanges, discs, hubs, covers.
//!
//! The generic dimensioner works from raw geometry: every cylinder gets a
//! diameter plus X/Y offsets from the bounding-box corner, the part gets X/Y/Z
//! overall sizes, and every bore lands in an X/Y hole table. That is the
//! plate convention, and on a round part it is wrong: a flange's bolt holes
//! are located by a pitch-circle diameter, not by offsets from a corner that
//! does not exist, and its outside is a diameter, not two equal widths.
//!
//! This module recognises a round part FROM GEOMETRY ALONE (a STEP import has
//! no bolt-circle feature to read) and states it the way a drawing does:
//!
//! * the outside as `Ø85` on the face view, not `85.00` twice;
//! * bores on the part axis as diameters, located by the centre lines;
//! * equally spaced holes on a common circle as ONE callout —
//!   `4× Ø11 THRU EQ SP` with `Ø60 PCD` — instead of one table row each.
//!
//! The recognition is deliberately narrow. It fires only when the part's
//! outer boundary IS a cylinder (the part's overall size across the axis
//! equals that cylinder's diameter, centred on it) and that cylinder's axis is
//! a world axis. Plates, brackets and blocks never qualify and keep the hole
//! table. A pattern is only claimed when its spacing is equal: unequal spacing
//! would need angles the callout does not carry, so those holes stay tabled.
//!
//! The callout is a claim, so [`bolt_circle_live_check`] re-measures it against
//! the live model — count, hole size, depth, pitch circle and spacing — the
//! same way the certificate re-measures a hole-table row.

use crate::primitives::solid::SolidId;
use crate::primitives::surface::Cylinder;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::DimensionRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Relative tolerance for "the same" length on a round part (scaled by the
/// part's outer radius, floored at 1 mm so tiny parts are not over-strict).
const REL_TOL: f64 = 1e-6;
/// Angular tolerance for equal spacing, degrees.
const ANGLE_TOL_DEG: f64 = 1e-4;

/// Equally spaced holes on a common circle, stated as one callout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoltCircleCallout {
    /// Number of holes.
    pub count: usize,
    /// Hole diameter, kernel mm.
    pub hole_diameter: f64,
    /// Pitch-circle diameter, kernel mm.
    pub pcd: f64,
    /// Hole depth along the axis, kernel mm; `None` when it was not measured.
    pub depth: Option<f64>,
    /// True when every hole runs the full thickness of the part.
    pub through: bool,
    /// Angle of the first hole from the reference direction, degrees [0, 360).
    pub start_angle_deg: f64,
    /// Face ids of each hole (a seam-split wall carries several).
    pub holes: Vec<Vec<u32>>,
    /// The hole callout as inked: `4× Ø11.00 THRU EQ SP`.
    pub hole_label: String,
    /// The pitch-circle callout as inked: `Ø60.00 PCD`.
    pub pcd_label: String,
}

/// What makes a part round, and the callouts that replace the plate
/// convention on its sheet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundPartCallouts {
    /// The part axis (unit, world-aligned).
    pub axis: [f64; 3],
    /// A point on the part axis.
    pub centre: [f64; 3],
    /// Reference direction in the face plane that angles are measured from.
    pub ref_dir: [f64; 3],
    /// Outside diameter, kernel mm.
    pub outer_diameter: f64,
    /// Faces of bores ON the part axis (located by the centre lines).
    pub coaxial_bore_faces: Vec<u32>,
    /// Faces of every cylinder on the part axis, bore or boss.
    pub coaxial_faces: Vec<u32>,
    /// Bolt circles found on the part.
    pub bolt_circles: Vec<BoltCircleCallout>,
}

impl RoundPartCallouts {
    /// Every face a callout (bolt circle or coaxial bore) states on the sheet.
    pub fn callout_faces(&self) -> HashSet<u32> {
        let mut s: HashSet<u32> = self.coaxial_bore_faces.iter().copied().collect();
        for bc in &self.bolt_circles {
            for h in &bc.holes {
                s.extend(h.iter().copied());
            }
        }
        s
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let n = norm(a);
    (n > 1e-12).then(|| scale(a, 1.0 / n))
}

/// One cylindrical wall of the part (several seam-split faces merged).
#[derive(Debug, Clone)]
struct Wall {
    faces: Vec<u32>,
    origin: [f64; 3],
    axis: [f64; 3],
    radius: f64,
    bore: bool,
}

/// Distance from point `p` to the line through `o` along unit `a`.
fn dist_to_line(p: [f64; 3], o: [f64; 3], a: [f64; 3]) -> f64 {
    let d = sub(p, o);
    norm(sub(d, scale(a, dot(d, a))))
}

/// Every cylindrical wall of the solid, seam-split faces merged.
fn cylinder_walls(model: &BRepModel, solid_id: SolidId) -> Vec<Wall> {
    let bores = crate::readable::bore_face_ids(model, solid_id);
    let Some(solid) = model.solids.get(solid_id) else {
        return Vec::new();
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.extend_from_slice(&solid.peer_shells);
    let mut walls: Vec<Wall> = Vec::new();
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let Some(surface) = model.surfaces.get(face.surface_id) else {
                continue;
            };
            let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() else {
                continue;
            };
            let Some(axis) = unit([cyl.axis.x, cyl.axis.y, cyl.axis.z]) else {
                continue;
            };
            let origin = [cyl.origin.x, cyl.origin.y, cyl.origin.z];
            let tol = REL_TOL * cyl.radius.max(1.0);
            let bore = bores.contains(&fid);
            if let Some(w) = walls.iter_mut().find(|w| {
                (w.radius - cyl.radius).abs() <= tol
                    && dot(w.axis, axis).abs() > 1.0 - 1e-9
                    && dist_to_line(origin, w.origin, w.axis) <= tol
                    && w.bore == bore
            }) {
                w.faces.push(fid);
            } else {
                walls.push(Wall {
                    faces: vec![fid],
                    origin,
                    axis,
                    radius: cyl.radius,
                    bore,
                });
            }
        }
    }
    walls
}

/// Index of the world axis `a` lies along, or `None` when it is oblique.
fn world_axis_index(a: [f64; 3]) -> Option<usize> {
    (0..3).find(|&i| a[i].abs() > 1.0 - 1e-9)
}

/// Recognise a round part and its bolt circles, or `None` when the part is
/// not round (its outer boundary is not a world-axis cylinder). `records` is
/// the solid's analytic dimension table (`extract_dimensions`), read for the
/// overall extents and the per-face lengths.
pub fn detect_round_part(
    model: &BRepModel,
    solid_id: SolidId,
    records: &[DimensionRecord],
) -> Option<RoundPartCallouts> {
    let walls = cylinder_walls(model, solid_id);
    let outer = walls.iter().filter(|w| !w.bore).max_by(|a, b| {
        a.radius
            .partial_cmp(&b.radius)
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    let ax_idx = world_axis_index(outer.axis)?;
    let axis = {
        let mut a = [0.0; 3];
        a[ax_idx] = 1.0;
        a
    };
    let tol = REL_TOL * outer.radius.max(1.0);

    // The outer cylinder must BE the part's boundary across the axis: each
    // perpendicular overall size equals its diameter, centred on its axis.
    let perps: Vec<usize> = (0..3).filter(|&i| i != ax_idx).collect();
    for &p in &perps {
        let ext = records
            .iter()
            .find(|d| d.kind == "extent" && d.entities.is_empty() && d.direction[p].abs() > 0.5)?;
        if (ext.value - 2.0 * outer.radius).abs() > 1e3 * tol {
            return None;
        }
        if (ext.anchor[p] - outer.origin[p]).abs() > 1e3 * tol {
            return None;
        }
    }
    let axial_extent = records
        .iter()
        .find(|d| d.kind == "extent" && d.entities.is_empty() && d.direction[ax_idx].abs() > 0.5)
        .map(|d| d.value);

    let centre = outer.origin;
    let ref_dir = {
        let mut r = [0.0; 3];
        r[perps[0]] = 1.0;
        r
    };
    let side_dir = cross(axis, ref_dir);

    let mut coaxial_faces: Vec<u32> = Vec::new();
    let mut coaxial_bore_faces: Vec<u32> = Vec::new();
    let mut off_axis: Vec<&Wall> = Vec::new();
    for w in &walls {
        if dot(w.axis, axis).abs() < 1.0 - 1e-9 {
            continue;
        }
        if dist_to_line(w.origin, centre, axis) <= 1e3 * tol {
            coaxial_faces.extend(w.faces.iter().copied());
            if w.bore {
                coaxial_bore_faces.extend(w.faces.iter().copied());
            }
        } else if w.bore {
            off_axis.push(w);
        }
    }

    // Group off-axis bores by (hole radius, pitch radius).
    let mut groups: Vec<Vec<&Wall>> = Vec::new();
    for w in off_axis {
        let pitch = dist_to_line(w.origin, centre, axis);
        if let Some(g) = groups.iter_mut().find(|g| {
            let g0 = g[0];
            (g0.radius - w.radius).abs() <= 1e3 * tol
                && (dist_to_line(g0.origin, centre, axis) - pitch).abs() <= 1e3 * tol
        }) {
            g.push(w);
        } else {
            groups.push(vec![w]);
        }
    }

    let unit_fmt = model.document_unit();
    let mut bolt_circles = Vec::new();
    for g in groups {
        let n = g.len();
        if n < 2 {
            continue;
        }
        let mut angles: Vec<(f64, &Wall)> = g
            .iter()
            .map(|w| {
                let d = sub(w.origin, centre);
                let a = dot(d, side_dir).atan2(dot(d, ref_dir)).to_degrees();
                (a.rem_euclid(360.0), *w)
            })
            .collect();
        angles.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let step = 360.0 / n as f64;
        let equal = (0..n).all(|i| {
            let next = if i + 1 < n {
                angles[i + 1].0
            } else {
                angles[0].0 + 360.0
            };
            ((next - angles[i].0) - step).abs() <= ANGLE_TOL_DEG
        });
        if !equal {
            continue; // unequal spacing needs angles a callout does not carry
        }
        let hole_d = 2.0 * g[0].radius;
        let pcd = 2.0 * dist_to_line(g[0].origin, centre, axis);
        let holes: Vec<Vec<u32>> = angles.iter().map(|(_, w)| w.faces.clone()).collect();
        let depth = records
            .iter()
            .find(|d| d.kind == "length" && d.entities.iter().any(|e| holes[0].contains(e)))
            .map(|d| d.value);
        let through = match (depth, axial_extent) {
            (Some(dp), Some(t)) => (dp - t).abs() <= 1e3 * tol,
            _ => false,
        };
        let start = angles[0].0;
        let on_centreline = (start / 90.0 - (start / 90.0).round()).abs() * 90.0 <= ANGLE_TOL_DEG;
        let depth_txt = if through {
            "THRU".to_string()
        } else {
            match depth {
                Some(dp) => format!("\u{21A7}{}", unit_fmt.format_len_bare(dp)),
                None => String::new(),
            }
        };
        let mut hole_label = format!("{n}\u{00D7} \u{00D8}{}", unit_fmt.format_len_bare(hole_d));
        if !depth_txt.is_empty() {
            hole_label.push(' ');
            hole_label.push_str(&depth_txt);
        }
        hole_label.push_str(" EQ SP");
        if !on_centreline {
            hole_label.push_str(&format!(" (1ST AT {start:.1}\u{00B0})"));
        }
        bolt_circles.push(BoltCircleCallout {
            count: n,
            hole_diameter: hole_d,
            pcd,
            depth,
            through,
            start_angle_deg: start,
            holes,
            hole_label,
            pcd_label: format!("\u{00D8}{} PCD", unit_fmt.format_len_bare(pcd)),
        });
    }

    Some(RoundPartCallouts {
        axis,
        centre,
        ref_dir,
        outer_diameter: 2.0 * outer.radius,
        coaxial_bore_faces,
        coaxial_faces,
        bolt_circles,
    })
}

/// Rewrite the analytic dimension table into what a drawing of a round part
/// states: drop the overall sizes across the axis (the outside DIAMETER says
/// it), drop part-corner offsets of anything located by the axis or the
/// pitch circle, and replace each bolt circle's per-hole records with one
/// hole callout and one pitch-circle diameter.
pub fn apply_round_conventions(
    records: Vec<DimensionRecord>,
    rp: &RoundPartCallouts,
) -> Vec<DimensionRecord> {
    let ax_idx = world_axis_index(rp.axis).unwrap_or(2);
    let coaxial: HashSet<u32> = rp.coaxial_faces.iter().copied().collect();
    let mut pattern: HashSet<u32> = HashSet::new();
    for bc in &rp.bolt_circles {
        for h in &bc.holes {
            pattern.extend(h.iter().copied());
        }
    }
    let touches =
        |d: &DimensionRecord, set: &HashSet<u32>| d.entities.iter().any(|e| set.contains(e));
    let template = records.iter().find(|d| d.kind == "diameter").cloned();

    let mut out: Vec<DimensionRecord> = records
        .into_iter()
        .filter(|d| {
            if d.kind == "extent" && d.entities.is_empty() {
                return d.direction[ax_idx].abs() > 0.5; // keep the thickness only
            }
            if d.kind == "position" && (touches(d, &coaxial) || touches(d, &pattern)) {
                return false;
            }
            if (d.kind == "diameter" || d.kind == "length") && touches(d, &pattern) {
                return false;
            }
            true
        })
        .collect();

    let Some(template) = template else {
        return out;
    };
    let mut next = out.len();
    for bc in &rp.bolt_circles {
        let first_angle = bc.start_angle_deg.to_radians();
        let side = cross(rp.axis, rp.ref_dir);
        let radial = add(
            scale(rp.ref_dir, first_angle.cos()),
            scale(side, first_angle.sin()),
        );
        let r_pitch = 0.5 * bc.pcd;
        let hole_centre = add(rp.centre, scale(radial, r_pitch));
        let tangential = cross(rp.axis, radial);
        let all_faces: Vec<u32> = bc.holes.iter().flatten().copied().collect();
        let pcd_faces = all_faces.clone();
        // Hole callout: a diameter across the first hole, TANGENTIALLY, so it
        // does not lie on the diameter line the OD and bore are dimensioned on.
        // Its own kind: a pattern callout is drawn on a LEADER (the label is
        // far wider than one hole), never centred on the hole's span.
        out.push(DimensionRecord {
            id: format!("d{next}"),
            kind: "hole_callout".into(),
            value: bc.hole_diameter,
            unit: template.unit.clone(),
            label: bc.hole_label.clone(),
            entities: all_faces,
            anchor: add(hole_centre, scale(tangential, 0.5 * bc.hole_diameter)),
            direction: tangential,
            axis: Some(rp.axis),
            pid: None,
            datum: None,
        });
        next += 1;
        // Pitch circle: a diameter across the pitch circle along the view's
        // own axis. The sheet inks diameters as horizontal / vertical linear
        // dimensions, so an oblique span would be drawn foreshortened under
        // its full value (the first version laid it at 45° and drew a
        // "Ø60 PCD" 42 mm long). Along a world axis the extension lines land
        // on the circle's extremes and the drawn length is the PCD.
        let pcd_dir = rp.ref_dir;
        out.push(DimensionRecord {
            id: format!("d{next}"),
            // Its own kind: it names the same holes as the hole callout but
            // states their LOCATION, not their size -- not a duplicate.
            kind: "pcd".into(),
            value: bc.pcd,
            unit: template.unit.clone(),
            label: bc.pcd_label.clone(),
            entities: pcd_faces,
            anchor: add(rp.centre, scale(pcd_dir, r_pitch)),
            direction: pcd_dir,
            axis: Some(rp.axis),
            pid: None,
            datum: None,
        });
        next += 1;
    }
    out
}

/// Re-measure a bolt-circle callout against the LIVE model. Returns `None`
/// when every claim holds, else a plain statement of the first that does not:
/// a hole gone, a hole re-drilled, a hole moved off the pitch circle or out of
/// equal spacing, or a depth that changed.
pub fn bolt_circle_live_check(
    model: &BRepModel,
    solid_id: SolidId,
    rp: &RoundPartCallouts,
    bc: &BoltCircleCallout,
    tol_mm: f64,
) -> Option<String> {
    let walls = cylinder_walls(model, solid_id);
    let side = cross(rp.axis, rp.ref_dir);
    let mut angles: Vec<f64> = Vec::new();
    for (i, hole) in bc.holes.iter().enumerate() {
        let Some(w) = walls
            .iter()
            .find(|w| w.faces.iter().any(|f| hole.contains(f)))
        else {
            return Some(format!(
                "hole {} of the bolt circle no longer exists",
                i + 1
            ));
        };
        if dot(w.axis, rp.axis).abs() < 1.0 - 1e-9 {
            return Some(format!(
                "hole {} is no longer parallel to the part axis",
                i + 1
            ));
        }
        let live_d = 2.0 * w.radius;
        if (live_d - bc.hole_diameter).abs() > tol_mm {
            return Some(format!(
                "hole {} measures \u{00D8}{live_d:.3}, the sheet says \u{00D8}{:.3}",
                i + 1,
                bc.hole_diameter
            ));
        }
        let live_pcd = 2.0 * dist_to_line(w.origin, rp.centre, rp.axis);
        if (live_pcd - bc.pcd).abs() > tol_mm {
            return Some(format!(
                "hole {} sits on \u{00D8}{live_pcd:.3}, the sheet says \u{00D8}{:.3} PCD",
                i + 1,
                bc.pcd
            ));
        }
        let d = sub(w.origin, rp.centre);
        angles.push(
            dot(d, side)
                .atan2(dot(d, rp.ref_dir))
                .to_degrees()
                .rem_euclid(360.0),
        );
    }
    angles.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = angles.len();
    let step = 360.0 / n.max(1) as f64;
    // Chord tolerance → angular tolerance on the pitch circle.
    let ang_tol = (tol_mm / (0.5 * bc.pcd).max(1e-9)).to_degrees();
    for i in 0..n {
        let next = if i + 1 < n {
            angles[i + 1]
        } else {
            angles[0] + 360.0
        };
        if ((next - angles[i]) - step).abs() > ang_tol {
            return Some("the holes are no longer equally spaced".to_string());
        }
    }
    if let Some(&first) = angles.first() {
        let drift = (first - bc.start_angle_deg + 180.0).rem_euclid(360.0) - 180.0;
        if drift.abs() > ang_tol {
            return Some(format!(
                "the pattern has rotated: first hole at {first:.2}\u{00B0}, the sheet says {:.2}\u{00B0}",
                bc.start_angle_deg
            ));
        }
    }
    if let Some(sheet_depth) = bc.depth {
        let live = crate::readable::extract_dimensions(model, solid_id)
            .into_iter()
            .find(|d| d.kind == "length" && d.entities.iter().any(|e| bc.holes[0].contains(e)))
            .map(|d| d.value);
        match live {
            Some(l) if (l - sheet_depth).abs() <= tol_mm => {}
            Some(l) => {
                return Some(format!(
                    "hole depth measures {l:.3}, the sheet says {sheet_depth:.3}"
                ))
            }
            None => return Some("hole depth can no longer be measured".to_string()),
        }
    }
    None
}
