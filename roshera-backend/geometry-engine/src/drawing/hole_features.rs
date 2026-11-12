//! Drilled holes as a drafter states them: COAXIAL STACKS and their PATTERNS.
//!
//! The generic dimensioner sees anonymous cylinders, so a counterbored bolt
//! hole reached the sheet as two holes — a Ø6.60 hole `↧ 13.20` deep (the
//! drilled wall below the counterbore floor, so a THROUGH hole was printed as
//! BLIND) and a separate Ø11.00 hole `↧ 6.80` — and a countersink's cone was
//! not a hole at all. A drawing states the hole once, as the tool sequence
//! that makes it (ISO 129-1 / ASME Y14.5 hole callouts):
//!
//! * `Ø6.60 THRU` · `Ø8.00 ↧ 12.00`
//! * `Ø6.60 THRU ⌴ Ø11.00 ↧ 6.80` (counterbore)
//! * `Ø9.00 THRU ⌵ Ø18.00 × 90°` (countersink)
//! * `4× Ø6.60 THRU ⌴ Ø11.00 ↧ 6.80` for a linear or rectangular pattern of
//!   identical stacks, located by its first member and its pitch.
//!
//! # Recognition, from geometry alone
//!
//! Built on the attributed adjacency graph
//! ([`crate::readable::adjacency`]). A stack is read from its ENTRY face — a
//! plane perpendicular to the axis whose rim meets the stack convexly — to its
//! far end, and only these shapes are claimed:
//!
//! | entry                                   | drilled wall | far end                         |
//! |-----------------------------------------|--------------|---------------------------------|
//! | the wall itself                         | cylinder     | exit plane (THRU), flat floor, or a drill-point cone |
//! | counterbore: larger coaxial cylinder + planar annular step (concave outer rim, convex inner rim) | cylinder | exit plane (THRU) |
//! | countersink: coaxial cone opening to the entry plane, meeting the wall convexly | cylinder | exit plane (THRU) |
//!
//! Every rim must be a full circle, every plane perpendicular to the axis,
//! every wall a cavity (material surrounding the axis). Anything else — a
//! counterbore over a blind hole, a counterbore AND a countersink, a hole that
//! breaks out into another feature, a chamfered exit — is not claimed: those
//! bores keep their plain hole-table rows. THRU is decided by TOPOLOGY (the
//! drilled wall's far rim opens onto a plane facing away from the entry), not
//! by comparing the wall's length with the part's extent — the comparison
//! that called the counterbored hole blind.
//!
//! A PATTERN is only claimed when identical stacks sit on a full grid with
//! equal pitch along world axes (so the pitch dimension is drawn at its true
//! length): unequal spacing would need per-hole coordinates a pattern callout
//! does not carry, so those holes stay tabled. A round part's polar pattern is
//! the bolt circle's ([`super::round_part`]), which reads its holes' stacks
//! from here so a counterbored flange gets ONE bolt circle.
//!
//! # Certified
//!
//! Every stack and every pattern on a sheet is a claim, so
//! [`hole_stack_live_check`] and [`hole_pattern_live_check`] re-recognise the
//! LIVE model and re-measure each member — sizes, depths, cone angle, axis,
//! entry position, pitch — turning the sheet stale on any mismatch.

use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::adjacency::{
    attributed_adjacency_graph, rim_circle, same_surface, station_and_radius,
    AttributedAdjacencyGraph, EdgeConvexity, FaceGeometry,
};
use crate::readable::DimensionRecord;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Tolerance for "the same" length when grouping and comparing stacks, kernel
/// mm (scaled up for large features).
const LEN_TOL: f64 = 1e-3;
/// Angular tolerance for "the same" cone angle, degrees.
const ANGLE_TOL_DEG: f64 = 1e-4;
/// Parallel-axes threshold on `|k₁ · k₂|`.
const PARALLEL: f64 = 1.0 - 1e-9;

/// A counterbore: a larger coaxial cylinder at the entry, flat-bottomed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Counterbore {
    /// Counterbore diameter, kernel mm.
    pub diameter: f64,
    /// Counterbore depth from the entry face to its floor, kernel mm.
    pub depth: f64,
}

/// A countersink: a coaxial cone at the entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Countersink {
    /// Diameter where the cone meets the entry face, kernel mm.
    pub diameter: f64,
    /// Included angle of the cone, degrees.
    pub angle_deg: f64,
}

/// One recognised hole: the coaxial stack of faces one drilling sequence makes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HoleStack {
    /// Drilled (smallest full) diameter, kernel mm.
    pub diameter: f64,
    /// True when the drilled wall opens onto an exit face.
    pub through: bool,
    /// Full-diameter depth from the entry face, kernel mm, for a blind hole.
    pub depth: Option<f64>,
    /// Counterbore at the entry, if any.
    pub counterbore: Option<Counterbore>,
    /// Countersink at the entry, if any.
    pub countersink: Option<Countersink>,
    /// Included angle of a blind hole's drill point, degrees (`None` for a
    /// flat floor or a through hole).
    pub drill_point_deg: Option<f64>,
    /// Unit axis, pointing from the entry face INTO the material.
    pub axis: [f64; 3],
    /// Where the axis meets the entry face (world).
    pub entry_point: [f64; 3],
    /// Faces of the drilled wall.
    pub drill_faces: Vec<u32>,
    /// Faces of the counterbore wall and its floor, or of the countersink cone.
    pub entry_faces: Vec<u32>,
    /// Faces closing a blind hole (flat floor or drill-point cone).
    pub floor_faces: Vec<u32>,
    /// The drill part of the callout: `Ø6.60`.
    pub drill_label: String,
    /// The depth part: `THRU` or `↧ 12.00`.
    pub depth_label: String,
    /// The entry part: `⌴ Ø11.00 ↧ 6.80`, `⌵ Ø18.00 × 90°`, or empty.
    pub entry_label: String,
}

