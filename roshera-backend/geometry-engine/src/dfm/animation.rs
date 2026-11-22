//! DFM finding animations — a deterministic, serializable descriptor that
//! EXPLAINS one violation over the part in the viewport.
//!
//! ## The kernel stays the source of truth
//!
//! An animation is built from a [`RuleVerdict`] that already exists — its
//! `measured`, `limit` and `witnesses` — plus the part's own geometry for
//! WHERE to show it. It invents no geometry and no number: the end mill's
//! diameter is the rule's limit, the drill's plunge depth is the measured
//! drilled length, the caliper's gap is the measured wall. Change the
//! measurement and the descriptor changes with it (the RED test pins
//! exactly that). The frontend player only interpolates the keyframes.
//!
//! ## What a descriptor is
//!
//! * `kind` — which explanation (tool approach, drill plunge, setup cycle,
//!   mould separation, layer build, caliper, spindle spin, highlight).
//! * `actors` — tools and guides, each a shape with kernel-mm dimensions
//!   and keyframes (`t ∈ [0, 1]` of `duration_s`): the shape's local `+Z`
//!   is placed along `axis`, its tip/origin at `position`.
//! * `face_tracks` — groups of the part's faces with keyframed display
//!   offsets and a state (`fail` / `ok` / `context`), e.g. mould halves
//!   separating along the pull.
//! * `caption` — the finding in one line, every number from the verdict.
//!
//! Anchors (a face's centroid, the part's extent along a direction) are
//! read from the faces' trim-edge points: DISPLAY placement, never a
//! measurement any rule decides on.

use serde::{Deserialize, Serialize};

use crate::dfm::analyzers::features::{concave_features, dot3, ConcaveFeatures, EdgeShape};
use crate::dfm::analyzers::holes::{hole_census, HoleCensus};
use crate::dfm::report::{DfmReport, FaceRef, PackParams, RuleVerdict, Verdict};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::adjacency::FaceGeometry;

/// Which explanation an animation plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationKind {
    /// A cutter approaches along a setup direction and stops short.
    ToolApproach,
    /// A drill plunges along a hole's axis to its measured depth.
    DrillPlunge,
    /// Setup directions shown one by one; none lines up with the feature.
    SetupCycle,
    /// The two mould / die halves separate along the pull direction.
    MoldSeparation,
    /// A build plane rises layer by layer; unsupported faces light up.
    LayerBuild,
    /// Caliper jaws close on a wall to its measured thickness.
    Caliper,
    /// The part spins about the spindle axis; off-axis faces flash.
    SpindleSpin,
    /// The offending faces flash with the caption (no motion explains it).
    Highlight,
}

/// A tool or guide shape, kernel millimetres. Local `+Z` is the shape's
/// axis; its origin is the tip (tools), the tail (arrow), or the centre
/// (plane, caliper).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActorShape {
    EndMill {
        diameter: f64,
        flute_length: f64,
        length: f64,
    },
    Drill {
        diameter: f64,
        point_angle_deg: f64,
        length: f64,
    },
    Arrow {
        length: f64,
    },
    /// A square plane of side `size` (a build layer, a parting plane).
    Plane {
        size: f64,
    },
    /// Caliper jaws `opening` apart along local `+Z`.
    Caliper {
        opening: f64,
    },
    /// An axis line of `length`.
    Axis {
        length: f64,
    },
}

/// One keyframe of an actor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActorKey {
    /// Normalized time, `0 ≤ t ≤ 1`.
    pub t: f64,
    pub position: [f64; 3],
    /// Unit direction the shape's local `+Z` points along.
    pub axis: [f64; 3],
    pub opacity: f64,
    /// Shape-specific scalar: a caliper's current opening; `0` otherwise.
    #[serde(default)]
    pub extent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    /// `tool` / `guide` / `needed` (the direction the feature needs).
    pub role: String,
    pub shape: ActorShape,
    pub keys: Vec<ActorKey>,
}

