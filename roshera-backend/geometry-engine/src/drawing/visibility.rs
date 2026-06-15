//! Hidden-line removal (#22) — visibility classification via the raytrace eye.
//!
//! A mechanical drawing draws OCCLUDED edges as a distinct dashed line, not as
//! a solid edge and not omitted. Visibility here is decided by the SAME analytic
//! ray-cast the perception layer uses (`queries::raycast_solid`): a point on an
//! edge is HIDDEN when, looking from the camera along the view direction, the
//! solid's own surface is hit nearer than that point — i.e. another face is in
//! front of it. No tessellation, no z-buffer raster: every classification is an
//! exact ray↔analytic-surface test, so the drawing cannot claim a hidden edge
//! is visible (a sound-eye violation) or vice-versa.
//!
//! Edges are classified PER SEGMENT (at each sampled sub-span's midpoint), so a
//! partially-occluded edge splits at the crossover into a visible run and a
//! hidden run — the drafting convention.

use std::collections::HashSet;

use crate::math::{Point3, Vector3};
use crate::primitives::edge::EdgeId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::queries::raycast_solid;

use super::projection::{view_matrix_for_projection, ProjectionError};
use super::types::{Polyline2d, ProjectionType};

/// The edges of a view split by visibility. `visible` draws solid; `hidden`
/// draws dashed.
#[derive(Debug, Clone)]
pub struct ViewEdges {
    pub visible: Vec<Polyline2d>,
    pub hidden: Vec<Polyline2d>,
}

/// Into-scene view direction (unit) for a projection: the third row of the
/// world→view matrix, recovered as `(Tx.z, Ty.z, Tz.z)` where `T` transforms a
/// world vector to view space (`row_w · e_i = w_i`).
fn view_direction(projection: ProjectionType) -> Vector3 {
    let vm = view_matrix_for_projection(projection);
    let w = Vector3::new(
        vm.transform_vector(&Vector3::X).z,
        vm.transform_vector(&Vector3::Y).z,
        vm.transform_vector(&Vector3::Z).z,
    );
    w.normalize().unwrap_or(Vector3::Z)
}

/// World AABB diagonal of a solid, from its face-loop vertices. Used to place
/// ray origins safely outside the part and to scale the occlusion epsilon.
fn solid_diagonal(model: &BRepModel, solid_id: SolidId) -> f64 {
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return 1.0,
    };
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut any = false;
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    for sh in shells {
        let shell = match model.shells.get(sh) {
            Some(s) => s,
            None => continue,
        };
        for &fid in &shell.faces {
            let face = match model.faces.get(fid) {
                Some(f) => f,
                None => continue,
            };
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                let lp = match model.loops.get(lid) {
                    Some(l) => l,
                    None => continue,
                };
                for &eid in &lp.edges {
                    if let Some(e) = model.edges.get(eid) {
                        for vid in [e.start_vertex, e.end_vertex] {
                            if let Some(v) = model.vertices.get(vid) {
                                for i in 0..3 {
                                    if v.position[i] < min[i] {
                                        min[i] = v.position[i];
                                    }
                                    if v.position[i] > max[i] {
                                        max[i] = v.position[i];
                                    }
                                }
                                any = true;
                            }
                        }
                    }
                }
            }
        }
    }
    if !any {
        return 1.0;
    }
    let dx = max[0] - min[0];
    let dy = max[1] - min[1];
    let dz = max[2] - min[2];
    (dx * dx + dy * dy + dz * dz).sqrt().max(1.0)
}

/// Core occlusion test: is `m` hidden, viewed along `w`? Cast from `back` units
/// behind `m` (toward the camera) along `w`; `m` sits at ray parameter `back`,
/// so a nearer hit (`< back − eps`) means another face occludes it.
fn occluded(
    model: &BRepModel,
    solid_id: SolidId,
    m: Point3,
    w: Vector3,
    back: f64,
    eps: f64,
) -> bool {
    let origin = m - w * back;
    match raycast_solid(model, solid_id, origin, w) {
        Some(hit) => hit.distance < back - eps,
        None => false,
    }
}

/// Is world point `p` hidden behind the solid in this view? Public so callers /
/// tests can probe visibility of an arbitrary point directly (the crisp sound
/// property: a point on the far face is hidden, one on the near face is not).
pub fn is_point_hidden(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
    p: Point3,
) -> bool {
    let w = view_direction(projection);
    let diag = solid_diagonal(model, solid_id);
    let back = 2.0 * diag + 10.0;
    let eps = diag * 1e-5 + 1e-3;
    occluded(model, solid_id, p, w, back, eps)
}

