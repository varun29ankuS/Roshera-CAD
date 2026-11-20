//! A cone's angle as a drawing states it.
//!
//! The analytic table ([`crate::readable::extract_dimensions`]) reads every
//! cone's HALF-ANGLE to its axis: a geometric fact, and what an agent asking
//! the kernel gets. A drawing states an angle by the convention of the
//! feature it sizes, and from a reference a reader can see:
//!
//! | cone | stated as | why |
//! |---|---|---|
//! | weld bevel: an external cone ending a part at its plane end face (or at a knife edge), steeper than 45 deg to the axis | `90 deg - half-angle`, from the end face | ASME B16.25 / ISO 9692-1 specify the weld preparation from the plane square to the pipe axis (37.5 deg); the half-angle (52.5 deg) is a number nobody on the shop floor checks |
//! | every other cone: a countersink or drill point the hole recogniser did not claim, a taper, a hub | the included angle, `2 x half-angle` | ISO 3040 states a cone by its included angle; a countersink is called "90 deg", a drill point "118 deg" |
//!
//! A chamfer's cone is not here: it is stated by its chamfer callout
//! (`0.5 × 45°`) and its angle row is dropped
//! ([`super::dimensioning::convention_records`]). A countersink or drill
//! point inside a recognised hole stack is stated by the hole callout
//! (`⌵ Ø18 × 90°`, [`super::hole_features`]) and its angle row is dropped
//! there.
//!
//! The label is the bare number and its degree sign (ISO 129-1): the `∠` the
//! analytic table prints is the GD&T angularity symbol and has no place in
//! front of a dimension.
//!
//! This runs inside [`super::arcs::drafting_dimensions`], the table the sheet
//! is built from AND the table the sheet certificate re-measures against: the
//! certificate re-runs this classification on the live solid, so the number
//! a sheet prints is checked against the same quantity measured the same way.
//! Each stated angle carries its own persistent id (`included_angle`,
//! `angle_from_end_face`), never the half-angle's, so a number of one
//! meaning is never compared with a number of another: a bevel re-cut below
//! 45 deg to the axis is no longer a bevel, and the sheet's bevel angle
//! dangles rather than being "re-measured" as an included angle.

use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::adjacency::{
    attributed_adjacency_graph, rim_circle, AttributedAdjacencyGraph, EdgeConvexity, FaceGeometry,
};
use crate::readable::dimensions::feature_dim_pid;
use crate::readable::{DatumDescriptor, DimensionRecord};

/// [`DatumDescriptor::kind`] of a bevel angle's reference: the plane square
/// to the cone's axis through the bevel's end.
pub const END_FACE_REFERENCE: &str = "end_face";

/// Persistent-id kind of an angle stated from the end face.
const FROM_END_FACE_PID_KIND: &str = "angle_from_end_face";
/// Persistent-id kind of an included angle.
const INCLUDED_PID_KIND: &str = "included_angle";

/// A bevel is steeper than this to its axis (degrees): at 45 deg and below
/// the cone reads as a taper or a lead-in, stated by its included angle.
const BEVEL_MIN_HALF_ANGLE_DEG: f64 = 45.0;

/// Relative length tolerance for "the same station" and "no material
/// beyond the end plane".
const REL_TOL: f64 = 1e-6;

/// Restate every cone angle of `records` by its drawing convention (see the
/// module docs). Records of any other kind pass through unchanged.
pub(crate) fn apply_cone_angle_conventions(
    model: &BRepModel,
    solid_id: SolidId,
    records: Vec<DimensionRecord>,
) -> Vec<DimensionRecord> {
    if !records.iter().any(|d| d.kind == "angle") {
        return records;
    }
    let g = attributed_adjacency_graph(model, solid_id);
    records.into_iter().map(|d| restate(model, &g, d)).collect()
}

