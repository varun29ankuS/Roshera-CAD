//! The natural frame of a part: the axes a drafter squares it to.
//!
//! A STEP file holds a part wherever its author left it. An OEM part is
//! usually modelled in vehicle position, so a bracket can arrive turned a few
//! degrees about some axis. The sheet projects along the world axes, so a
//! turned part gets skewed views and -- worse -- its projected widths inked as
//! if they were sizes.
//!
//! [`analyse_natural_frame`] reads the part's own analytic geometry: every
//! planar face votes for its normal and every cylinder, cone and torus votes
//! for its axis, each weighted by the face's area. Directions are unsigned and
//! clustered. The heaviest cluster is the part's primary direction; the
//! heaviest cluster square to it is the secondary. The squaring rotation is the
//! SMALLEST rotation that lays both along world axes -- the part is squared to
//! the axes it is nearest, never re-posed, so a part modelled square is left
//! exactly as its author placed it and its front view stays the author's.
//! A part whose primary is already square keeps its in-plane angle (the
//! author placed it), and so does a part that is round about its primary.
//!
//! A part with no planar, cylindrical, conical or toroidal face (an all-NURBS
//! casting) has no natural frame here: the analysis says so with a typed
//! reason and the caller draws it as modelled. It never guesses an axis from a
//! mesh.

use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cone, Cylinder, Plane, Torus};
use crate::primitives::topology_builder::BRepModel;

/// Two unsigned directions closer than this (as `1 - |cos|`) are one
/// direction: about 0.08 degrees, far above the 1e-9 noise of exported
/// analytic surfaces and far below any designed angle.
const SAME_DIRECTION: f64 = 1.0e-6;

/// A secondary direction must be square to the primary within this `|cos|`
/// (about 0.06 degrees).
const SQUARE: f64 = 1.0e-3;

/// A part turned less than this from its nearest square pose is drawn as
/// modelled: 0.01 degrees moves a 1 m edge by 0.17 mm across its length, well
/// inside any drawing's line width at a scale that fits that edge on a sheet.
pub const SQUARE_ENOUGH_DEG: f64 = 0.01;

/// One unsigned direction the part's faces agree on.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectionVote {
    /// Unit direction, sign-canonical (its largest component positive).
    pub direction: Vector3,
    /// Area of the planar faces whose normal is this direction.
    pub planar_area: f64,
    /// Area of the cylinders, cones and tori whose axis is this direction.
    pub axial_area: f64,
}

impl DirectionVote {
    /// Total weight of the vote.
    pub fn weight(&self) -> f64 {
        self.planar_area + self.axial_area
    }
}

/// What [`analyse_natural_frame`] found.
#[derive(Debug, Clone)]
pub struct FrameAnalysis {
    /// The heaviest direction.
    pub primary: DirectionVote,
    /// The heaviest direction square to the primary, if the part has one
    /// that sets its in-plane angle. `None` for a part whose every direction
    /// is the primary's (a plain turned part), and for a part that is round
    /// about its primary (more turned area about it than the heaviest square
    /// direction carries). The squaring turns the part about its primary by
    /// this direction only when the primary itself sat off square.
    pub secondary: Option<DirectionVote>,
    /// The smallest rotation that lays the primary and secondary along world
    /// axes (about the world origin).
    pub squaring: Matrix4,
    /// The angle of that rotation, in degrees: how far the part is turned
    /// from square. 0 for a part modelled square.
    pub tilt_deg: f64,
    /// Every vote, heaviest first.
    pub votes: Vec<DirectionVote>,
}

/// Why a part has no natural frame.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NoNaturalFrame {
    /// The solid id does not resolve.
    #[error("solid {0} does not exist")]
    UnknownSolid(SolidId),
    /// No planar, cylindrical, conical or toroidal face with a measurable
    /// area: nothing to square to.
    #[error("the part has no planar, cylindrical, conical or toroidal face to square it by")]
    NoAnalyticFaces,
}