/// Project a solid's edges, classifying every sub-segment visible / hidden.
pub fn project_solid_edges_visibility(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
    samples_per_curve: usize,
) -> Result<ViewEdges, ProjectionError> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or(ProjectionError::SolidNotFound(solid_id))?;
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or(ProjectionError::MissingShell(solid_id))?;

    let vm = view_matrix_for_projection(projection);
    let w = view_direction(projection);
    let diag = solid_diagonal(model, solid_id);
    let back = 2.0 * diag + 10.0;
    let eps = diag * 1e-5 + 1e-3;

    let mut visited: HashSet<EdgeId> = HashSet::new();
    let mut out = ViewEdges {
        visible: Vec::new(),
        hidden: Vec::new(),
    };

    // All shells (outer + inner), so a bore's own walls are classified too.
    let mut shell_ids = vec![solid.outer_shell];
    shell_ids.extend_from_slice(&solid.inner_shells);
    let _ = shell; // outer shell fetched above only to validate existence.

    for sh in shell_ids {
        let shell = match model.shells.get(sh) {
            Some(s) => s,
            None => continue,
        };
        for face_id in &shell.faces {
            let face = match model.faces.get(*face_id) {
                Some(f) => f,
                None => continue,
            };
            let loop_ids = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
            for loop_id in loop_ids {
                let topo_loop = match model.loops.get(loop_id) {
                    Some(l) => l,
                    None => continue,
                };
                for edge_id in &topo_loop.edges {
                    if !visited.insert(*edge_id) {
                        continue;
                    }
                    let edge = match model.edges.get(*edge_id) {
                        Some(e) => e,
                        None => continue,
                    };
                    let curve = match model.curves.get(edge.curve_id) {
                        Some(c) => c,
                        None => continue,
                    };
                    let is_linear = curve.is_linear(crate::math::Tolerance::default());
                    let n = if is_linear {
                        2
                    } else {
                        samples_per_curve.max(2)
                    };
                    let t0 = edge.param_range.start;
                    let t1 = edge.param_range.end;

                    // Sample 3D + 2D in lockstep.
                    let mut p3: Vec<Point3> = Vec::with_capacity(n);
                    let mut p2: Vec<[f64; 2]> = Vec::with_capacity(n);
                    let mut ok = true;
                    for i in 0..n {
                        let frac = i as f64 / (n - 1) as f64;
                        let t = t0 + (t1 - t0) * frac;
                        match curve.point_at(t) {
                            Ok(p) => {
                                let v = vm.transform_point(&p);
                                p3.push(p);
                                p2.push([v.x, v.y]);
                            }
                            Err(_) => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok || p2.len() < 2 {
                        continue;
                    }

                    // Classify each segment, grouping consecutive same-visibility
                    // runs into polylines.
                    let mut runs: Vec<(bool, Vec<[f64; 2]>)> = Vec::new();
                    for i in 0..p2.len() - 1 {
                        let mid = Point3::new(
                            0.5 * (p3[i].x + p3[i + 1].x),
                            0.5 * (p3[i].y + p3[i + 1].y),
                            0.5 * (p3[i].z + p3[i + 1].z),
                        );
                        let visible = !occluded(model, solid_id, mid, w, back, eps);
                        match runs.last_mut() {
                            Some((v, pts)) if *v == visible => pts.push(p2[i + 1]),
                            _ => runs.push((visible, vec![p2[i], p2[i + 1]])),
                        }
                    }

                    for (visible, pts) in runs {
                        let pl = Polyline2d::from_points(pts);
                        if pl.points.len() < 2 {
                            continue;
                        }
                        if visible {
                            out.visible.push(pl);
                        } else {
                            out.hidden.push(pl);
                        }
                    }
                }
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::projection::DEFAULT_CURVE_SAMPLES;
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    #[test]
    fn box_far_face_hidden_near_face_visible() {
        // Box 20³ centred at origin. Front view (camera +Y). The +Y face centre
        // (0,10,0) is the near face → visible; the −Y face centre (0,−10,0) sits
        // behind it → hidden.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        assert!(
            !is_point_hidden(&m, b, ProjectionType::Front, Point3::new(0.0, 10.0, 0.0)),
            "near (+Y) face is visible"
        );
        assert!(
            is_point_hidden(&m, b, ProjectionType::Front, Point3::new(0.0, -10.0, 0.0)),
            "far (−Y) face is hidden"
        );
    }

    #[test]
    fn box_front_view_has_visible_and_hidden_runs() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        let e = project_solid_edges_visibility(&m, b, ProjectionType::Front, DEFAULT_CURVE_SAMPLES)
            .expect("vis");
        // The front face's 4 edges are visible; the back face's 4 edges are
        // hidden (they project onto the same square but classify hidden).
        assert!(!e.visible.is_empty(), "some visible edges");
        assert!(!e.hidden.is_empty(), "some hidden edges (the back face)");
    }

    #[test]
    fn bored_plate_far_bore_wall_is_hidden_in_front() {
        // Plate 50×50×16 with a Ø20 through-bore on Z. In Front view the bore is
        // a vertical slot; its FAR wall (the +Y side of the cylinder, behind the
        // plate front) is hidden. Probe a point on the far bore wall.
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 50.0, 16.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore");
        // Far bore wall point: on the cylinder at +Y (y=+10), mid-thickness.
        assert!(
            is_point_hidden(&m, part, ProjectionType::Front, Point3::new(0.0, 10.0, 0.0)),
            "far bore wall is hidden behind the plate front"
        );
        // The near plate front face is visible.
        assert!(
            !is_point_hidden(
                &m,
                part,
                ProjectionType::Front,
                Point3::new(20.0, 25.0, 0.0)
            ),
            "plate front face is visible"
        );
    }

    #[test]
    fn visibility_split_is_deterministic() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(30.0, 20.0, 16.0)
            .expect("box"));
        let a = project_solid_edges_visibility(&m, b, ProjectionType::Isometric, 12).expect("a");
        let c = project_solid_edges_visibility(&m, b, ProjectionType::Isometric, 12).expect("c");
        assert_eq!(a.visible.len(), c.visible.len(), "visible count stable");
        assert_eq!(a.hidden.len(), c.hidden.len(), "hidden count stable");
    }
}