/// How a face group is shown at a keyframe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaceState {
    Context,
    Fail,
    Ok,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceKey {
    pub t: f64,
    /// Display translation of the group, kernel mm.
    pub offset: [f64; 3],
    /// Rotation about the animation's `pivot`/`spin_axis`, degrees.
    #[serde(default)]
    pub rotation_deg: f64,
    pub state: FaceState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceTrack {
    pub faces: Vec<FaceRef>,
    pub keys: Vec<FaceKey>,
}

/// One finding, explained.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DfmAnimation {
    pub rule: String,
    pub kind: AnimationKind,
    pub duration_s: f64,
    pub caption: String,
    /// The verdict's measured value and limit, verbatim.
    pub measured: f64,
    pub limit: f64,
    /// The verdict's witnesses (the faces the finding is about).
    pub faces: Vec<FaceRef>,
    pub actors: Vec<Actor>,
    pub face_tracks: Vec<FaceTrack>,
    /// For `spindle_spin`: the axis line `(point, direction)` faces turn about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spin_axis: Option<([f64; 3], [f64; 3])>,
}

// ── geometry helpers (display anchors) ──────────────────────────────────────

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn neg(a: [f64; 3]) -> [f64; 3] {
    scale(a, -1.0)
}
fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let n = dot3(a, a).sqrt();
    (n > 1e-12).then(|| scale(a, 1.0 / n))
}

/// Trim-edge sample points of a face.
fn face_points(model: &BRepModel, face: FaceId) -> Vec<[f64; 3]> {
    let Some(f) = model.faces.get(face) else {
        return Vec::new();
    };
    let mut loops = vec![f.outer_loop];
    loops.extend_from_slice(&f.inner_loops);
    let mut out = Vec::new();
    for lid in loops {
        let Some(lp) = model.loops.get(lid) else {
            continue;
        };
        for &e in &lp.edges {
            if let Some(edge) = model.edges.get(e) {
                for k in 0..=8 {
                    if let Ok(p) = edge.evaluate(k as f64 / 8.0, &model.curves) {
                        out.push([p.x, p.y, p.z]);
                    }
                }
            }
        }
    }
    out
}

fn centroid(points: &[[f64; 3]]) -> Option<[f64; 3]> {
    if points.is_empty() {
        return None;
    }
    let s = points.iter().fold([0.0; 3], |a, p| add(a, *p));
    Some(scale(s, 1.0 / points.len() as f64))
}

fn faces_centroid(model: &BRepModel, faces: &[FaceId]) -> Option<[f64; 3]> {
    let pts: Vec<[f64; 3]> = faces.iter().flat_map(|&f| face_points(model, f)).collect();
    centroid(&pts)
}

/// Part extent `(min, max)` along `dir` and its diagonal size.
fn part_extent(model: &BRepModel, features: &ConcaveFeatures, dir: [f64; 3]) -> (f64, f64, f64) {
    let pts: Vec<[f64; 3]> = features
        .graph
        .nodes
        .iter()
        .flat_map(|n| face_points(model, n.face))
        .collect();
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut bmin = [f64::INFINITY; 3];
    let mut bmax = [f64::NEG_INFINITY; 3];
    for p in &pts {
        let v = dot3(*p, dir);
        lo = lo.min(v);
        hi = hi.max(v);
        for i in 0..3 {
            bmin[i] = bmin[i].min(p[i]);
            bmax[i] = bmax[i].max(p[i]);
        }
    }
    if pts.is_empty() {
        return (0.0, 0.0, 1.0);
    }
    let diag = dot3(sub(bmax, bmin), sub(bmax, bmin)).sqrt().max(1.0);
    (lo, hi, diag)
}

fn key(t: f64, position: [f64; 3], axis: [f64; 3], opacity: f64) -> ActorKey {
    ActorKey {
        t,
        position,
        axis,
        opacity,
        extent: 0.0,
    }
}