fn restate(
    model: &BRepModel,
    g: &AttributedAdjacencyGraph,
    mut d: DimensionRecord,
) -> DimensionRecord {
    if d.kind != "angle" {
        return d;
    }
    let [fid] = d.entities[..] else {
        return d;
    };
    let Some(FaceGeometry::Cone {
        apex,
        axis,
        half_angle,
    }) = g.node(fid).map(|n| n.geometry.clone())
    else {
        return d;
    };
    let half_deg = half_angle.to_degrees();
    match bevel_end(model, g, fid, apex, axis, half_angle) {
        Some(end) => {
            let value = 90.0 - half_deg;
            d.value = value;
            d.label = format!("{value:.1}\u{00B0}");
            d.pid = feature_dim_pid(model, &[fid], FROM_END_FACE_PID_KIND);
            d.axis = Some(axis);
            d.datum = Some(DatumDescriptor {
                kind: END_FACE_REFERENCE.to_string(),
                name: "end face".to_string(),
                origin: end,
            });
        }
        None => {
            let value = 2.0 * half_deg;
            d.value = value;
            d.label = format!("{value:.1}\u{00B0}");
            d.pid = feature_dim_pid(model, &[fid], INCLUDED_PID_KIND);
        }
    }
    d
}

/// When cone face `fid` is a weld bevel, the centre of its end plane (on the
/// axis): an EXTERNAL cone steeper than 45 deg to its axis whose small rim
/// is a convex edge onto a plane square to the axis facing out of the part
/// (the root face), or onto a bore (a knife edge), with no material of the
/// part beyond that plane -- the part's open end. `None` for every other
/// cone.
fn bevel_end(
    model: &BRepModel,
    g: &AttributedAdjacencyGraph,
    fid: u32,
    apex: [f64; 3],
    axis: [f64; 3],
    half_angle: f64,
) -> Option<[f64; 3]> {
    let node = g.node(fid)?;
    if node.cavity != Some(false) || half_angle.to_degrees() <= BEVEL_MIN_HALF_ANGLE_DEG {
        return None;
    }
    let [s0, s1] = node.station_range?;
    // Radius grows with |station|: the small rim is the station nearer the
    // apex, and "outward" along the axis is from the large rim to it.
    let (small, large) = if s0.abs() <= s1.abs() {
        (s0, s1)
    } else {
        (s1, s0)
    };
    let out = if small >= large { 1.0 } else { -1.0 };
    let tol = REL_TOL * large.abs().max(1.0) * 10.0;

    let meets_end = g.neighbours(fid).into_iter().any(|(arc, other)| {
        if arc.convexity != EdgeConvexity::Convex || other == fid {
            return false;
        }
        let Some(rim) = rim_circle(model, arc.edge, apex, axis) else {
            return false;
        };
        if (rim.station - small).abs() > tol {
            return false;
        }
        match g.node(other).map(|n| (&n.geometry, n.cavity)) {
            // The root face: square to the axis, facing out of the part.
            Some((FaceGeometry::Plane { normal, .. }, _)) => dot(*normal, axis) * out > 1.0 - 1e-9,
            // A knife edge onto the bore.
            Some((FaceGeometry::Cylinder { axis: k, .. }, Some(true))) => {
                dot(*k, axis).abs() > 1.0 - 1e-9
            }
            _ => false,
        }
    });
    if !meets_end {
        return None;
    }
    // The part's open end: nothing of it lies beyond the end plane.
    let end_t = small * out;
    let beyond = solid_points(model, g)
        .into_iter()
        .any(|p| dot(sub(p, apex), axis) * out > end_t + tol);
    if beyond {
        return None;
    }
    Some([
        apex[0] + axis[0] * small,
        apex[1] + axis[1] * small,
        apex[2] + axis[2] * small,
    ])
}

/// Every vertex of the solid and the midpoint of every edge (a curved edge
/// can bulge past its vertices).
fn solid_points(model: &BRepModel, g: &AttributedAdjacencyGraph) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for n in &g.nodes {
        let Some(face) = model.faces.get(n.face) else {
            continue;
        };
        let mut loops = vec![face.outer_loop];
        loops.extend(face.inner_loops.iter().copied());
        for lid in loops {
            let Some(l) = model.loops.get(lid) else {
                continue;
            };
            for &eid in &l.edges {
                if !seen.insert(eid) {
                    continue;
                }
                let Some(e) = model.edges.get(eid) else {
                    continue;
                };
                for vid in [e.start_vertex, e.end_vertex] {
                    if let Some(v) = model.vertices.get(vid) {
                        let p = v.point();
                        out.push([p.x, p.y, p.z]);
                    }
                }
                if let Ok(p) = e.evaluate(0.5, &model.curves) {
                    out.push([p.x, p.y, p.z]);
                }
            }
        }
    }
    out
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
