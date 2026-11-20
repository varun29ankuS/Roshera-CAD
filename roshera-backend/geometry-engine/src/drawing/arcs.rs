//! A cylinder that does not go all the way round is an ARC, and a drawing
//! states an arc by its radius.
//!
//! `readable::extract_dimensions` gives every cylindrical face a diameter, a
//! length and two corner offsets, and `readable::bore_face_ids` calls every
//! concave one a bore. Both are right for a drilled hole and a turned boss.
//! For a corner round, a pocket corner or a half-round end they put a hole of
//! twice the radius in the hole table ("Ø30.00 ↧ 12.00" for an R15 inside
//! corner), and they owe the sheet four numbers where a drafter writes one:
//! `R15`.
//!
//! This module reads how far round each cylinder of the solid goes and
//! rewrites the dimension table the way a drawing states it:
//!
//! * a cylinder that **closes** (alone, or with the faces a seam or a boolean
//!   split it into) is untouched: a bore or a boss, as before;
//! * an arc of **more than half a turn** keeps its diameter, length and
//!   offsets (ISO 129-1: a diameter where the arc exceeds 180 degrees), but
//!   is not a drilled hole and gets no hole-table row;
//! * an arc of **half a turn or less** is stated by one `"radius"` record
//!   (`R15`), anchored on the arc. Its centre offsets and its length are kept
//!   only when the arc is NOT tangent to its neighbours at both ends: a
//!   tangent round is located by the faces it blends and is as long as they
//!   are; a notch cut into an edge is located by its centre.
//!
//! The same table feeds the sheet ([`super::dimensioning::convention_records`])
//! and the certificate's re-measurement
//! ([`super::sheet_certificate::certify_drawing`]), so a radius on the sheet
//! is checked against a radius read off the live model, never against a
//! diameter under another name.

use std::collections::HashSet;

use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::adjacency::{
    attributed_adjacency_graph, rim_circle, same_surface, AttributedAdjacencyGraph, EdgeConvexity,
    FaceGeometry,
};
use crate::readable::dimensions::feature_dim_pid;
use crate::readable::{bore_face_ids, extract_dimensions, DimensionRecord};

/// Samples per boundary edge when reading how far round a cylinder goes.
const EDGE_SAMPLES: usize = 64;

/// A gap in azimuth wider than this is azimuth the cylinder does not reach
/// (one and a half sampling steps of a full-turn edge).
const OPEN_GAP: f64 = 1.5 * std::f64::consts::TAU / EDGE_SAMPLES as f64;

/// Half a turn, with the slack a sampled coverage needs.
const HALF_TURN: f64 = std::f64::consts::PI + 1e-6;

/// Every face of the solid on one cylinder, on one side of the material.
#[derive(Debug, Clone)]
pub(crate) struct CylinderWall {
    /// The faces, ascending.
    pub faces: Vec<u32>,
    pub origin: [f64; 3],
    /// Unit axis.
    pub axis: [f64; 3],
    pub radius: f64,
    /// Angle (radians) the wall covers about its axis: `TAU` when it closes.
    pub coverage: f64,
    /// Unit radial direction at the middle of the covered arc (any radial
    /// direction for a closed wall).
    pub mid_radial: [f64; 3],
    /// `[min, max]` station of the wall along its axis from `origin`.
    pub stations: [f64; 2],
    /// Every straight side edge of the arc meets its neighbour tangentially
    /// (and there are at least two): a corner round, not a notch.
    pub tangent_ends: bool,
}

impl CylinderWall {
    /// The wall goes all the way round.
    pub fn closes(&self) -> bool {
        self.coverage >= std::f64::consts::TAU - 1e-9
    }