fn fail_track(faces: &[FaceRef]) -> FaceTrack {
    FaceTrack {
        faces: faces.to_vec(),
        keys: vec![
            FaceKey {
                t: 0.0,
                offset: [0.0; 3],
                rotation_deg: 0.0,
                state: FaceState::Context,
            },
            FaceKey {
                t: 0.55,
                offset: [0.0; 3],
                rotation_deg: 0.0,
                state: FaceState::Fail,
            },
            FaceKey {
                t: 1.0,
                offset: [0.0; 3],
                rotation_deg: 0.0,
                state: FaceState::Fail,
            },
        ],
    }
}

fn setups_of(params: &PackParams) -> Vec<[f64; 3]> {
    let raw: Vec<[f64; 3]> = match params {
        PackParams::Cnc3Axis(p) => p.setup_directions.clone(),
        PackParams::Casting(p) => vec![p.pull_direction, neg(p.pull_direction)],
        PackParams::InjectionMolding { pull_direction, .. } => {
            vec![*pull_direction, neg(*pull_direction)]
        }
        _ => vec![[0.0, 0.0, 1.0]],
    };
    raw.into_iter().filter_map(unit).collect()
}

struct Ctx<'a> {
    model: &'a BRepModel,
    features: ConcaveFeatures,
    census: HoleCensus,
    params: &'a PackParams,
}

/// Animations for every VIOLATION in `report` (a pass or an honest refusal
/// has nothing to show). One per violated rule, in report order.
pub fn animate_report(
    model: &BRepModel,
    solid_id: SolidId,
    report: &DfmReport,
) -> Vec<DfmAnimation> {
    let Ok(features) = concave_features(model, solid_id) else {
        return Vec::new();
    };
    let census = hole_census(model, solid_id, &features);
    let ctx = Ctx {
        model,
        features,
        census,
        params: report.params(),
    };
    report
        .verdicts()
        .iter()
        .filter_map(|v| animate_verdict(&ctx, v))
        .collect()
}

fn animate_verdict(ctx: &Ctx<'_>, v: &RuleVerdict) -> Option<DfmAnimation> {
    let Verdict::Violation {
        witnesses,
        measured,
        limit,
    } = &v.verdict
    else {
        return None;
    };
    let (m, l) = (measured.value, limit.value);
    let base = DfmAnimation {
        rule: v.rule.clone(),
        kind: AnimationKind::Highlight,
        duration_s: 4.0,
        caption: format!("{}: measured {m:.3}, limit {l:.3}", v.rule),
        measured: m,
        limit: l,
        faces: witnesses.clone(),
        actors: Vec::new(),
        face_tracks: vec![fail_track(witnesses)],
        spin_axis: None,
    };
    let rule = v.rule.as_str();
    let built = match rule {
        "cnc.internal_corner_radius" => corner_tool(ctx, base.clone(), m, l),
        "cnc.corner_reach" => corner_reach(ctx, base.clone(), m, l),
        "cnc.hole_depth_ratio"
        | "cnc.blind_hole_floor"
        | "cnc.min_hole_diameter"
        | "cnc.standard_drill_size"
        | "cast.cored_hole" => drill_plunge(ctx, base.clone(), m, l),
        "cnc.hole_access" | "cast.side_core" | "mold.side_action" => {
            setup_cycle(ctx, base.clone(), m, l)
        }
        "cast.draft" | "mold.draft" => mold_separation(ctx, base.clone(), m, l),
        "fdm.overhang" => layer_build(ctx, base.clone(), m, l),
        "turn.axisymmetry" => spindle_spin(ctx, base.clone(), m),
        r if r.ends_with("min_wall")
            || r.ends_with("max_wall")
            || r == "sheet.uniform_thickness" =>
        {
            caliper(ctx, base.clone(), m, l)
        }
        _ => None,
    };
    Some(built.unwrap_or(base))
}