fn canonical(v: Vector3) -> Option<Vector3> {
    let u = v.normalize().ok()?;
    let big = [u.x, u.y, u.z]
        .into_iter()
        .max_by(|a, b| {
            a.abs()
                .partial_cmp(&b.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0.0);
    Some(if big < 0.0 { -u } else { u })
}

fn vote(votes: &mut Vec<DirectionVote>, dir: Vector3, planar: f64, axial: f64) {
    let Some(d) = canonical(dir) else {
        return;
    };
    for v in votes.iter_mut() {
        if 1.0 - v.direction.dot(&d).abs() < SAME_DIRECTION {
            v.planar_area += planar;
            v.axial_area += axial;
            return;
        }
    }
    votes.push(DirectionVote {
        direction: d,
        planar_area: planar,
        axial_area: axial,
    });
}

/// Points along a loop's edges, in loop order: each edge at
/// [`SAMPLES_PER_EDGE`] parameters, walked in the direction the loop uses it.
fn loop_samples(model: &BRepModel, loop_id: u32) -> Vec<Point3> {
    let Some(lp) = model.loops.get(loop_id) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(lp.edges.len() * SAMPLES_PER_EDGE);
    for (i, eid) in lp.edges.iter().enumerate() {
        let Some(edge) = model.edges.get(*eid) else {
            continue;
        };
        let forward = lp.orientations.get(i).copied().unwrap_or(true);
        for k in 0..SAMPLES_PER_EDGE {
            let t = k as f64 / SAMPLES_PER_EDGE as f64;
            let t = if forward { t } else { 1.0 - t };
            if let Ok(p) = edge.evaluate(t, &model.curves) {
                out.push(p);
            }
        }
    }
    out
}

/// Boundary samples per edge for the area estimate.
const SAMPLES_PER_EDGE: usize = 8;

/// What a face votes with.
enum Vote {
    /// A planar face: its normal.
    Plane(Vector3),
    /// A cylinder, cone or torus: its axis line.
    Axial { origin: Point3, axis: Vector3 },
}

/// The face's area, ESTIMATED from its boundary: a vote weight, not a
/// measurement (nothing on a sheet reads it). A planar face is the polygon
/// area of its sampled loops (outer less inner); a surface of revolution is
/// mean radius × swept angle × axial span of its sampled boundary. Exact
/// area integration (`Face::compute_stats`) took minutes on a 309-face part.
fn estimated_area(model: &BRepModel, face: u32, vote: &Vote) -> Option<f64> {
    let f = model.faces.get(face)?;
    let area = match vote {
        Vote::Plane(n) => {
            let polygon = |pts: &[Point3]| -> f64 {
                if pts.len() < 3 {
                    return 0.0;
                }
                let mut acc = Vector3::ZERO;
                for i in 0..pts.len() {
                    let a = pts[i].to_vec();
                    let b = pts[(i + 1) % pts.len()].to_vec();
                    acc += a.cross(&b);
                }
                0.5 * acc.dot(n).abs()
            };
            let outer = polygon(&loop_samples(model, f.outer_loop));
            let inner: f64 = f
                .inner_loops
                .iter()
                .map(|l| polygon(&loop_samples(model, *l)))
                .sum();
            outer - inner
        }
        Vote::Axial { origin, axis } => {
            let pts: Vec<Point3> = f
                .all_loops()
                .iter()
                .flat_map(|l| loop_samples(model, *l))
                .collect();
            if pts.is_empty() {
                return None;
            }
            let (u, w) = {
                let helper = if axis.x.abs() < 0.9 {
                    Vector3::X
                } else {
                    Vector3::new(0.0, 1.0, 0.0)
                };
                let u = axis.cross(&helper).normalize().ok()?;
                (u, axis.cross(&u))
            };
            let (mut lo, mut hi, mut r_sum) = (f64::INFINITY, f64::NEG_INFINITY, 0.0);
            let mut angles = Vec::with_capacity(pts.len());
            for p in &pts {
                let d = *p - *origin;
                let h = d.dot(axis);
                lo = lo.min(h);
                hi = hi.max(h);
                let radial = d - *axis * h;
                r_sum += radial.magnitude();
                angles.push(radial.dot(&w).atan2(radial.dot(&u)));
            }
            angles.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mut max_gap = std::f64::consts::TAU - (angles[angles.len() - 1] - angles[0]);
            for pair in angles.windows(2) {
                max_gap = max_gap.max(pair[1] - pair[0]);
            }
            let swept = std::f64::consts::TAU - max_gap;
            let r = r_sum / pts.len() as f64;
            // A full ring sampled at its seams can read a tiny sweep: never
            // less than the sampled spacing allows.
            r * swept.max(0.0) * (hi - lo).max(r * 1e-3)
        }
    };
    (area.is_finite() && area > 0.0).then_some(area)
}

/// The world axis nearest `d`, signed to agree with it, and its index.
fn nearest_axis(d: Vector3, exclude: Option<usize>) -> (usize, Vector3) {
    let axes = [Vector3::X, Vector3::new(0.0, 1.0, 0.0), Vector3::Z];
    let mut best = (0usize, -1.0f64);
    for (i, a) in axes.iter().enumerate() {
        if Some(i) == exclude {
            continue;
        }
        let c = a.dot(&d).abs();
        if c > best.1 {
            best = (i, c);
        }
    }
    let a = axes[best.0];
    (best.0, if a.dot(&d) < 0.0 { -a } else { a })
}

/// Read the natural frame of `solid` from its analytic faces.
pub fn analyse_natural_frame(
    model: &BRepModel,
    solid: SolidId,
) -> Result<FrameAnalysis, NoNaturalFrame> {
    let face_ids: Vec<u32> = {
        let s = model
            .solids
            .get(solid)
            .ok_or(NoNaturalFrame::UnknownSolid(solid))?;
        s.all_shells()
            .iter()
            .filter_map(|sh| model.shells.get(*sh))
            .flat_map(|sh| sh.faces.iter().copied())
            .collect()
    };
    let mut votes: Vec<DirectionVote> = Vec::new();
    for fid in face_ids {
        // (direction, is planar) from the exact surface, read before the
        // area so the surface borrow ends first.
        let face_vote = {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let Some(surf) = model.surfaces.get(face.surface_id) else {
                continue;
            };
            let any = surf.as_any();
            if let Some(p) = any.downcast_ref::<Plane>() {
                Some(Vote::Plane(p.normal))
            } else if let Some(c) = any.downcast_ref::<Cylinder>() {
                Some(Vote::Axial {
                    origin: c.origin,
                    axis: c.axis,
                })
            } else if let Some(c) = any.downcast_ref::<Cone>() {
                Some(Vote::Axial {
                    origin: c.apex,
                    axis: c.axis,
                })
            } else {
                any.downcast_ref::<Torus>().map(|t| Vote::Axial {
                    origin: t.center,
                    axis: t.axis,
                })
            }
        };
        let Some(face_vote) = face_vote else {
            continue;
        };
        let Some(area) = estimated_area(model, fid, &face_vote) else {
            continue;
        };
        let (dir, planar) = match face_vote {
            Vote::Plane(n) => (n, true),
            Vote::Axial { axis, .. } => (axis, false),
        };
        if planar {
            vote(&mut votes, dir, area, 0.0);
        } else {
            vote(&mut votes, dir, 0.0, area);
        }
    }
    votes.sort_by(|a, b| {
        b.weight()
            .partial_cmp(&a.weight())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let primary = votes
        .first()
        .cloned()
        .ok_or(NoNaturalFrame::NoAnalyticFaces)?;
    // A part that is ROUND about its primary (more turned area about that
    // axis than the heaviest square direction carries) has no in-plane angle
    // of its own: a flange's arm or flat at 20 degrees is a designed angle,
    // and the author's frame keeps it. Only the primary is squared then.
    let secondary = votes
        .iter()
        .skip(1)
        .find(|v| v.direction.dot(&primary.direction).abs() < SQUARE)
        .filter(|v| primary.axial_area <= v.weight())
        .cloned();

    let p = primary.direction;
    let (pi, pa) = nearest_axis(p, None);
    // A part whose primary already lies on a world axis was placed square by
    // its author: its in-plane angle (an arm at 10 degrees, a flange's flat)
    // is theirs, and the sheet keeps it. Only a part tilted OUT of that plane
    // -- the way a part modelled in vehicle position arrives -- is squared
    // about its primary as well.
    let turn_by = secondary
        .as_ref()
        .filter(|_| p.dot(&pa).abs() < (SQUARE_ENOUGH_DEG.to_radians()).cos());
    // The second direction of the source triad: the secondary vote made
    // exactly square to the primary, or -- for a part symmetric about its
    // primary -- the world axis most square to it, so the squaring adds no
    // turn about the primary.
    let s = match turn_by {
        Some(v) => v.direction - p * v.direction.dot(&p),
        None => {
            let (_, b) = nearest_axis(p.cross(&pa).normalize_or_zero(), Some(pi));
            let b = if b.dot(&pa).abs() > 0.5 {
                nearest_axis(b, Some(pi)).1
            } else {
                b
            };
            b - p * b.dot(&p)
        }
    };
    let s = s.normalize().map_err(|_| NoNaturalFrame::NoAnalyticFaces)?;
    let (_, sa) = nearest_axis(s, Some(pi));
    let t = p.cross(&s);
    let ta = pa.cross(&sa);
    // R maps p -> pa, s -> sa, t -> ta: R = [pa sa ta] [p s t]^T.
    let target = [pa, sa, ta];
    let source = [p, s, t];
    let mut rows = [[0.0f64; 4]; 4];
    for (r, row) in rows.iter_mut().take(3).enumerate() {
        for (c, cell) in row.iter_mut().take(3).enumerate() {
            *cell = (0..3)
                .map(|k| component(target[k], r) * component(source[k], c))
                .sum();
        }
    }
    rows[3][3] = 1.0;
    let squaring = Matrix4::from_rows_array(rows);
    let trace = rows[0][0] + rows[1][1] + rows[2][2];
    let tilt_deg = ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees();
    Ok(FrameAnalysis {
        primary,
        secondary,
        squaring,
        tilt_deg,
        votes,
    })
}

fn component(v: Vector3, i: usize) -> f64 {
    match i {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}