    /// An arc of half a turn or less: stated by its radius.
    pub fn is_radius(&self) -> bool {
        self.coverage <= HALF_TURN
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let n = dot(a, a).sqrt();
    (n > 1e-12).then(|| scale(a, 1.0 / n))
}

/// A frame `(x, y)` square to the unit axis `k`: `x` is the world axis most
/// square to `k` (X, then Y, then Z on ties), projected off it.
fn radial_frame(k: [f64; 3]) -> Option<([f64; 3], [f64; 3])> {
    let mut best: Option<(f64, [f64; 3])> = None;
    for e in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
        let perp = sub(e, scale(k, dot(e, k)));
        let n = dot(perp, perp).sqrt();
        if best.map_or(true, |(bn, _)| n > bn + 1e-9) {
            best = Some((n, perp));
        }
    }
    let x = unit(best?.1)?;
    let y = unit(cross(k, x))?;
    Some((x, y))
}

/// Every cylinder wall of `solid_id`, faces on one surface and one material
/// side merged, with how far round each goes.
pub(crate) fn cylinder_walls(model: &BRepModel, solid_id: SolidId) -> Vec<CylinderWall> {
    let g = attributed_adjacency_graph(model, solid_id);
    walls_of(model, &g)
}

fn walls_of(model: &BRepModel, g: &AttributedAdjacencyGraph) -> Vec<CylinderWall> {
    // Group the cylinder faces: one surface, one material side (a cavity
    // wall -- material round the axis -- never merges with a boss on the
    // same surface).
    let mut groups: Vec<(FaceGeometry, bool, Vec<u32>)> = Vec::new();
    for n in &g.nodes {
        let FaceGeometry::Cylinder { .. } = &n.geometry else {
            continue;
        };
        let cavity = n.cavity == Some(true);
        match groups
            .iter_mut()
            .find(|(geom, c, _)| *c == cavity && same_surface(geom, &n.geometry))
        {
            Some((_, _, faces)) => faces.push(n.face),
            None => groups.push((n.geometry.clone(), cavity, vec![n.face])),
        }
    }

    let mut walls = Vec::new();
    for (geom, cavity, mut faces) in groups {
        faces.sort_unstable();
        let FaceGeometry::Cylinder {
            origin,
            axis,
            radius,
        } = geom
        else {
            continue;
        };
        let Some((x, y)) = radial_frame(axis) else {
            continue;
        };

        // Azimuths and stations of every boundary sample; the side edges'
        // convexities.
        let mut azimuths: Vec<f64> = Vec::new();
        let mut stations = [f64::INFINITY, f64::NEG_INFINITY];
        let mut seen_edges: HashSet<u32> = HashSet::new();
        let mut side_edges = 0usize;
        let mut all_tangent = true;
        for &f in &faces {
            for (arc, other) in g.neighbours(f) {
                if !seen_edges.insert(arc.edge) {
                    continue;
                }
                let Some(edge) = model.edges.get(arc.edge) else {
                    continue;
                };
                for i in 0..=EDGE_SAMPLES {
                    let Ok(p) = edge.evaluate(i as f64 / EDGE_SAMPLES as f64, &model.curves) else {
                        continue;
                    };
                    let d = sub([p.x, p.y, p.z], origin);
                    let s = dot(d, axis);
                    stations = [stations[0].min(s), stations[1].max(s)];
                    azimuths.push(dot(d, y).atan2(dot(d, x)));
                }
                // A side edge leaves the wall for another surface and is not
                // one of its end rims.
                let leaves = !faces.contains(&other) && !arc.same_surface;
                if leaves && rim_circle(model, arc.edge, origin, axis).is_none() {
                    side_edges += 1;
                    all_tangent &= arc.convexity == EdgeConvexity::Tangent;
                }
            }
        }
        if azimuths.is_empty() || !(stations[1] > stations[0]) {
            continue;
        }
        azimuths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let tau = std::f64::consts::TAU;
        // Widest gap between neighbouring samples, wrapping round.
        let (first, last) = (azimuths[0], azimuths[azimuths.len() - 1]);
        let (mut gap, mut gap_end) = (first + tau - last, first);
        for pair in azimuths.windows(2) {
            if pair[1] - pair[0] > gap {
                gap = pair[1] - pair[0];
                gap_end = pair[1];
            }
        }
        let (coverage, mid) = if gap > OPEN_GAP {
            (tau - gap, gap_end + 0.5 * (tau - gap))
        } else {
            (tau, 0.0)
        };
        let mid_radial = add(scale(x, mid.cos()), scale(y, mid.sin()));
        walls.push(CylinderWall {
            faces,
            origin,
            axis,
            radius,
            coverage,
            mid_radial,
            stations,
            tangent_ends: coverage < tau && side_edges >= 2 && all_tangent,
        });
    }
    walls
}