/// `cnc.internal_corner_radius`: an end mill of the LIMIT's diameter
/// descends along the corner's setup direction and stops where its side
/// meets the two walls — leaving its own radius where the part has `m`.
fn corner_tool(ctx: &Ctx<'_>, mut a: DfmAnimation, m: f64, l: f64) -> Option<DfmAnimation> {
    let setups = setups_of(ctx.params);
    let tool_r = l;
    // The corner: a sharp concave Line edge between two witness faces, or
    // a witness corner cylinder whose radius is the measured value.
    let (top, bottom, dir) = if m == 0.0 {
        ctx.features.sharp_edges.iter().find_map(|e| {
            if !(a.faces.contains(&e.faces[0]) && a.faces.contains(&e.faces[1])) {
                return None;
            }
            let EdgeShape::Line { direction } = e.shape else {
                return None;
            };
            let d = *setups
                .iter()
                .find(|d| dot3(**d, direction).abs() >= 1.0 - 1e-9)?;
            let n = |f: FaceId| match ctx.features.geometry(f) {
                Some(FaceGeometry::Plane { normal, .. }) => Some(*normal),
                _ => None,
            };
            let void = unit(add(n(e.faces[0])?, n(e.faces[1])?))?;
            let edge = ctx.model.edges.get(e.edge)?;
            let p0 = edge.evaluate(0.0, &ctx.model.curves).ok()?;
            let p1 = edge.evaluate(1.0, &ctx.model.curves).ok()?;
            let (p0, p1) = ([p0.x, p0.y, p0.z], [p1.x, p1.y, p1.z]);
            let (top, bot) = if dot3(p0, d) >= dot3(p1, d) {
                (p0, p1)
            } else {
                (p1, p0)
            };
            // Tool centre sits r / sin(φ/2) off the corner along the void
            // bisector (φ the void angle between the walls).
            let cos_phi = -dot3(n(e.faces[0])?, n(e.faces[1])?);
            let half = (cos_phi.clamp(-1.0, 1.0).acos() / 2.0).sin().max(1e-6);
            let off = scale(void, tool_r / half);
            Some((add(top, off), add(bot, off), d))
        })?
    } else {
        ctx.features.cavity_cylinders.iter().find_map(|c| {
            if !c.faces.iter().any(|f| a.faces.contains(f)) || (c.radius - m).abs() > 1e-9 {
                return None;
            }
            let d = *setups
                .iter()
                .find(|d| dot3(**d, c.axis).abs() >= 1.0 - 1e-9)?;
            let surf = faces_centroid(ctx.model, &c.faces)?;
            let s = dot3(sub(surf, c.origin), c.axis);
            let on_axis = add(c.origin, scale(c.axis, s));
            let void = unit(sub(on_axis, surf))?;
            let off = scale(
                void,
                (tool_r - c.radius).max(0.0) * std::f64::consts::SQRT_2,
            );
            let pa = add(c.origin, scale(c.axis, c.station[0]));
            let pb = add(c.origin, scale(c.axis, c.station[1]));
            let (top, bot) = if dot3(pa, d) >= dot3(pb, d) {
                (pa, pb)
            } else {
                (pb, pa)
            };
            Some((add(top, off), add(bot, off), d))
        })?
    };
    let depth = dot3(sub(top, bottom), dir).abs().max(1.0);
    let length = depth * 2.0;
    let above = add(top, scale(dir, depth));
    a.kind = AnimationKind::ToolApproach;
    a.actors.push(Actor {
        id: "end_mill".into(),
        role: "tool".into(),
        shape: ActorShape::EndMill {
            diameter: 2.0 * tool_r,
            flute_length: depth,
            length,
        },
        keys: vec![
            key(0.0, above, neg(dir), 0.0),
            key(0.15, above, neg(dir), 1.0),
            key(0.6, bottom, neg(dir), 1.0),
            key(1.0, bottom, neg(dir), 1.0),
        ],
    });
    a.caption = format!(
        "Ø{:.2} end mill leaves R{:.2} — this inside corner is R{:.2}",
        2.0 * tool_r,
        tool_r,
        m
    );
    Some(a)
}