impl HoleStack {
    /// The whole callout: `Ø6.60 THRU ⌴ Ø11.00 ↧ 6.80`.
    pub fn label(&self) -> String {
        let mut s = format!("{} {}", self.drill_label, self.depth_label);
        if !self.entry_label.is_empty() {
            s.push(' ');
            s.push_str(&self.entry_label);
        }
        s
    }

    /// The size cell of a hole-table row (the depth has its own column):
    /// `Ø6.60 ⌴ Ø11.00 ↧ 6.80`.
    pub fn table_size_label(&self) -> String {
        if self.entry_label.is_empty() {
            self.drill_label.clone()
        } else {
            format!("{} {}", self.drill_label, self.entry_label)
        }
    }

    /// Every face of the stack.
    pub fn faces(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .drill_faces
            .iter()
            .chain(&self.entry_faces)
            .chain(&self.floor_faces)
            .copied()
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The diameter a reader sees at the entry face (counterbore, countersink
    /// or the drill itself) — where a leader touches the hole.
    pub fn entry_diameter(&self) -> f64 {
        if let Some(c) = &self.counterbore {
            c.diameter
        } else if let Some(c) = &self.countersink {
            c.diameter
        } else {
            self.diameter
        }
    }

    /// First difference between this stack's SPECIFICATION (sizes, depths,
    /// angles, axis direction) and `other`'s, in words; `None` when they are
    /// the same hole. Position is not part of the specification.
    pub fn spec_mismatch(&self, other: &HoleStack, tol: f64) -> Option<String> {
        let len = |what: &str, a: f64, b: f64| -> Option<String> {
            ((a - b).abs() > tol).then(|| format!("{what} measures {b:.3}, the sheet says {a:.3}"))
        };
        if self.through != other.through {
            return Some(format!(
                "the hole is {} on the model, the sheet says {}",
                if other.through { "THRU" } else { "blind" },
                if self.through { "THRU" } else { "blind" }
            ));
        }
        if let Some(m) = len("the drill \u{00D8}", self.diameter, other.diameter) {
            return Some(m);
        }
        match (self.depth, other.depth) {
            (Some(a), Some(b)) => {
                if let Some(m) = len("the hole depth", a, b) {
                    return Some(m);
                }
            }
            (None, None) => {}
            _ => return Some("the hole depth claim no longer applies".to_string()),
        }
        match (&self.counterbore, &other.counterbore) {
            (Some(a), Some(b)) => {
                if let Some(m) = len("the counterbore \u{00D8}", a.diameter, b.diameter) {
                    return Some(m);
                }
                if let Some(m) = len("the counterbore depth", a.depth, b.depth) {
                    return Some(m);
                }
            }
            (None, None) => {}
            (Some(_), None) => return Some("the counterbore is gone".to_string()),
            (None, Some(_)) => return Some("the hole now has a counterbore".to_string()),
        }
        match (&self.countersink, &other.countersink) {
            (Some(a), Some(b)) => {
                if let Some(m) = len("the countersink \u{00D8}", a.diameter, b.diameter) {
                    return Some(m);
                }
                if (a.angle_deg - b.angle_deg).abs() > ANGLE_TOL_DEG.max(tol) {
                    return Some(format!(
                        "the countersink angle measures {:.3}\u{00B0}, the sheet says {:.3}\u{00B0}",
                        b.angle_deg, a.angle_deg
                    ));
                }
            }
            (None, None) => {}
            (Some(_), None) => return Some("the countersink is gone".to_string()),
            (None, Some(_)) => return Some("the hole now has a countersink".to_string()),
        }
        match (self.drill_point_deg, other.drill_point_deg) {
            (Some(a), Some(b)) if (a - b).abs() > ANGLE_TOL_DEG.max(tol) => {
                return Some(format!(
                    "the drill point measures {b:.3}\u{00B0}, the sheet says {a:.3}\u{00B0}"
                ))
            }
            (Some(_), None) | (None, Some(_)) => {
                return Some("the hole's floor changed between flat and pointed".to_string())
            }
            _ => {}
        }
        if dot(self.axis, other.axis) < PARALLEL {
            return Some("the hole axis has turned".to_string());
        }
        None
    }
}

/// A linear or rectangular pattern of identical stacks on equal pitch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HolePattern {
    /// Indices into [`HoleFeatures::stacks`], row-major from the first member:
    /// member `iu + iv·counts[0]` sits at `first + iu·pitch[0]·dirs[0] +
    /// iv·pitch[1]·dirs[1]`.
    pub members: Vec<usize>,
    /// Members along each pattern direction.
    pub counts: [usize; 2],
    /// Pitch along each direction, kernel mm (`0.0` where the count is 1).
    pub pitch: [f64; 2],
    /// The two pattern directions: world axes perpendicular to the hole axis.
    pub dirs: [[f64; 3]; 2],
    /// The pattern callout as inked: `4× Ø6.60 THRU ⌴ Ø11.00 ↧ 6.80`.
    pub label: String,
}

/// Every hole stack on a part and the patterns among them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HoleFeatures {
    /// Every recognised stack.
    pub stacks: Vec<HoleStack>,
    /// Patterns, each stated as one callout.
    pub patterns: Vec<HolePattern>,
}

impl HoleFeatures {
    /// Faces of every stack a PATTERN callout states (size and location) —
    /// these holes are inked by the callout, not by a hole-table row.
    pub fn callout_faces(&self) -> HashSet<u32> {
        let mut s = HashSet::new();
        for p in &self.patterns {
            for &i in &p.members {
                if let Some(st) = self.stacks.get(i) {
                    s.extend(st.faces());
                }
            }
        }
        s
    }

