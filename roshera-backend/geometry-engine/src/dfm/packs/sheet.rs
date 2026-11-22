//! Sheet-metal rule pack — "can this be cut from flat stock and bent, and
//! what will tear or distort?"
//!
//! ## What a sheet part is, read off the B-Rep
//!
//! * **Thickness `t`** — given, or inferred as the THINNEST proven wall
//!   pair ([`crate::dfm::analyzers::pair_thickness`]) — and inferred ONLY
//!   when every face was paired (an unpaired face may bound a thinner wall,
//!   leaving the thinnest proven pair the part's width). Otherwise, with no
//!   given gauge, every rule that needs `t` refuses by name.
//! * **Bends** — partial-turn cylinders (the bend's inside is a cavity
//!   cylinder, its outside a convex one); a SHARP concave edge is a bend of
//!   inside radius 0.
//! * **Bend lines on a flange** — the flange face's Line edges shared with
//!   a partial-turn cylinder, and its sharp concave Line edges.
//! * **Free edges** — a flange face's Line edges whose neighbour is a plane
//!   perpendicular to the flange spanning exactly `t` along the flange
//!   normal (the sheet's cut edge). Any other neighbour leaves the edge
//!   unclassified and the distance rules refuse on it.
//! * **Holes** — inner loops of a flange face made only of circles/arcs
//!   about one centre; any other inner loop (a slot, a cut-out) refuses.
//!
//! Every distance is exact: point-to-segment / point-to-line in the flange
//! plane, from a hole centre (minus its radius) or a flange vertex.
//!
//! | rule | claim |
//! |---|---|
//! | `sheet.uniform_thickness` | no proven wall pair is thinner than `t − thickness_tolerance` (thicker pairs are the part's width/length and are not judged); unpaired faces refuse |
//! | `sheet.bend_radius` | every bend's inside radius (sharp concave edge = 0) ≥ `min_bend_radius_factor · t` |
//! | `sheet.hole_to_edge` | every hole rim to every free edge ≥ `hole_edge_factor · t` |
//! | `sheet.hole_to_bend` | every hole rim to every bend line of its flange ≥ `hole_bend_factor · t + R_inside` (R of that bend; 0 for a sharp one) |
//! | `sheet.flange_length` | every flange with a bend line: its farthest outer vertex from that line ≥ `min_flange_factor · t` |
//! | `sheet.developable` | no sphere/torus face (non-zero Gaussian curvature cannot be bent from flat); freeform refuses |
//!
//! All factors are shop practice; material-specific bend tables are the
//! caller's (`kb_lookup` reference data), never assumed here. Not checked:
//! bend relief, K-factor flat-pattern length, grain direction, hem/curl.

use std::collections::BTreeMap;

use crate::dfm::analyzers::features::{concave_features, dot3, ConcaveFeatures, EdgeShape};
use crate::dfm::packs::common::{
    analytic, analytic_on, solid_face_ids, solid_wall_pairs, RuleFold, Sense,
};
use crate::dfm::packs::{Rule, RulePack};
use crate::dfm::provenance::RuleProvenance;
use crate::dfm::report::{
    DfmError, DfmReport, PackParams, RuleVerdict, SheetMetalParams, SurfaceKind, UnverifiableReason,
};
use crate::primitives::curve::{Arc, Circle, Line};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::adjacency::{EdgeConvexity, FaceGeometry};

pub const UNIFORM_THICKNESS_RULE_ID: &str = "sheet.uniform_thickness";
pub const BEND_RADIUS_RULE_ID: &str = "sheet.bend_radius";
pub const HOLE_TO_EDGE_RULE_ID: &str = "sheet.hole_to_edge";
pub const HOLE_TO_BEND_RULE_ID: &str = "sheet.hole_to_bend";
pub const FLANGE_LENGTH_RULE_ID: &str = "sheet.flange_length";
pub const DEVELOPABLE_RULE_ID: &str = "sheet.developable";