/// `cnc.corner_reach`: the largest tool that fits the fillet (Ø 2r) has a
/// cutting length `limit × 2r`; it descends and its flutes run out before
/// the corner's measured depth.
fn corner_reach(ctx: &Ctx<'_>, mut a: DfmAnimation, m: f64, l: f64) -> Option<DfmAnimation> {
    let setups = setups_of(ctx.params);
    let c = ctx.features.cavity_cylinders.iter().find(|c| {
        c.faces.iter().any(|f| a.faces.contains(f))
            && c.radius > 0.0
            && (c.length() / (2.0 * c.radius) - m).abs() <= 1e-9
    })?;
    let d = *setups
        .iter()
        .find(|d| dot3(**d, c.axis).abs() >= 1.0 - 1e-9)?;
    let pa = add(c.origin, scale(c.axis, c.station[0]));
    let pb = add(c.origin, scale(c.axis, c.station[1]));
    let top = if dot3(pa, d) >= dot3(pb, d) { pa } else { pb };
    let dia = 2.0 * c.radius;
    let flute = l * dia;
    let reach_end = add(top, scale(neg(d), flute));
    a.kind = AnimationKind::ToolApproach;
    a.actors.push(Actor {
        id: "end_mill".into(),
        role: "tool".into(),
        shape: ActorShape::EndMill {
            diameter: dia,
            flute_length: flute,
            length: flute + 2.0 * dia,
        },
        keys: vec![
            key(0.0, add(top, scale(d, flute)), neg(d), 0.0),
            key(0.15, add(top, scale(d, flute)), neg(d), 1.0),
            key(0.65, reach_end, neg(d), 1.0),
            key(1.0, reach_end, neg(d), 1.0),
        ],
    });
    a.caption = format!(
        "corner {:.2} deep at R{:.2}: reach L/D {m:.2} > {l:.2} — a Ø{dia:.2} tool cuts only {flute:.2}",
        c.length(),
        c.radius
    );
    Some(a)
}

/// Hole rules: a drill of the hole's diameter plunges from the entry along
/// the hole axis to its measured drilled length.
fn drill_plunge(ctx: &Ctx<'_>, mut a: DfmAnimation, m: f64, l: f64) -> Option<DfmAnimation> {
    let h = ctx
        .census
        .holes
        .iter()
        .find(|h| h.faces.iter().any(|f| a.faces.contains(f)))?;
    let axis = h.stack.axis;
    let entry = h.stack.entry_point;
    let depth = if h.drilled_length.is_finite() {
        h.drilled_length
    } else {
        return None;
    };
    let tip_end = add(entry, scale(axis, depth));
    let start = add(entry, scale(axis, -depth.max(h.stack.diameter * 2.0)));
    a.kind = AnimationKind::DrillPlunge;
    a.actors.push(Actor {
        id: "drill".into(),
        role: "tool".into(),
        shape: ActorShape::Drill {
            diameter: h.stack.diameter,
            point_angle_deg: h.stack.drill_point_deg.unwrap_or(118.0),
            length: depth + 2.0 * h.stack.diameter,
        },
        keys: vec![
            key(0.0, start, axis, 0.0),
            key(0.15, start, axis, 1.0),
            key(0.7, tip_end, axis, 1.0),
            key(1.0, tip_end, axis, 1.0),
        ],
    });
    a.caption = match a.rule.as_str() {
        "cnc.hole_depth_ratio" => format!(
            "Ø{:.2} drill, {depth:.2} deep: L/D {m:.2} > {l:.2}",
            h.stack.diameter
        ),
        "cnc.blind_hole_floor" => format!(
            "blind floor {:.1}° — {m:.1}° from a standard drill point",
            h.stack.drill_point_deg.unwrap_or(180.0)
        ),
        "cnc.standard_drill_size" => format!(
            "Ø{:.3} is {m:.3} off the nearest series drill",
            h.stack.diameter
        ),
        _ => format!("Ø{m:.2} hole below the Ø{l:.2} minimum"),
    };
    Some(a)
}