/// The faces of the solid's DRILLED bores: concave cylinders that close.
/// [`crate::readable::bore_face_ids`] without the pocket corners, inside
/// rounds and notches, which are arcs and never get a hole-table row.
pub(crate) fn closed_bore_faces(model: &BRepModel, solid_id: SolidId) -> HashSet<u32> {
    let open: HashSet<u32> = cylinder_walls(model, solid_id)
        .into_iter()
        .filter(|w| !w.closes())
        .flat_map(|w| w.faces)
        .collect();
    bore_face_ids(model, solid_id)
        .into_iter()
        .filter(|f| !open.contains(f))
        .collect()
}

/// The solid's dimension table as a drawing owes it: `extract_dimensions`
/// with every arc stated the way this module's docs describe and every cone
/// angle by its drawing convention ([`super::cone_angles`]). The sheet is
/// built from it and the certificate re-measures against it.
pub(crate) fn drafting_dimensions(model: &BRepModel, solid_id: SolidId) -> Vec<DimensionRecord> {
    let records = extract_dimensions(model, solid_id);
    let walls = cylinder_walls(model, solid_id);
    let records = apply_arc_conventions(model, records, &walls);
    super::cone_angles::apply_cone_angle_conventions(model, solid_id, records)
}

/// Rewrite the records of every open cylinder wall (see the module docs).
fn apply_arc_conventions(
    model: &BRepModel,
    records: Vec<DimensionRecord>,
    walls: &[CylinderWall],
) -> Vec<DimensionRecord> {
    let unit = model.document_unit();
    let open: Vec<&CylinderWall> = walls.iter().filter(|w| w.is_radius()).collect();
    if open.is_empty() {
        return records;
    }
    let wall_of = |entities: &[u32]| -> Option<usize> {
        if entities.is_empty() {
            return None;
        }
        open.iter()
            .position(|w| entities.iter().all(|e| w.faces.contains(e)))
    };
    let mut stated: HashSet<usize> = HashSet::new();
    let mut out = Vec::with_capacity(records.len());
    for mut d in records {
        let Some(wi) = wall_of(&d.entities) else {
            out.push(d);
            continue;
        };
        let w = open[wi];
        match d.kind.as_str() {
            "diameter" => {
                // One radius per arc, whichever of its faces came first.
                if !stated.insert(wi) {
                    continue;
                }
                let mid_station = 0.5 * (w.stations[0] + w.stations[1]);
                d.kind = "radius".to_string();
                d.value = w.radius;
                d.label = format!("R{}", unit.format_len_bare(w.radius));
                d.entities = w.faces.clone();
                // On the arc, at the middle of what it covers; `direction`
                // points from the axis out to it, so the centre is one
                // radius back along it.
                d.anchor = add(
                    add(w.origin, scale(w.axis, mid_station)),
                    scale(w.mid_radial, w.radius),
                );
                d.direction = w.mid_radial;
                d.axis = Some(w.axis);
                d.pid = feature_dim_pid(model, &w.faces, "radius");
                d.datum = None;
                out.push(d);
            }
            "length" | "position" if w.tangent_ends => {}
            _ => out.push(d),
        }
    }
    out
}