fn practice(note: &str) -> RuleProvenance {
    RuleProvenance::ShopPractice {
        note: note.to_string(),
    }
}
pub fn uniform_thickness_provenance() -> RuleProvenance {
    practice("a sheet part is cut from stock of one gauge (definition of the process)")
}
pub fn bend_radius_provenance() -> RuleProvenance {
    practice(
        "inside bend radius >= 1.0 t default (0.5 t soft alloys .. 1.5 t hard alloys; resolve per \
         material) — press-brake practice. Related standard DIN 6935:2011-10 (cold bending of \
         flat steel; current per din.de, checked 2026-11-22) — its radius tables were NOT \
         transcribed, so it is not cited as authority",
    )
}
pub fn hole_to_edge_provenance() -> RuleProvenance {
    practice(
        "holes >= 2 t from a cut edge or the web bulges (Xometry sheet-metal design guide, \
         https://www.xometry.com/resources/design-guides/design-guide-sheet-metal-fabrication/, \
         accessed 2026-11-22; others quote 1.5-3 t) — vendor practice",
    )
}
pub fn hole_to_bend_provenance() -> RuleProvenance {
    practice("holes >= 2.5 t + R from a bend or they distort when bent — press-brake practice")
}
pub fn flange_length_provenance() -> RuleProvenance {
    practice(
        "flange >= 4 t so the press-brake die can grip it (Xometry sheet-metal design guide, \
         accessed 2026-11-22; DFMPro material quotes 3.5 t) — vendor practice",
    )
}
pub fn developable_provenance() -> RuleProvenance {
    practice(
        "bending from flat stock produces only developable (zero Gaussian curvature) faces \
         (geometric fact); doubly curved faces need forming/stamping",
    )
}