/// Hole access / side cores: each setup (or pull) direction is shown in
/// turn; then the direction the hole actually needs.
fn setup_cycle(ctx: &Ctx<'_>, mut a: DfmAnimation, m: f64, _l: f64) -> Option<DfmAnimation> {
    let setups = setups_of(ctx.params);
    let (anchor, need) = if let Some(h) = ctx
        .census
        .holes
        .iter()
        .find(|h| h.faces.iter().any(|f| a.faces.contains(f)))
    {
        (h.stack.entry_point, neg(h.stack.axis))
    } else {
        let b = ctx
            .census
            .unrecognised
            .iter()
            .find(|b| b.faces.iter().any(|f| a.faces.contains(f)))?;
        (faces_centroid(ctx.model, &b.faces)?, b.axis)
    };
    let n = setups.len().max(1) as f64;
    let len = 20.0;
    for (i, d) in setups.iter().enumerate() {
        let t0 = 0.75 * i as f64 / n;
        let t1 = 0.75 * (i + 1) as f64 / n;
        let tail = add(anchor, scale(*d, len));
        a.actors.push(Actor {
            id: format!("setup_{i}"),
            role: "guide".into(),
            shape: ActorShape::Arrow { length: len },
            keys: vec![
                key(0.0, tail, neg(*d), 0.0),
                key(t0, tail, neg(*d), 0.0),
                key((t0 + t1) / 2.0, tail, neg(*d), 1.0),
                key(t1, tail, neg(*d), 0.0),
                key(1.0, tail, neg(*d), 0.0),
            ],
        });
    }
    let tail = add(anchor, scale(need, len));
    a.actors.push(Actor {
        id: "needed".into(),
        role: "needed".into(),
        shape: ActorShape::Arrow { length: len },
        keys: vec![
            key(0.0, tail, neg(need), 0.0),
            key(0.75, tail, neg(need), 0.0),
            key(0.85, tail, neg(need), 1.0),
            key(1.0, tail, neg(need), 1.0),
        ],
    });
    a.kind = AnimationKind::SetupCycle;
    a.duration_s = 2.0 + 1.5 * n;
    a.caption = format!(
        "no setup lines up with this hole: nearest is {m:.1}° off its axis — it needs another setup"
    );
    Some(a)
}