    /// The stack whose drilled wall includes `face`.
    pub fn stack_of_drill_face(&self, face: u32) -> Option<&HoleStack> {
        self.stacks.iter().find(|s| s.drill_faces.contains(&face))
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// Kind of a cavity wall of revolution.
#[derive(Debug, Clone, Copy, PartialEq)]
enum WallKind {
    Cylinder { radius: f64 },
    Cone { half_angle: f64 },
}

/// A cavity wall: every face of the solid on one cylinder / cone surface.
#[derive(Debug, Clone)]
struct Wall {
    faces: Vec<u32>,
    kind: WallKind,
    /// Axis point (cylinder origin / cone apex) and unit axis.
    o: [f64; 3],
    k: [f64; 3],
    /// Union of the faces' station ranges.
    range: [f64; 2],
}

/// What a wall end opens onto.
#[derive(Debug, Clone, PartialEq)]
enum End {
    /// A plane perpendicular to the axis facing away from the wall, met
    /// convexly. `faces` are the plane's faces, `station` its station.
    Opening { faces: Vec<u32>, station: f64 },
    /// A counterbore step: the plane above, whose outer rim meets the larger
    /// coaxial wall `wall` concavely.
    Step {
        faces: Vec<u32>,
        station: f64,
        wall: usize,
    },
    /// A plane disc facing into the wall, met concavely (a flat floor).
    Floor { faces: Vec<u32> },
    /// A coaxial cone wall sharing the rim.
    Cone { wall: usize, convex: bool },
    /// Anything else.
    Other,
}

fn tol_for(size: f64) -> f64 {
    LEN_TOL * size.max(1.0)
}

/// Every cavity wall of revolution in the graph, faces on one surface merged.
fn cavity_walls(g: &AttributedAdjacencyGraph) -> Vec<Wall> {
    let mut walls: Vec<(Wall, FaceGeometry)> = Vec::new();
    for n in &g.nodes {
        if n.cavity != Some(true) {
            continue;
        }
        let Some(range) = n.station_range else {
            continue;
        };
        let (kind, o, k) = match &n.geometry {
            FaceGeometry::Cylinder {
                origin,
                axis,
                radius,
            } => (WallKind::Cylinder { radius: *radius }, *origin, *axis),
            FaceGeometry::Cone {
                apex,
                axis,
                half_angle,
            } => (
                WallKind::Cone {
                    half_angle: *half_angle,
                },
                *apex,
                *axis,
            ),
            _ => continue,
        };
        if let Some((w, _)) = walls
            .iter_mut()
            .find(|(_, geom)| same_surface(geom, &n.geometry))
        {
            w.faces.push(n.face);
            w.range = [w.range[0].min(range[0]), w.range[1].max(range[1])];
        } else {
            walls.push((
                Wall {
                    faces: vec![n.face],
                    kind,
                    o,
                    k,
                    range,
                },
                n.geometry.clone(),
            ));
        }
    }
    walls.into_iter().map(|(w, _)| w).collect()
}

/// Radius of a wall at a station.
fn wall_radius_at(w: &Wall, station: f64) -> f64 {
    match w.kind {
        WallKind::Cylinder { radius } => radius,
        WallKind::Cone { half_angle } => station.abs() * half_angle.tan(),
    }
}

/// The rim arcs of a wall at its `lo` (false) or `hi` (true) end: every arc
/// leaving the wall whose edge is a circle about the wall axis at that
/// station, as `(other face, convexity, rim radius)`, plus the swept angle the
/// rim covers (2π for a full circle).
fn wall_end_rim(
    model: &BRepModel,
    g: &AttributedAdjacencyGraph,
    w: &Wall,
    hi: bool,
) -> (Vec<(u32, EdgeConvexity, f64)>, f64) {
    let station = if hi { w.range[1] } else { w.range[0] };
    let tol = tol_for(wall_radius_at(w, station));
    let mut out = Vec::new();
    let mut seen_edges = BTreeSet::new();
    let mut sweep = 0.0;
    for &f in &w.faces {
        for (arc, other) in g.neighbours(f) {
            if w.faces.contains(&other) || !seen_edges.insert(arc.edge) {
                continue;
            }
            let Some(rim) = rim_circle(model, arc.edge, w.o, w.k) else {
                continue;
            };
            if (rim.station - station).abs() > tol {
                continue;
            }
            sweep += edge_sweep(model, arc.edge, w.o, w.k);
            out.push((other, arc.convexity, rim.radius));
        }
    }
    (out, sweep)
}

/// Angle an edge sweeps about an axis (unwrapped over its samples).
fn edge_sweep(model: &BRepModel, edge_id: u32, o: [f64; 3], k: [f64; 3]) -> f64 {
    let Some(edge) = model.edges.get(edge_id) else {
        return 0.0;
    };
    // A reference frame in the plane perpendicular to k.
    let helper = if k[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let x = {
        let d = sub(helper, scale(k, dot(helper, k)));
        let n = norm(d);
        if n < 1e-12 {
            return 0.0;
        }
        scale(d, 1.0 / n)
    };
    let y = [
        k[1] * x[2] - k[2] * x[1],
        k[2] * x[0] - k[0] * x[2],
        k[0] * x[1] - k[1] * x[0],
    ];
    const N: usize = 32;
    let mut total = 0.0;
    let mut prev: Option<f64> = None;
    for i in 0..=N {
        let Ok(p) = edge.evaluate(i as f64 / N as f64, &model.curves) else {
            return 0.0;
        };
        let d = sub([p.x, p.y, p.z], o);
        let a = dot(d, y).atan2(dot(d, x));
        if let Some(pa) = prev {
            let mut da = a - pa;
            while da > std::f64::consts::PI {
                da -= std::f64::consts::TAU;
            }
            while da < -std::f64::consts::PI {
                da += std::f64::consts::TAU;
            }
            total += da;
        }
        prev = Some(a);
    }
    total.abs()
}

/// The plane face's outward normal, when the face is a plane.
fn plane_normal(g: &AttributedAdjacencyGraph, f: u32) -> Option<([f64; 3], [f64; 3])> {
    match g.node(f).map(|n| &n.geometry) {
        Some(FaceGeometry::Plane { origin, normal }) => Some((*origin, *normal)),
        _ => None,
    }
}

/// Classify the `hi` / `lo` end of wall `wi`.
fn classify_end(
    model: &BRepModel,
    g: &AttributedAdjacencyGraph,
    walls: &[Wall],
    wi: usize,
    hi: bool,
) -> End {
    let w = &walls[wi];
    let station = if hi { w.range[1] } else { w.range[0] };
    let (rim, sweep) = wall_end_rim(model, g, w, hi);
    if rim.is_empty() || (sweep - std::f64::consts::TAU).abs() > 1e-6 {
        return End::Other;
    }
    let out_dir = if hi { w.k } else { scale(w.k, -1.0) };
    let others: BTreeSet<u32> = rim.iter().map(|r| r.0).collect();
    let convexities: BTreeSet<_> = rim.iter().map(|r| r.1 as u8).collect();
    if convexities.len() != 1 {
        return End::Other;
    }
    let convexity = rim[0].1;

    // A coaxial cone wall?
    if let Some(cwi) = walls.iter().position(|x| {
        matches!(x.kind, WallKind::Cone { .. }) && x.faces.iter().any(|f| others.contains(f))
    }) {
        let all_on_cone = others.iter().all(|f| walls[cwi].faces.contains(f));
        let coaxial = dot(walls[cwi].k, w.k).abs() > PARALLEL
            && station_and_radius(walls[cwi].o, w.o, w.k).1 <= tol_for(1.0);
        if all_on_cone && coaxial && cwi != wi {
            return match convexity {
                EdgeConvexity::Convex => End::Cone {
                    wall: cwi,
                    convex: true,
                },
                EdgeConvexity::Concave => End::Cone {
                    wall: cwi,
                    convex: false,
                },
                _ => End::Other,
            };
        }
        return End::Other;
    }

    // Otherwise every neighbour must be ONE plane perpendicular to the axis.
    let mut plane: Option<([f64; 3], [f64; 3])> = None;
    for &f in &others {
        let Some((po, pn)) = plane_normal(g, f) else {
            return End::Other;
        };
        if dot(pn, w.k).abs() < PARALLEL {
            return End::Other;
        }
        if let Some((o0, n0)) = plane {
            if dot(n0, pn) < PARALLEL || dot(sub(po, o0), n0).abs() > tol_for(1.0) {
                return End::Other;
            }
        } else {
            plane = Some((po, pn));
        }
    }
    let Some((po, pn)) = plane else {
        return End::Other;
    };
    let plane_station = dot(sub(po, w.o), w.k);
    if (plane_station - station).abs() > tol_for(wall_radius_at(w, station)) {
        return End::Other;
    }
    let faces: Vec<u32> = others.iter().copied().collect();
    let facing_out = dot(pn, out_dir) > 0.0;
    match (convexity, facing_out) {
        (EdgeConvexity::Convex, true) => {
            // Opening — or a counterbore step, when the same plane's other
            // circular rim meets a LARGER coaxial cavity cylinder concavely.
            let r_here = wall_radius_at(w, station);
            for (lwi, lw) in walls.iter().enumerate() {
                if lwi == wi {
                    continue;
                }
                let WallKind::Cylinder { radius } = lw.kind else {
                    continue;
                };
                if radius <= r_here + tol_for(radius) {
                    continue;
                }
                if dot(lw.k, w.k).abs() < PARALLEL
                    || station_and_radius(lw.o, w.o, w.k).1 > tol_for(radius)
                {
                    continue;
                }
                // The larger wall's end at this plane must be the step's
                // concave outer rim, and the step plane is all it meets there.
                for lhi in [false, true] {
                    let lst = if lhi { lw.range[1] } else { lw.range[0] };
                    let lst_in_w = dot(sub(add(lw.o, scale(lw.k, lst)), w.o), w.k);
                    if (lst_in_w - station).abs() > tol_for(radius) {
                        continue;
                    }
                    let (lrim, lsweep) = wall_end_rim(model, g, lw, lhi);
                    let lothers: BTreeSet<u32> = lrim.iter().map(|r| r.0).collect();
                    if !lrim.is_empty()
                        && (lsweep - std::f64::consts::TAU).abs() <= 1e-6
                        && lrim.iter().all(|r| r.1 == EdgeConvexity::Concave)
                        && lothers.iter().all(|f| faces.contains(f))
                    {
                        return End::Step {
                            faces,
                            station: plane_station,
                            wall: lwi,
                        };
                    }
                }
            }
            End::Opening {
                faces,
                station: plane_station,
            }
        }
        (EdgeConvexity::Concave, false) => {
            // A flat floor: a disc whose every edge is this rim.
            let disc = faces.iter().all(|&f| {
                g.node(f).is_some_and(|n| {
                    n.loop_count == 1
                        && g.neighbours(f)
                            .iter()
                            .all(|(_, o)| w.faces.contains(o) || faces.contains(o))
                })
            });
            if disc {
                End::Floor { faces }
            } else {
                End::Other
            }
        }
        _ => End::Other,
    }
}

/// Station of the far end of a cone wall away from the end that touches
/// `station`, with its rim classification.
fn cone_far_end(
    model: &BRepModel,
    g: &AttributedAdjacencyGraph,
    walls: &[Wall],
    cwi: usize,
    near_station_world: [f64; 3],
) -> (bool, End) {
    let c = &walls[cwi];
    let near = dot(sub(near_station_world, c.o), c.k);
    let near_is_hi = (near - c.range[1]).abs() < (near - c.range[0]).abs();
    let far_hi = !near_is_hi;
    (far_hi, classify_end(model, g, walls, cwi, far_hi))
}

fn fmt_angle(a: f64) -> String {
    if (a - a.round()).abs() < 1e-6 {
        format!("{:.0}\u{00B0}", a.round())
    } else {
        format!("{a:.1}\u{00B0}")
    }
}

/// Recognise every hole stack of `solid_id` (see the module docs for the
/// shapes claimed). Deterministic: ordered by the drilled wall's first face.
pub fn recognize_hole_stacks(model: &BRepModel, solid_id: SolidId) -> Vec<HoleStack> {
    let g = attributed_adjacency_graph(model, solid_id);
    let walls = cavity_walls(&g);
    let unit = model.document_unit();
    let mut stacks = Vec::new();
    let mut used: BTreeSet<usize> = BTreeSet::new();

    for (wi, w) in walls.iter().enumerate() {
        let WallKind::Cylinder { radius } = w.kind else {
            continue;
        };
        if used.contains(&wi) {
            continue;
        }
        let tol = tol_for(radius);
        let lo = classify_end(model, &g, &walls, wi, false);
        let hi = classify_end(model, &g, &walls, wi, true);

        // Read the stack from its entry end. Candidates: (entry end is hi?).
        let mut found: Option<HoleStack> = None;
        for entry_hi in [true, false] {
            let (entry, far) = if entry_hi { (&hi, &lo) } else { (&lo, &hi) };
            // Unit axis into the material from the entry.
            let axis = if entry_hi { scale(w.k, -1.0) } else { w.k };
            let s_entry_wall = if entry_hi { w.range[1] } else { w.range[0] };
            let s_far_wall = if entry_hi { w.range[0] } else { w.range[1] };
            let wall_len = (s_far_wall - s_entry_wall).abs();

            // ── Entry ────────────────────────────────────────────────────────
            let mut entry_faces: Vec<u32> = Vec::new();
            let mut counterbore = None;
            let mut countersink = None;
            let entry_station_w: f64; // station of the entry face on w's axis
            let mut extra_walls: Vec<usize> = Vec::new();
            match entry {
                End::Opening { station, .. } => {
                    entry_station_w = *station;
                }
                End::Step {
                    faces,
                    station,
                    wall,
                } => {
                    let lw = &walls[*wall];
                    let WallKind::Cylinder { radius: lr } = lw.kind else {
                        continue;
                    };
                    // The counterbore wall's far end must open onto the entry.
                    let step_world = add(w.o, scale(w.k, *station));
                    let s_step_lw = dot(sub(step_world, lw.o), lw.k);
                    let step_is_hi =
                        (s_step_lw - lw.range[1]).abs() < (s_step_lw - lw.range[0]).abs();
                    let End::Opening {
                        station: open_lw, ..
                    } = classify_end(model, &g, &walls, *wall, !step_is_hi)
                    else {
                        continue;
                    };
                    let open_world = add(lw.o, scale(lw.k, open_lw));
                    entry_station_w = dot(sub(open_world, w.o), w.k);
                    let cb_depth = (entry_station_w - station).abs();
                    // The counterbore must open AWAY from the drilled wall.
                    if dot(sub(open_world, step_world), axis) > 0.0 {
                        continue;
                    }
                    entry_faces.extend(lw.faces.iter().copied());
                    entry_faces.extend(faces.iter().copied());
                    extra_walls.push(*wall);
                    counterbore = Some(Counterbore {
                        diameter: 2.0 * lr,
                        depth: cb_depth,
                    });
                }
                End::Cone { wall, convex: true } => {
                    let near_world = add(w.o, scale(w.k, s_entry_wall));
                    let (far_hi, far_end) = cone_far_end(model, &g, &walls, *wall, near_world);
                    let End::Opening {
                        station: open_c, ..
                    } = far_end
                    else {
                        continue;
                    };
                    let c = &walls[*wall];
                    let WallKind::Cone { half_angle } = c.kind else {
                        continue;
                    };
                    let open_world = add(c.o, scale(c.k, open_c));
                    // It must widen toward the entry.
                    let r_open = wall_radius_at(c, if far_hi { c.range[1] } else { c.range[0] });
                    if r_open <= radius + tol || dot(sub(open_world, near_world), axis) > 0.0 {
                        continue;
                    }
                    entry_station_w = dot(sub(open_world, w.o), w.k);
                    entry_faces.extend(c.faces.iter().copied());
                    extra_walls.push(*wall);
                    countersink = Some(Countersink {
                        diameter: 2.0 * r_open,
                        angle_deg: 2.0 * half_angle.to_degrees(),
                    });
                }
                _ => continue,
            }

            // ── Far end ──────────────────────────────────────────────────────
            let mut floor_faces = Vec::new();
            let mut drill_point_deg = None;
            let through;
            match far {
                End::Opening { .. } => through = true,
                End::Floor { faces } if counterbore.is_none() && countersink.is_none() => {
                    through = false;
                    floor_faces.extend(faces.iter().copied());
                }
                End::Cone {
                    wall,
                    convex: false,
                } if counterbore.is_none() && countersink.is_none() => {
                    // A drill point: the cone closes to its apex — it has no
                    // other rim, and it narrows away from the entry.
                    let c = &walls[*wall];
                    let WallKind::Cone { half_angle } = c.kind else {
                        continue;
                    };
                    let near_world = add(w.o, scale(w.k, s_far_wall));
                    let near_c = dot(sub(near_world, c.o), c.k);
                    let far_c = if (near_c - c.range[1]).abs() < (near_c - c.range[0]).abs() {
                        c.range[0]
                    } else {
                        c.range[1]
                    };
                    if far_c.abs() > tol || dot(sub(c.o, near_world), axis) < 0.0 {
                        continue;
                    }
                    through = false;
                    floor_faces.extend(c.faces.iter().copied());
                    extra_walls.push(*wall);
                    drill_point_deg = Some(2.0 * half_angle.to_degrees());
                }
                _ => continue,
            }
            // Read a THRU hole from its world-positive end only, so both
            // directions do not yield a stack; a blind or entry-featured hole
            // has exactly one entry.
            if through && counterbore.is_none() && countersink.is_none() {
                let dom = (0..3)
                    .max_by(|&a, &b| {
                        axis[a]
                            .abs()
                            .partial_cmp(&axis[b].abs())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .unwrap_or(2);
                if axis[dom] > 0.0 {
                    continue;
                }
            }
            let depth = (!through).then_some(wall_len + (s_entry_wall - entry_station_w).abs());
            let drill_label = format!("\u{00D8}{}", unit.format_len_bare(2.0 * radius));
            let depth_label = match depth {
                None => "THRU".to_string(),
                Some(d) => format!("\u{21A7} {}", unit.format_len_bare(d)),
            };
            let entry_label = if let Some(c) = &counterbore {
                format!(
                    "\u{2334} \u{00D8}{} \u{21A7} {}",
                    unit.format_len_bare(c.diameter),
                    unit.format_len_bare(c.depth)
                )
            } else if let Some(c) = &countersink {
                format!(
                    "\u{2335} \u{00D8}{} \u{00D7} {}",
                    unit.format_len_bare(c.diameter),
                    fmt_angle(c.angle_deg)
                )
            } else {
                String::new()
            };
            let entry_point = add(w.o, scale(w.k, entry_station_w));
            let mut drill_faces = w.faces.clone();
            drill_faces.sort_unstable();
            entry_faces.sort_unstable();
            entry_faces.dedup();
            floor_faces.sort_unstable();
            floor_faces.dedup();
            used.insert(wi);
            used.extend(extra_walls);
            found = Some(HoleStack {
                diameter: 2.0 * radius,
                through,
                depth,
                counterbore,
                countersink,
                drill_point_deg,
                axis,
                entry_point,
                drill_faces,
                entry_faces,
                floor_faces,
                drill_label,
                depth_label,
                entry_label,
            });
            break;
        }
        if let Some(s) = found {
            stacks.push(s);
        }
    }
    stacks.sort_by_key(|s| s.drill_faces.first().copied().unwrap_or(u32::MAX));
    stacks
}

/// Index of the world axis `a` lies along, or `None` when it is oblique.
fn world_axis_index(a: [f64; 3]) -> Option<usize> {
    (0..3).find(|&i| a[i].abs() > PARALLEL)
}

/// Sorted distinct values (within `tol`).
fn distinct(mut v: Vec<f64>, tol: f64) -> Vec<f64> {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<f64> = Vec::new();
    for x in v {
        if out.last().is_none_or(|l| (x - l).abs() > tol) {
            out.push(x);
        }
    }
    out
}

/// Equal spacing of sorted values; `Some(pitch)` (0 for a single value).
fn equal_pitch(v: &[f64], tol: f64) -> Option<f64> {
    if v.len() < 2 {
        return Some(0.0);
    }
    let p = v[1] - v[0];
    v.windows(2)
        .all(|w| ((w[1] - w[0]) - p).abs() <= tol)
        .then_some(p)
}

/// Patterns among identical stacks (plates only — a round part's polar
/// pattern is the bolt circle's).
fn find_patterns(stacks: &[HoleStack]) -> Vec<HolePattern> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, s) in stacks.iter().enumerate() {
        let tol = tol_for(s.entry_diameter());
        if let Some(g) = groups.iter_mut().find(|g| {
            let r = &stacks[g[0]];
            r.spec_mismatch(s, tol).is_none()
                && dot(r.axis, s.axis) > PARALLEL
                && dot(sub(s.entry_point, r.entry_point), r.axis).abs() <= tol
        }) {
            g.push(i);
        } else {
            groups.push(vec![i]);
        }
    }
    let mut out = Vec::new();
    for g in groups {
        if g.len() < 2 {
            continue;
        }
        let first = &stacks[g[0]];
        let Some(ax) = world_axis_index(first.axis) else {
            continue;
        };
        let perps: Vec<usize> = (0..3).filter(|&i| i != ax).collect();
        let (u, v) = (perps[0], perps[1]);
        let tol = tol_for(first.entry_diameter());
        let us = distinct(g.iter().map(|&i| stacks[i].entry_point[u]).collect(), tol);
        let vs = distinct(g.iter().map(|&i| stacks[i].entry_point[v]).collect(), tol);
        if us.len() * vs.len() != g.len() {
            continue;
        }
        let (Some(pu), Some(pv)) = (equal_pitch(&us, tol), equal_pitch(&vs, tol)) else {
            continue;
        };
        // Every grid cell holds exactly one member.
        let mut members = Vec::with_capacity(g.len());
        let mut ok = true;
        for &cv in &vs {
            for &cu in &us {
                let hit: Vec<usize> = g
                    .iter()
                    .copied()
                    .filter(|&i| {
                        (stacks[i].entry_point[u] - cu).abs() <= tol
                            && (stacks[i].entry_point[v] - cv).abs() <= tol
                    })
                    .collect();
                if hit.len() != 1 {
                    ok = false;
                    break;
                }
                members.push(hit[0]);
            }
            if !ok {
                break;
            }
        }
        if !ok {
            continue;
        }
        let mut du = [0.0; 3];
        du[u] = 1.0;
        let mut dv = [0.0; 3];
        dv[v] = 1.0;
        out.push(HolePattern {
            label: format!("{}\u{00D7} {}", g.len(), first.label()),
            members,
            counts: [us.len(), vs.len()],
            pitch: [pu, pv],
            dirs: [du, dv],
        });
    }
    out
}

/// Recognise the hole stacks of a solid and the patterns among them; `None`
/// when the part has no recognisable hole stack. On a round part
/// (`round_part` is `Some`) no linear/rectangular pattern is claimed — its
/// off-axis holes are located by the bolt circle.
pub fn detect_hole_features(
    model: &BRepModel,
    solid_id: SolidId,
    round_part: Option<&super::round_part::RoundPartCallouts>,
) -> Option<HoleFeatures> {
    let mut stacks = recognize_hole_stacks(model, solid_id);
    if let Some(rp) = round_part {
        // A bore ON the part axis is stated by the round-part convention (its
        // diameter on the face view), not as a drilled hole.
        stacks.retain(|s| {
            !(dot(s.axis, rp.axis).abs() > PARALLEL
                && station_and_radius(s.entry_point, rp.centre, rp.axis).1
                    <= tol_for(s.entry_diameter()))
        });
    }
    if stacks.is_empty() {
        return None;
    }
    let patterns = if round_part.is_some() {
        Vec::new()
    } else {
        find_patterns(&stacks)
    };
    Some(HoleFeatures { stacks, patterns })
}

/// Rewrite the analytic dimension table into what the hole callouts state:
///
/// * a stack's wall lengths, cone angles and cone "diameters" are stated by
///   its callout (the cone record's diameter is read off the uncut surface
///   bound and is not the countersink's), and a counterbore wall is not a
///   separately located hole — those records go;
/// * a PATTERN's members lose their own diameter and corner offsets, and the
///   pattern gains one `hole_callout` (on a leader to the first member), the
///   first member's two corner offsets as `location` dimensions, and one
///   `pitch` dimension per pattern direction (`3× 20.00 (=60.00)`).
pub fn apply_hole_conventions(
    records: Vec<DimensionRecord>,
    hf: &HoleFeatures,
    unit: crate::units::LengthUnit,
) -> Vec<DimensionRecord> {
    let mut stated: HashSet<u32> = HashSet::new(); // lengths/angles stated
    let mut secondary: HashSet<u32> = HashSet::new(); // never a hole of its own
    for s in &hf.stacks {
        stated.extend(s.faces());
        secondary.extend(s.entry_faces.iter().copied());
        secondary.extend(s.floor_faces.iter().copied());
    }
    let patterned = hf.callout_faces();
    let touches =
        |d: &DimensionRecord, set: &HashSet<u32>| d.entities.iter().any(|e| set.contains(e));

    // The first member's corner offsets, before they are dropped.
    let mut locations: Vec<Vec<DimensionRecord>> = Vec::new();
    for p in &hf.patterns {
        let first = p.members.first().and_then(|&i| hf.stacks.get(i));
        let locs = first
            .map(|st| {
                records
                    .iter()
                    .filter(|d| {
                        d.kind == "position"
                            && d.entities.iter().any(|e| st.drill_faces.contains(e))
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        locations.push(locs);
    }
    let template = records
        .iter()
        .find(|d| d.kind == "diameter" || d.kind == "extent")
        .map(|d| d.unit.clone())
        .unwrap_or_else(|| "mm".to_string());

    let mut out: Vec<DimensionRecord> = records
        .into_iter()
        .filter(|d| {
            if matches!(d.kind.as_str(), "length" | "angle") && touches(d, &stated) {
                return false;
            }
            if matches!(d.kind.as_str(), "diameter" | "position") && touches(d, &secondary) {
                return false;
            }
            if matches!(d.kind.as_str(), "diameter" | "position") && touches(d, &patterned) {
                return false;
            }
            true
        })
        .collect();

    let mut next = 0usize;
    let mut id = || {
        let s = format!("hf{next}");
        next += 1;
        s
    };
    for (p, locs) in hf.patterns.iter().zip(locations) {
        let Some(first) = p.members.first().and_then(|&i| hf.stacks.get(i)) else {
            continue;
        };
        let all_faces: Vec<u32> = p
            .members
            .iter()
            .filter_map(|&i| hf.stacks.get(i))
            .flat_map(|s| s.faces())
            .collect();
        // The leader touches the hole where a reader sees it: the entry
        // circle — of the member furthest along +u +v (up and right in plan),
        // nearest where the leader leaves the view, so it crosses as little
        // of the part as it can.
        let lead = p
            .members
            .iter()
            .filter_map(|&i| hf.stacks.get(i))
            .max_by(|a, b| {
                let k = |s: &HoleStack| dot(s.entry_point, add(p.dirs[0], p.dirs[1]));
                k(a).partial_cmp(&k(b)).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(first);
        let d_entry = first.entry_diameter();
        out.push(DimensionRecord {
            id: id(),
            kind: "hole_callout".into(),
            value: d_entry,
            unit: template.clone(),
            label: p.label.clone(),
            entities: all_faces,
            anchor: add(lead.entry_point, scale(p.dirs[0], 0.5 * d_entry)),
            direction: p.dirs[0],
            axis: Some(first.axis),
            pid: None,
            datum: None,
        });
        for mut l in locs {
            l.id = id();
            l.kind = "location".into();
            out.push(l);
        }
        let fmt = |v: f64| unit.format_len_bare(v);
        for k in 0..2 {
            if p.counts[k] < 2 {
                continue;
            }
            let step = if k == 0 { 1 } else { p.counts[0] };
            let (Some(a), Some(b)) = (
                p.members.first().and_then(|&i| hf.stacks.get(i)),
                p.members.get(step).and_then(|&i| hf.stacks.get(i)),
            ) else {
                continue;
            };
            let n = p.counts[k] - 1;
            let label = if n >= 2 {
                format!(
                    "{n}\u{00D7} {} (={})",
                    fmt(p.pitch[k]),
                    fmt(p.pitch[k] * n as f64)
                )
            } else {
                fmt(p.pitch[k])
            };
            let mut ents = a.drill_faces.clone();
            ents.extend(b.drill_faces.iter().copied());
            out.push(DimensionRecord {
                id: id(),
                kind: "pitch".into(),
                value: p.pitch[k],
                unit: template.clone(),
                label,
                entities: ents,
                anchor: scale(add(a.entry_point, b.entry_point), 0.5),
                direction: p.dirs[k],
                axis: Some(first.axis),
                pid: None,
                datum: None,
            });
        }
    }
    out
}

/// The live stack whose drilled wall shares a face with `stack`'s.
fn live_counterpart<'a>(live: &'a [HoleStack], stack: &HoleStack) -> Option<&'a HoleStack> {
    live.iter()
        .find(|l| l.drill_faces.iter().any(|f| stack.drill_faces.contains(f)))
}

/// Re-recognise `stack` on the LIVE model and re-measure every claim it makes:
/// THRU/blind, drill Ø, depth, counterbore Ø and depth, countersink Ø and
/// angle, drill-point angle, axis and entry position. `None` when every claim
/// holds, else the first that does not, in words.
pub fn hole_stack_live_check(
    model: &BRepModel,
    solid_id: SolidId,
    stack: &HoleStack,
    tol_mm: f64,
) -> Option<String> {
    let live = recognize_hole_stacks(model, solid_id);
    check_against(&live, stack, tol_mm)
}

fn check_against(live: &[HoleStack], stack: &HoleStack, tol_mm: f64) -> Option<String> {
    let Some(l) = live_counterpart(live, stack) else {
        return Some(format!(
            "the {} hole is no longer a recognisable hole on the model",
            stack.label()
        ));
    };
    if let Some(m) = stack.spec_mismatch(l, tol_mm) {
        return Some(m);
    }
    let moved = norm(sub(l.entry_point, stack.entry_point));
    (moved > tol_mm).then(|| format!("the hole has moved {moved:.3} mm"))
}

/// Re-measure a pattern callout on the LIVE model: every member still the
/// stated hole, still at its grid position (first member + pitch).
pub fn hole_pattern_live_check(
    model: &BRepModel,
    solid_id: SolidId,
    hf: &HoleFeatures,
    pattern: &HolePattern,
    tol_mm: f64,
) -> Option<String> {
    let live = recognize_hole_stacks(model, solid_id);
    let mut live_members: Vec<&HoleStack> = Vec::new();
    for (n, &i) in pattern.members.iter().enumerate() {
        let Some(sheet) = hf.stacks.get(i) else {
            return Some(format!(
                "pattern member {} is missing from the sheet",
                n + 1
            ));
        };
        let Some(l) = live_counterpart(&live, sheet) else {
            return Some(format!("pattern member {} no longer exists", n + 1));
        };
        if let Some(m) = sheet.spec_mismatch(l, tol_mm) {
            return Some(format!("pattern member {}: {m}", n + 1));
        }
        live_members.push(l);
    }
    let Some(first) = live_members.first() else {
        return Some("the pattern has no members".to_string());
    };
    let mut by_cell: BTreeMap<usize, [f64; 3]> = BTreeMap::new();
    for (n, l) in live_members.iter().enumerate() {
        by_cell.insert(n, l.entry_point);
    }
    for (n, p) in by_cell {
        let iu = n % pattern.counts[0].max(1);
        let iv = n / pattern.counts[0].max(1);
        let want = add(
            first.entry_point,
            add(
                scale(pattern.dirs[0], iu as f64 * pattern.pitch[0]),
                scale(pattern.dirs[1], iv as f64 * pattern.pitch[1]),
            ),
        );
        let off = norm(sub(p, want));
        if off > tol_mm {
            return Some(format!(
                "pattern member {} is {off:.3} mm off its pitch position",
                n + 1
            ));
        }
    }
    None
}

/// Restate hole-table rows from their stacks: a row whose bore is a stack's
/// drilled wall reads the stack's size (`Ø6.60 ⌴ Ø11.00 ↧ 6.80`) and its
/// THRU / blind depth as recognised from topology, then every row is re-tagged
/// so a group letter still names ONE feature (a counterbored Ø6.60 and a plain
/// Ø6.60 are different holes). Rows the sheet states by a callout instead
/// (`called_out` faces) are tagged after the tabled ones, so the visible
/// table starts at `A1`.
pub(crate) fn restate_hole_sites(
    sites: &mut [super::hole_table::HoleSite],
    hf: &HoleFeatures,
    called_out: &HashSet<u32>,
) {
    for site in sites.iter_mut() {
        let Some(stack) = site
            .face_entities
            .iter()
            .find_map(|&f| hf.stack_of_drill_face(f))
        else {
            continue;
        };
        site.dia_label = stack.table_size_label();
        site.depth_label = stack.depth_label.clone();
        site.is_through = stack.through;
    }
    // Re-tag: tabled rows first, then by drill size, THRU before blind, the
    // stated size, and position — the order `build_hole_table` uses, with the
    // size label joining the group key.
    let class = |s: &super::hole_table::HoleSite| -> u8 {
        match (s.depth_mm, s.is_through) {
            (Some(_), true) => 0,
            (Some(_), false) => 1,
            (None, _) => 2,
        }
    };
    let hidden =
        |s: &super::hole_table::HoleSite| s.face_entities.iter().any(|f| called_out.contains(f));
    sites.sort_by(|a, b| {
        hidden(a)
            .cmp(&hidden(b))
            .then(
                ((a.diameter_mm * 100.0).round() as i64)
                    .cmp(&((b.diameter_mm * 100.0).round() as i64)),
            )
            .then(class(a).cmp(&class(b)))
            .then(a.dia_label.cmp(&b.dia_label))
            .then(
                a.x_mm
                    .partial_cmp(&b.x_mm)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(
                a.y_mm
                    .partial_cmp(&b.y_mm)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    let mut groups: Vec<(bool, i64, u8, String)> = Vec::new();
    let mut counters: Vec<usize> = Vec::new();
    for s in sites.iter_mut() {
        let key = (
            hidden(s),
            (s.diameter_mm * 100.0).round() as i64,
            class(s),
            s.dia_label.clone(),
        );
        let gi = match groups.iter().position(|g| *g == key) {
            Some(i) => i,
            None => {
                groups.push(key);
                counters.push(0);
                groups.len() - 1
            }
        };
        counters[gi] += 1;
        let letter = super::hole_table::tag_letter(gi);
        s.tag = format!("{letter}{}", counters[gi]);
        s.group = letter;
    }
}