pub fn rule_pack(params: PackParams) -> RulePack {
    let r = |id, provenance| Rule { id, provenance };
    RulePack {
        params,
        rules: vec![
            r(UNIFORM_THICKNESS_RULE_ID, uniform_thickness_provenance()),
            r(BEND_RADIUS_RULE_ID, bend_radius_provenance()),
            r(HOLE_TO_EDGE_RULE_ID, hole_to_edge_provenance()),
            r(HOLE_TO_BEND_RULE_ID, hole_to_bend_provenance()),
            r(FLANGE_LENGTH_RULE_ID, flange_length_provenance()),
            r(DEVELOPABLE_RULE_ID, developable_provenance()),
        ],
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn norm(a: [f64; 3]) -> f64 {
    dot3(a, a).sqrt()
}
fn p3(p: crate::math::Point3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

/// A straight boundary edge of a flange: endpoints.
#[derive(Debug, Clone, Copy)]
struct Segment {
    a: [f64; 3],
    b: [f64; 3],
}

impl Segment {
    fn distance(&self, p: [f64; 3]) -> f64 {
        let ab = sub(self.b, self.a);
        let l2 = dot3(ab, ab);
        let t = if l2 > 0.0 {
            (dot3(sub(p, self.a), ab) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        norm(sub(
            p,
            [
                self.a[0] + ab[0] * t,
                self.a[1] + ab[1] * t,
                self.a[2] + ab[2] * t,
            ],
        ))
    }
    /// Distance from `p` to the infinite line through the segment.
    fn line_distance(&self, p: [f64; 3]) -> f64 {
        let ab = sub(self.b, self.a);
        let l = norm(ab);
        if l == 0.0 {
            return norm(sub(p, self.a));
        }
        let ap = sub(p, self.a);
        let c = [
            ap[1] * ab[2] - ap[2] * ab[1],
            ap[2] * ab[0] - ap[0] * ab[2],
            ap[0] * ab[1] - ap[1] * ab[0],
        ];
        norm(c) / l
    }
}

fn segment_of(model: &BRepModel, edge: EdgeId) -> Option<Segment> {
    let e = model.edges.get(edge)?;
    let c = model.curves.get(e.curve_id)?;
    c.as_any().downcast_ref::<Line>()?;
    Some(Segment {
        a: p3(e.evaluate(0.0, &model.curves).ok()?),
        b: p3(e.evaluate(1.0, &model.curves).ok()?),
    })
}

fn circle_of(model: &BRepModel, edge: EdgeId) -> Option<([f64; 3], f64)> {
    let e = model.edges.get(edge)?;
    let c = model.curves.get(e.curve_id)?;
    let any = c.as_any();
    if let Some(a) = any.downcast_ref::<Arc>() {
        return Some((p3(a.center), a.radius));
    }
    any.downcast_ref::<Circle>()
        .map(|c| (p3(c.center()), c.radius()))
}

/// How a flange's straight boundary edge is classified.
enum EdgeRole {
    /// A cut edge of the sheet.
    Free,
    /// A bend line; the bend's inside radius.
    Bend { inside_radius: f64 },
    /// Neither could be proven.
    Unclassified,
}

struct Flange {
    face: FaceId,
    /// Straight outer-loop edges with their role.
    edges: Vec<(EdgeId, Segment, EdgeRole)>,
    /// Outer loop has a non-straight edge (its extent is not read exactly).
    curved_outer: bool,
    /// Holes: (inner-loop edges' faces witness, centre, radius).
    holes: Vec<([f64; 3], f64)>,
    /// Inner loops that are not a single circle.
    odd_inner_loops: usize,
}

fn other_face(
    features: &ConcaveFeatures,
    edge: EdgeId,
    face: FaceId,
) -> Option<(FaceId, EdgeConvexity)> {
    features
        .graph
        .neighbours(face)
        .into_iter()
        .find(|(a, _)| a.edge == edge)
        .map(|(a, o)| (o, a.convexity))
}

/// Extent of a planar face's straight-edge vertices along `dir`, or `None`
/// when any boundary edge is not straight.
fn extent_along(model: &BRepModel, face: FaceId, dir: [f64; 3]) -> Option<f64> {
    let f = model.faces.get(face)?;
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut loops = vec![f.outer_loop];
    loops.extend_from_slice(&f.inner_loops);
    for lid in loops {
        for &e in &model.loops.get(lid)?.edges {
            let s = segment_of(model, e)?;
            for p in [s.a, s.b] {
                let v = dot3(p, dir);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    (hi >= lo).then_some(hi - lo)
}

/// Is the sharp concave straight edge between `faces` (direction `dir`) a
/// CUT-OUT corner — both faces cut faces spanning exactly `t` along the
/// edge — rather than a bend? `None` when a face's extent along the edge
/// cannot be read exactly (a curved boundary edge).
fn cut_corner(
    model: &BRepModel,
    faces: [FaceId; 2],
    dir: [f64; 3],
    t: f64,
    tol: f64,
) -> Option<bool> {
    let a = extent_along(model, faces[0], dir)?;
    let b = extent_along(model, faces[1], dir)?;
    Some((a - t).abs() <= tol && (b - t).abs() <= tol)
}

fn flanges(model: &BRepModel, features: &ConcaveFeatures, t: f64, tol: f64) -> Vec<Flange> {
    // Bend cylinders by face: partial-turn cylinders (cavity or convex).
    let mut bend_radius_of_face: BTreeMap<FaceId, f64> = BTreeMap::new();
    for c in &features.cavity_cylinders {
        if c.is_partial() {
            for &f in &c.faces {
                bend_radius_of_face.insert(f, c.radius);
            }
        }
    }
    for n in &features.graph.nodes {
        if n.cavity == Some(false) {
            if let FaceGeometry::Cylinder { radius, .. } = n.geometry {
                // Outside of a bend: inside radius = R_out - t.
                bend_radius_of_face
                    .entry(n.face)
                    .or_insert((radius - t).max(0.0));
            }
        }
    }

    let mut out = Vec::new();
    for n in &features.graph.nodes {
        let FaceGeometry::Plane { normal, .. } = n.geometry else {
            continue;
        };
        // A face spanning exactly t along some neighbour's normal is a cut
        // (thickness) face, not a flange: flanges are the skins.
        let Some(face) = model.faces.get(n.face) else {
            continue;
        };
        let Some(outer) = model.loops.get(face.outer_loop) else {
            continue;
        };
        let mut edges = Vec::new();
        let mut curved_outer = false;
        for &e in &outer.edges {
            let Some(seg) = segment_of(model, e) else {
                curved_outer = true;
                continue;
            };
            let role = match other_face(features, e, n.face) {
                Some((o, conv)) => {
                    if let Some(&r) = bend_radius_of_face.get(&o) {
                        if conv == EdgeConvexity::Tangent {
                            EdgeRole::Bend { inside_radius: r }
                        } else {
                            EdgeRole::Unclassified
                        }
                    } else if conv == EdgeConvexity::Concave {
                        let d = sub(seg.b, seg.a);
                        let l = norm(d);
                        let dir = if l > 0.0 {
                            [d[0] / l, d[1] / l, d[2] / l]
                        } else {
                            [0.0; 3]
                        };
                        match cut_corner(model, [n.face, o], dir, t, tol) {
                            Some(false) => EdgeRole::Bend { inside_radius: 0.0 },
                            // A cut-out corner, or undecided: never a bend line.
                            _ => EdgeRole::Unclassified,
                        }
                    } else {
                        match features.geometry(o) {
                            Some(FaceGeometry::Plane { normal: on, .. })
                                if dot3(*on, normal).abs() <= 1e-9
                                    && extent_along(model, o, normal)
                                        .is_some_and(|w| (w - t).abs() <= tol) =>
                            {
                                EdgeRole::Free
                            }
                            _ => EdgeRole::Unclassified,
                        }
                    }
                }
                None => EdgeRole::Unclassified,
            };
            edges.push((e, seg, role));
        }
        let mut holes = Vec::new();
        let mut odd = 0usize;
        for &lid in &face.inner_loops {
            let Some(lp) = model.loops.get(lid) else {
                odd += 1;
                continue;
            };
            let circles: Option<Vec<([f64; 3], f64)>> =
                lp.edges.iter().map(|&e| circle_of(model, e)).collect();
            match circles.as_deref() {
                Some([first, rest @ ..])
                    if rest.iter().all(|(c, r)| {
                        norm(sub(*c, first.0)) <= 1e-6 * first.1.max(1.0)
                            && (r - first.1).abs() <= 1e-6 * first.1.max(1.0)
                    }) =>
                {
                    holes.push(*first)
                }
                _ => odd += 1,
            }
        }
        out.push(Flange {
            face: n.face,
            edges,
            curved_outer,
            holes,
            odd_inner_loops: odd,
        });
    }
    out
}

fn refuse_no_t(rule: &'static str, prov: RuleProvenance, faces: &[FaceId]) -> RuleVerdict {
    let mut f = RuleFold::new(rule, prov, Sense::AtLeast, 0.0, "n/a: thickness unknown");
    f.refuse(
        faces.to_vec(),
        UnverifiableReason::UnsupportedTopology {
            detail: "sheet thickness unknown: no proven wall pair and no thickness given"
                .to_string(),
        },
    );
    f.finish()
}

pub fn evaluate(
    model: &BRepModel,
    solid_id: SolidId,
    params: &SheetMetalParams,
) -> Result<DfmReport, DfmError> {
    let faces = solid_face_ids(model, solid_id)?;
    let features = concave_features(model, solid_id)?;
    let walls = solid_wall_pairs(model, &faces)?;
    // The thinnest proven pair is the gauge ONLY when every face was
    // paired: an unpaired face may bound a thinner wall, and then the
    // thinnest PROVEN pair is merely the part's width — never a gauge.
    let inferred = if walls.unverifiable.is_empty() {
        walls
            .pairs
            .iter()
            .map(|p| p.thickness.value)
            .fold(None::<f64>, |m, v| Some(m.map_or(v, |m| m.min(v))))
    } else {
        None
    };
    let t = params.thickness.or(inferred);

    let developable = {
        let mut f = RuleFold::new(
            DEVELOPABLE_RULE_ID,
            developable_provenance(),
            Sense::AtMost,
            0.0,
            "sheet.developable limit: no doubly curved face",
        );
        let mut bad = Vec::new();
        for n in &features.graph.nodes {
            match &n.geometry {
                FaceGeometry::Sphere { .. } | FaceGeometry::Torus { .. } => bad.push(n.face),
                FaceGeometry::Other { .. } if features.resolved_fillets.contains(&n.face) => {}
                FaceGeometry::Other { type_name } => f.refuse(
                    vec![n.face],
                    UnverifiableReason::UnsupportedTopology {
                        detail: format!(
                            "face {} is a {type_name} surface: developability not decided",
                            n.face
                        ),
                    },
                ),
                _ => {}
            }
        }
        f.observe(
            bad.clone(),
            bad.len() as f64,
            analytic("count of sphere/torus (non-zero Gaussian curvature) faces"),
        );
        f.finish()
    };

    let Some(t) = t else {
        return Ok(DfmReport::new(
            PackParams::SheetMetal(params.clone()),
            vec![
                refuse_no_t(
                    UNIFORM_THICKNESS_RULE_ID,
                    uniform_thickness_provenance(),
                    &faces,
                ),
                refuse_no_t(BEND_RADIUS_RULE_ID, bend_radius_provenance(), &faces),
                refuse_no_t(HOLE_TO_EDGE_RULE_ID, hole_to_edge_provenance(), &faces),
                refuse_no_t(HOLE_TO_BEND_RULE_ID, hole_to_bend_provenance(), &faces),
                refuse_no_t(FLANGE_LENGTH_RULE_ID, flange_length_provenance(), &faces),
                developable,
            ],
        ));
    };
    let t_src = if params.thickness.is_some() {
        "given"
    } else {
        "inferred: thinnest proven wall pair"
    };

    // sheet.uniform_thickness — nothing THINNER than the gauge (a coined,
    // milled or mis-modelled region). A pair THICKER than t is not judged:
    // a sheet's own width and length are proven wall pairs too, and are
    // indistinguishable from a thick section by pairing alone.
    let mut uniform = RuleFold::new(
        UNIFORM_THICKNESS_RULE_ID,
        uniform_thickness_provenance(),
        Sense::AtLeast,
        t - params.thickness_tolerance,
        format!("sheet.uniform_thickness limit: t ({t_src}) - thickness_tolerance"),
    );
    for p in &walls.pairs {
        uniform.observe(
            vec![p.face_a, p.face_b],
            p.thickness.value,
            p.thickness.derivation.clone(),
        );
    }
    for u in &walls.unverifiable {
        uniform.refuse(vec![u.face], u.reason.clone());
    }

    let tol = params.thickness_tolerance.max(1e-9);

    // sheet.bend_radius
    let mut bend = RuleFold::new(
        BEND_RADIUS_RULE_ID,
        bend_radius_provenance(),
        Sense::AtLeast,
        params.min_bend_radius_factor * t,
        format!("sheet.bend_radius limit: min_bend_radius_factor x t ({t_src})"),
    );
    for e in &features.sharp_edges {
        if let EdgeShape::Line { direction } = e.shape {
            // A CUT-OUT corner (a laser-cut window's inside corner) is a
            // concave straight edge too — but it runs THROUGH the sheet:
            // both of its faces are cut faces spanning exactly t along it.
            // A bend's inside edge runs ACROSS the sheet's width.
            match cut_corner(model, e.faces, direction, t, tol) {
                Some(true) => continue,
                Some(false) => {}
                None => {
                    bend.refuse(
                        e.faces.to_vec(),
                        UnverifiableReason::UnsupportedTopology {
                            detail: format!(
                                "concave edge {}: cannot tell a cut-out corner from a bend \
                                 (a face's extent along the edge is not read exactly)",
                                e.edge
                            ),
                        },
                    );
                    continue;
                }
            }
            bend.observe(
                e.faces.to_vec(),
                0.0,
                analytic(format!(
                    "sharp concave bend (edge {}): inside radius 0",
                    e.edge
                )),
            );
        } else {
            bend.refuse(
                e.faces.to_vec(),
                UnverifiableReason::UnsupportedTopology {
                    detail: format!(
                        "concave edge {} is curved: not a press-brake bend line",
                        e.edge
                    ),
                },
            );
        }
    }
    for c in features.cavity_cylinders.iter().filter(|c| c.is_partial()) {
        bend.observe(
            c.faces.clone(),
            c.radius,
            analytic_on(
                SurfaceKind::Cylinder,
                "bend inside radius (partial cavity cylinder)",
            ),
        );
    }
    for u in &features.undetermined {
        bend.refuse(
            u.faces.to_vec(),
            UnverifiableReason::UnsupportedTopology {
                detail: format!("edge {}: convexity could not be derived", u.edge),
            },
        );
    }

    // Flange-level distance rules.
    let fl = flanges(model, &features, t, tol);
    let mut to_edge = RuleFold::new(
        HOLE_TO_EDGE_RULE_ID,
        hole_to_edge_provenance(),
        Sense::AtLeast,
        params.hole_edge_factor * t,
        format!("sheet.hole_to_edge limit: hole_edge_factor x t ({t_src})"),
    );
    let mut to_bend = RuleFold::new(
        HOLE_TO_BEND_RULE_ID,
        hole_to_bend_provenance(),
        Sense::AtLeast,
        0.0,
        "sheet.hole_to_bend: per bend, distance - (hole_bend_factor x t + R_inside) >= 0",
    );
    let mut flange_len = RuleFold::new(
        FLANGE_LENGTH_RULE_ID,
        flange_length_provenance(),
        Sense::AtLeast,
        params.min_flange_factor * t,
        format!("sheet.flange_length limit: min_flange_factor x t ({t_src})"),
    );
    for f in &fl {
        if !f.holes.is_empty() || f.odd_inner_loops > 0 {
            if f.odd_inner_loops > 0 {
                let why = UnverifiableReason::UnsupportedTopology {
                    detail: format!(
                        "face {}: an inner loop is not a single circular hole (slot / cut-out)",
                        f.face
                    ),
                };
                to_edge.refuse(vec![f.face], why.clone());
                to_bend.refuse(vec![f.face], why);
            }
            if f.curved_outer {
                to_edge.refuse(
                    vec![f.face],
                    UnverifiableReason::UnsupportedTopology {
                        detail: format!(
                            "face {}: a curved outer edge; hole-to-edge distance not read exactly",
                            f.face
                        ),
                    },
                );
            }
            for &(c, r) in &f.holes {
                for (e, seg, role) in &f.edges {
                    match role {
                        EdgeRole::Free => to_edge.observe(
                            vec![f.face],
                            seg.distance(c) - r,
                            analytic(format!(
                                "hole rim (centre {c:?}, r {r:.4}) to cut edge {e}: \
                                 point-segment distance - r"
                            )),
                        ),
                        EdgeRole::Bend { inside_radius } => {
                            let need = params.hole_bend_factor * t + inside_radius;
                            to_bend.observe(
                                vec![f.face],
                                seg.line_distance(c) - r - need,
                                analytic(format!(
                                    "hole rim (centre {c:?}, r {r:.4}) to bend line {e}: \
                                     distance - r - ({:.3} t + R {inside_radius:.4})",
                                    params.hole_bend_factor
                                )),
                            );
                        }
                        EdgeRole::Unclassified => {
                            let why = UnverifiableReason::UnsupportedTopology {
                                detail: format!(
                                    "face {} edge {e}: neither a proven cut edge nor a bend line",
                                    f.face
                                ),
                            };
                            to_edge.refuse(vec![f.face], why.clone());
                            to_bend.refuse(vec![f.face], why);
                        }
                    }
                }
            }
        }
        // Flange length: faces with a bend line.
        for (e, seg, role) in &f.edges {
            if !matches!(role, EdgeRole::Bend { .. }) {
                continue;
            }
            if f.curved_outer {
                flange_len.refuse(
                    vec![f.face],
                    UnverifiableReason::UnsupportedTopology {
                        detail: format!(
                            "flange {}: a curved outer edge; its length is not read exactly",
                            f.face
                        ),
                    },
                );
                continue;
            }
            let far = f
                .edges
                .iter()
                .flat_map(|(_, s, _)| [s.a, s.b])
                .map(|p| seg.line_distance(p))
                .fold(0.0f64, f64::max);
            flange_len.observe(
                vec![f.face],
                far,
                analytic(format!(
                    "flange length: farthest outer vertex from bend line {e}"
                )),
            );
        }
    }

    Ok(DfmReport::new(
        PackParams::SheetMetal(params.clone()),
        vec![
            uniform.finish(),
            bend.finish(),
            to_edge.finish(),
            to_bend.finish(),
            flange_len.finish(),
            developable,
        ],
    ))
}