/// Draft: the two halves part along the pull; the walls without draft
/// flash as they would drag.
fn mold_separation(ctx: &Ctx<'_>, mut a: DfmAnimation, m: f64, l: f64) -> Option<DfmAnimation> {
    let pull = match ctx.params {
        PackParams::Casting(p) => unit(p.pull_direction)?,
        PackParams::InjectionMolding { pull_direction, .. } => unit(*pull_direction)?,
        _ => return None,
    };
    let (lo, hi, diag) = part_extent(ctx.model, &ctx.features, pull);
    let mid = 0.5 * (lo + hi);
    let gap = 0.6 * diag;
    let (mut upper, mut lower) = (Vec::new(), Vec::new());
    for n in &ctx.features.graph.nodes {
        let c = faces_centroid(ctx.model, &[n.face]).unwrap_or([0.0; 3]);
        if dot3(c, pull) >= mid {
            upper.push(n.face);
        } else {
            lower.push(n.face);
        }
    }
    let track = |faces: Vec<FaceRef>, dir: [f64; 3]| FaceTrack {
        faces,
        keys: vec![
            FaceKey {
                t: 0.0,
                offset: [0.0; 3],
                rotation_deg: 0.0,
                state: FaceState::Context,
            },
            FaceKey {
                t: 0.2,
                offset: [0.0; 3],
                rotation_deg: 0.0,
                state: FaceState::Context,
            },
            FaceKey {
                t: 0.8,
                offset: scale(dir, gap / 2.0),
                rotation_deg: 0.0,
                state: FaceState::Context,
            },
            FaceKey {
                t: 1.0,
                offset: scale(dir, gap / 2.0),
                rotation_deg: 0.0,
                state: FaceState::Context,
            },
        ],
    };
    // The failing walls travel WITH their half (same offset keys), turning
    // red as the halves start to part — they are the faces that drag.
    let fail_with = |half: &[FaceRef], dir: [f64; 3]| {
        let mut tr = track(
            a.faces
                .iter()
                .copied()
                .filter(|f| half.contains(f))
                .collect(),
            dir,
        );
        for k in tr.keys.iter_mut().filter(|k| k.t >= 0.2) {
            k.state = FaceState::Fail;
        }
        tr
    };
    let fail_upper = fail_with(&upper, pull);
    let fail_lower = fail_with(&lower, neg(pull));
    a.face_tracks = vec![track(upper, pull), track(lower, neg(pull))];
    a.face_tracks.extend(
        [fail_upper, fail_lower]
            .into_iter()
            .filter(|t| !t.faces.is_empty()),
    );
    a.kind = AnimationKind::MoldSeparation;
    a.caption = format!("halves part along the pull: {m:.2}° draft < {l:.2}° — these walls drag");
    Some(a)
}

/// FDM overhang: a build plane rises along the build direction; the
/// overhanging faces turn red as the plane reaches them.
fn layer_build(ctx: &Ctx<'_>, mut a: DfmAnimation, m: f64, l: f64) -> Option<DfmAnimation> {
    let PackParams::Fdm {
        build_direction, ..
    } = ctx.params
    else {
        return None;
    };
    let b = unit(*build_direction)?;
    let (lo, hi, diag) = part_extent(ctx.model, &ctx.features, b);
    let c = faces_centroid(ctx.model, &a.faces).unwrap_or([0.0; 3]);
    let base = sub(c, scale(b, dot3(c, b) - lo));
    let top = add(base, scale(b, hi - lo));
    let reach = if hi > lo {
        ((faces_centroid(ctx.model, &a.faces).map_or(lo, |p| dot3(p, b)) - lo) / (hi - lo))
            .clamp(0.0, 1.0)
    } else {
        0.5
    };
    a.actors.push(Actor {
        id: "build_plane".into(),
        role: "guide".into(),
        shape: ActorShape::Plane { size: diag },
        keys: vec![
            key(0.0, base, b, 0.6),
            key(0.9, top, b, 0.6),
            key(1.0, top, b, 0.0),
        ],
    });
    a.face_tracks = vec![FaceTrack {
        faces: a.faces.clone(),
        keys: vec![
            FaceKey {
                t: 0.0,
                offset: [0.0; 3],
                rotation_deg: 0.0,
                state: FaceState::Context,
            },
            FaceKey {
                t: 0.9 * reach,
                offset: [0.0; 3],
                rotation_deg: 0.0,
                state: FaceState::Fail,
            },
            FaceKey {
                t: 1.0,
                offset: [0.0; 3],
                rotation_deg: 0.0,
                state: FaceState::Fail,
            },
        ],
    }];
    a.kind = AnimationKind::LayerBuild;
    a.caption = format!("prints unsupported: {m:.1}° from vertical > {l:.1}°");
    Some(a)
}

/// Turning: the part spins on the spindle; faces that are not surfaces of
/// revolution flash (they need a milling op).
fn spindle_spin(ctx: &Ctx<'_>, mut a: DfmAnimation, m: f64) -> Option<DfmAnimation> {
    let axis = match ctx.params {
        PackParams::CncTurning(p) => match p.spindle_axis {
            Some((o, k)) => (o, unit(k)?),
            None => {
                let s = crate::dfm::packs::turning::infer_spindle_axis(&ctx.features).ok()?;
                (s.origin, s.direction)
            }
        },
        _ => return None,
    };
    let all: Vec<FaceRef> = ctx.features.graph.nodes.iter().map(|n| n.face).collect();
    a.face_tracks = vec![
        FaceTrack {
            faces: all,
            keys: vec![
                FaceKey {
                    t: 0.0,
                    offset: [0.0; 3],
                    rotation_deg: 0.0,
                    state: FaceState::Context,
                },
                FaceKey {
                    t: 1.0,
                    offset: [0.0; 3],
                    rotation_deg: 720.0,
                    state: FaceState::Context,
                },
            ],
        },
        // The off-axis faces spin WITH the part (same rotation keys),
        // flashing as they come round — a lathe tool cannot follow them.
        FaceTrack {
            faces: a.faces.clone(),
            keys: vec![
                FaceKey {
                    t: 0.0,
                    offset: [0.0; 3],
                    rotation_deg: 0.0,
                    state: FaceState::Fail,
                },
                FaceKey {
                    t: 1.0,
                    offset: [0.0; 3],
                    rotation_deg: 720.0,
                    state: FaceState::Fail,
                },
            ],
        },
    ];
    a.actors.push(Actor {
        id: "spindle".into(),
        role: "guide".into(),
        shape: ActorShape::Axis { length: 200.0 },
        keys: vec![key(0.0, axis.0, axis.1, 1.0), key(1.0, axis.0, axis.1, 1.0)],
    });
    a.spin_axis = Some(axis);
    a.kind = AnimationKind::SpindleSpin;
    a.caption = format!("{m:.0} face(s) are not turned on this spindle — a milling op is needed");
    Some(a)
}

/// Wall rules: caliper jaws close on a witness wall pair whose separation
/// is the measured value.
fn caliper(ctx: &Ctx<'_>, mut a: DfmAnimation, m: f64, l: f64) -> Option<DfmAnimation> {
    let planes: Vec<(FaceId, [f64; 3], [f64; 3])> = a
        .faces
        .iter()
        .filter_map(|&f| match ctx.features.geometry(f) {
            Some(FaceGeometry::Plane { origin, normal }) => Some((f, *origin, *normal)),
            _ => None,
        })
        .collect();
    let mut found = None;
    'outer: for (i, (fa, oa, na)) in planes.iter().enumerate() {
        for (fb, ob, nb) in planes.iter().skip(i + 1) {
            if dot3(*na, *nb) <= -1.0 + 1e-9 && (dot3(sub(*ob, *oa), *na).abs() - m).abs() <= 1e-9 {
                found = Some((*fa, *fb, *na));
                break 'outer;
            }
        }
    }
    let (fa, fb, na) = found?;
    let ca = faces_centroid(ctx.model, &[fa])?;
    let cb = faces_centroid(ctx.model, &[fb])?;
    let mid = scale(add(ca, cb), 0.5);
    let open = (4.0 * m).max(m + 10.0);
    a.actors.push(Actor {
        id: "caliper".into(),
        role: "tool".into(),
        shape: ActorShape::Caliper { opening: open },
        keys: vec![
            ActorKey {
                t: 0.0,
                position: mid,
                axis: na,
                opacity: 0.0,
                extent: open,
            },
            ActorKey {
                t: 0.15,
                position: mid,
                axis: na,
                opacity: 1.0,
                extent: open,
            },
            ActorKey {
                t: 0.7,
                position: mid,
                axis: na,
                opacity: 1.0,
                extent: m,
            },
            ActorKey {
                t: 1.0,
                position: mid,
                axis: na,
                opacity: 1.0,
                extent: m,
            },
        ],
    });
    a.kind = AnimationKind::Caliper;
    a.caption = if a.rule.ends_with("max_wall") {
        format!("wall {m:.2} thick > {l:.2} — a hot spot")
    } else {
        format!("wall {m:.2} thick < {l:.2} minimum")
    };
    Some(a)
}
