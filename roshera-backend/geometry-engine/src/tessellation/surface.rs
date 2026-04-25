//! Surface tessellation algorithms

use super::adaptive::compute_plane_axes;
use super::{AdaptiveTessellator, MeshVertex, TessellationParams, ThreeJsMesh, TriangleMesh};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::face::Face;
use crate::primitives::surface::Surface;
use crate::primitives::topology_builder::BRepModel;
use std::collections::{HashMap, HashSet};
use tracing;

/// Tessellate a face into triangles
pub fn tessellate_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    // Get surface
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    match surface.type_name() {
        "Plane" => tessellate_planar_face(face, model, mesh),
        "Cylinder" => tessellate_cylindrical_face(face, model, params, mesh),
        "Sphere" => tessellate_spherical_face(face, model, params, mesh),
        "Cone" => tessellate_conical_face(face, model, params, mesh),
        "Torus" => tessellate_toroidal_face(face, model, params, mesh),
        "NURBS" => tessellate_nurbs_face(face, model, params, mesh),
        _ => tessellate_generic_face(face, model, params, mesh),
    }
}

/// Convert ThreeJsMesh to TriangleMesh
fn threejs_mesh_to_triangle_mesh(threejs_mesh: &ThreeJsMesh) -> TriangleMesh {
    let mut triangle_mesh = TriangleMesh::new();

    // Convert vertices
    for i in 0..threejs_mesh.vertex_count() {
        let idx = i * 3;
        let position = Point3::new(
            threejs_mesh.positions[idx] as f64,
            threejs_mesh.positions[idx + 1] as f64,
            threejs_mesh.positions[idx + 2] as f64,
        );
        let normal = Vector3::new(
            threejs_mesh.normals[idx] as f64,
            threejs_mesh.normals[idx + 1] as f64,
            threejs_mesh.normals[idx + 2] as f64,
        );

        let uv = if let Some(ref uvs) = threejs_mesh.uvs {
            let uv_idx = i * 2;
            if uv_idx + 1 < uvs.len() {
                Some((uvs[uv_idx] as f64, uvs[uv_idx + 1] as f64))
            } else {
                None
            }
        } else {
            None
        };

        triangle_mesh.add_vertex(MeshVertex {
            position,
            normal,
            uv,
        });
    }

    // Convert triangles
    for i in 0..threejs_mesh.triangle_count() {
        let idx = i * 3;
        triangle_mesh.add_triangle(
            threejs_mesh.indices[idx],
            threejs_mesh.indices[idx + 1],
            threejs_mesh.indices[idx + 2],
        );
    }

    triangle_mesh
}

/// Internal function that uses ThreeJsMesh
fn tessellate_face_threejs(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    // Get surface
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    match surface.type_name() {
        "Plane" => tessellate_planar_face(face, model, mesh),
        "Cylinder" => tessellate_cylindrical_face(face, model, params, mesh),
        "Sphere" => tessellate_spherical_face(face, model, params, mesh),
        "Cone" => tessellate_conical_face(face, model, params, mesh),
        "Torus" => tessellate_toroidal_face(face, model, params, mesh),
        "NURBS" => tessellate_nurbs_face(face, model, params, mesh),
        _ => tessellate_generic_face(face, model, params, mesh),
    }
}

/// Tessellate a planar face using constrained Delaunay triangulation
fn tessellate_planar_face(face: &Face, model: &BRepModel, mesh: &mut TriangleMesh) {
    // Get surface and compute normal
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    let (u_range, v_range) = surface.parameter_bounds();
    let u_mid = (u_range.0 + u_range.1) / 2.0;
    let v_mid = (v_range.0 + v_range.1) / 2.0;

    let normal = face
        .normal_at(u_mid, v_mid, &model.surfaces)
        .unwrap_or(Vector3::Z);

    // Collect all vertices from outer loop and holes
    let mut all_vertices = Vec::new();
    let mut loop_boundaries = Vec::new();

    // Process outer loop
    if let Some(outer_loop) = model.loops.get(face.outer_loop) {
        let start_idx = all_vertices.len();
        if let Ok(vertices) = outer_loop.vertices(&model.edges) {
            for &vertex_id in &vertices {
                if let Some(vertex) = model.vertices.get(vertex_id) {
                    all_vertices.push(Point3::from(vertex.position));
                }
            }
        }
        let end_idx = all_vertices.len();
        if end_idx > start_idx {
            loop_boundaries.push((start_idx, end_idx, true)); // true = outer loop
        }
    }

    // Process inner loops (holes)
    for &inner_loop_id in &face.inner_loops {
        if let Some(inner_loop) = model.loops.get(inner_loop_id) {
            let start_idx = all_vertices.len();
            if let Ok(vertices) = inner_loop.vertices(&model.edges) {
                for &vertex_id in &vertices {
                    if let Some(vertex) = model.vertices.get(vertex_id) {
                        all_vertices.push(Point3::from(vertex.position));
                    }
                }
            }
            let end_idx = all_vertices.len();
            if end_idx > start_idx {
                loop_boundaries.push((start_idx, end_idx, false)); // false = inner loop (hole)
            }
        }
    }

    if all_vertices.len() < 3 {
        return;
    }

    // Perform constrained Delaunay triangulation
    let triangles = constrained_delaunay_triangulation(&all_vertices, &loop_boundaries, &normal);

    // Add vertices to mesh and build index mapping
    let mut vertex_map = Vec::new();
    for vertex in &all_vertices {
        let index = mesh.add_vertex(MeshVertex {
            position: *vertex,
            normal,
            uv: None,
        });
        vertex_map.push(index);
    }

    // Add triangles to mesh with proper orientation
    for triangle in triangles {
        if face.orientation == crate::primitives::face::FaceOrientation::Forward {
            mesh.add_triangle(
                vertex_map[triangle[0]],
                vertex_map[triangle[1]],
                vertex_map[triangle[2]],
            );
        } else {
            mesh.add_triangle(
                vertex_map[triangle[0]],
                vertex_map[triangle[2]],
                vertex_map[triangle[1]],
            );
        }
    }
}

/// Perform constrained Delaunay triangulation for a face with holes
fn constrained_delaunay_triangulation(
    vertices: &[Point3],
    loop_boundaries: &[(usize, usize, bool)],
    normal: &Vector3,
) -> Vec<[usize; 3]> {
    if vertices.len() < 3 {
        return Vec::new();
    }

    // Project vertices to 2D plane
    let (u_axis, v_axis) = compute_plane_axes(normal);
    let origin = vertices[0];

    let vertices_2d: Vec<(f64, f64)> = vertices
        .iter()
        .map(|p| {
            let relative = *p - origin;
            (relative.dot(&u_axis), relative.dot(&v_axis))
        })
        .collect();

    // Build constraint edges
    let mut constraint_edges = Vec::new();
    for &(start, end, _is_outer) in loop_boundaries {
        for i in start..end {
            let j = if i + 1 < end { i + 1 } else { start };
            constraint_edges.push((i, j));
        }
    }

    // Perform Delaunay triangulation
    let mut triangles = bowyer_watson_constrained(&vertices_2d, &constraint_edges);

    // Remove triangles outside outer boundary and inside holes
    triangles.retain(|triangle| {
        let centroid = calculate_triangle_centroid_2d(
            &vertices_2d[triangle[0]],
            &vertices_2d[triangle[1]],
            &vertices_2d[triangle[2]],
        );

        // Check if centroid is inside outer loop
        let mut inside_outer = false;
        for &(start, end, is_outer) in loop_boundaries {
            if is_outer {
                let loop_polygon: Vec<(f64, f64)> = (start..end).map(|i| vertices_2d[i]).collect();
                if is_point_inside_polygon_2d(&centroid, &loop_polygon) {
                    inside_outer = true;
                    break;
                }
            }
        }

        if !inside_outer {
            return false;
        }

        // Check if centroid is inside any hole
        for &(start, end, is_outer) in loop_boundaries {
            if !is_outer {
                let hole_polygon: Vec<(f64, f64)> = (start..end).map(|i| vertices_2d[i]).collect();
                if is_point_inside_polygon_2d(&centroid, &hole_polygon) {
                    return false;
                }
            }
        }

        true
    });

    triangles
}

/// Bowyer-Watson algorithm with constraints
fn bowyer_watson_constrained(
    vertices: &[(f64, f64)],
    constraints: &[(usize, usize)],
) -> Vec<[usize; 3]> {
    // Start with standard Bowyer-Watson
    let mut triangles = bowyer_watson_2d_indexed(vertices);

    // Enforce constraints
    for &(v1, v2) in constraints {
        enforce_edge_constraint(&mut triangles, vertices, v1, v2);
    }

    triangles
}

/// Bowyer-Watson algorithm that returns triangles with original vertex indices
fn bowyer_watson_2d_indexed(points: &[(f64, f64)]) -> Vec<[usize; 3]> {
    if points.len() < 3 {
        return Vec::new();
    }

    // Create super-triangle
    let (min_x, min_y, max_x, max_y) = compute_bounds_2d(points);
    let dx = max_x - min_x;
    let dy = max_y - min_y;
    let delta_max = dx.max(dy);
    let mid_x = (min_x + max_x) / 2.0;
    let mid_y = (min_y + max_y) / 2.0;

    let super_vertices = vec![
        (mid_x - 2.0 * delta_max, mid_y - delta_max),
        (mid_x, mid_y + 2.0 * delta_max),
        (mid_x + 2.0 * delta_max, mid_y - delta_max),
    ];

    let n = points.len();
    let mut all_vertices = super_vertices;
    all_vertices.extend_from_slice(points);

    let mut triangles = vec![[0, 1, 2]];

    // Add points one by one
    for i in 0..n {
        let vertex_idx = i + 3; // Account for super-triangle vertices
        let point = points[i];

        let mut bad_triangles = Vec::new();

        // Find triangles whose circumcircle contains the point
        for (tri_idx, &triangle) in triangles.iter().enumerate() {
            if in_circumcircle_indexed(&all_vertices, triangle, &point) {
                bad_triangles.push(tri_idx);
            }
        }

        // Find boundary edges
        let mut boundary_edges = Vec::new();
        for &tri_idx in &bad_triangles {
            let triangle = triangles[tri_idx];
            for j in 0..3 {
                let edge = [triangle[j], triangle[(j + 1) % 3]];

                let mut is_shared = false;
                for &other_idx in &bad_triangles {
                    if other_idx != tri_idx {
                        let other_tri = triangles[other_idx];
                        if triangle_has_edge(&other_tri, edge[0], edge[1]) {
                            is_shared = true;
                            break;
                        }
                    }
                }

                if !is_shared {
                    boundary_edges.push(edge);
                }
            }
        }

        // Remove bad triangles
        bad_triangles.sort_unstable_by(|a, b| b.cmp(a));
        for idx in bad_triangles {
            triangles.swap_remove(idx);
        }

        // Re-triangulate
        for edge in boundary_edges {
            triangles.push([edge[0], edge[1], vertex_idx]);
        }
    }

    // Remove triangles containing super-triangle vertices and adjust indices
    triangles.retain(|tri| tri[0] >= 3 && tri[1] >= 3 && tri[2] >= 3);

    for triangle in &mut triangles {
        triangle[0] -= 3;
        triangle[1] -= 3;
        triangle[2] -= 3;
    }

    triangles
}

/// Check if a point is inside the circumcircle of a triangle
fn in_circumcircle_indexed(
    vertices: &[(f64, f64)],
    triangle: [usize; 3],
    point: &(f64, f64),
) -> bool {
    let p1 = &vertices[triangle[0]];
    let p2 = &vertices[triangle[1]];
    let p3 = &vertices[triangle[2]];

    let ax = p1.0 - point.0;
    let ay = p1.1 - point.1;
    let bx = p2.0 - point.0;
    let by = p2.1 - point.1;
    let cx = p3.0 - point.0;
    let cy = p3.1 - point.1;

    let det = (ax * ax + ay * ay) * (bx * cy - cx * by) - (bx * bx + by * by) * (ax * cy - cx * ay)
        + (cx * cx + cy * cy) * (ax * by - bx * ay);

    det > 0.0
}

/// Check if triangle has a specific edge
fn triangle_has_edge(triangle: &[usize; 3], v1: usize, v2: usize) -> bool {
    for i in 0..3 {
        let j = (i + 1) % 3;
        if (triangle[i] == v1 && triangle[j] == v2) || (triangle[i] == v2 && triangle[j] == v1) {
            return true;
        }
    }
    false
}

/// Enforce an edge constraint in the triangulation.
///
/// Removes all triangles whose edges cross the constraint edge (v1, v2),
/// then retriangulates the two resulting cavities (one on each side of the
/// constraint edge) using ear-clipping. This produces a valid constrained
/// Delaunay triangulation.
fn enforce_edge_constraint(
    triangles: &mut Vec<[usize; 3]>,
    vertices: &[(f64, f64)],
    v1: usize,
    v2: usize,
) {
    // Check if edge already exists in the triangulation
    for triangle in triangles.iter() {
        if triangle_has_edge(triangle, v1, v2) {
            return;
        }
    }

    // Find triangles whose edges intersect the constraint edge
    let mut intersecting = Vec::new();
    for (idx, triangle) in triangles.iter().enumerate() {
        if edge_intersects_triangle(vertices, v1, v2, *triangle) {
            intersecting.push(idx);
        }
    }

    if intersecting.is_empty() {
        return;
    }

    // Collect boundary edges of the cavity (edges not shared between removed triangles)
    let _removed_set: HashSet<usize> = intersecting.iter().cloned().collect();
    let mut edge_count: HashMap<(usize, usize), usize> = HashMap::new();
    let mut cavity_vertices: HashSet<usize> = HashSet::new();

    for &idx in &intersecting {
        let tri = triangles[idx];
        for k in 0..3 {
            let a = tri[k];
            let b = tri[(k + 1) % 3];
            cavity_vertices.insert(a);
            cavity_vertices.insert(b);
            let edge = if a < b { (a, b) } else { (b, a) };
            *edge_count.entry(edge).or_insert(0) += 1;
        }
    }
    // Ensure constraint endpoints are included
    cavity_vertices.insert(v1);
    cavity_vertices.insert(v2);

    // Boundary edges are those appearing exactly once among removed triangles
    let _boundary_edges: Vec<(usize, usize)> = edge_count
        .iter()
        .filter(|(_, &count)| count == 1)
        .map(|(&edge, _)| edge)
        .collect();

    // Remove intersecting triangles (reverse order to keep indices valid with swap_remove)
    intersecting.sort_unstable_by(|a, b| b.cmp(a));
    for idx in &intersecting {
        triangles.swap_remove(*idx);
    }

    // Separate cavity vertices into two polygons: above and below the constraint edge
    let edge_dir = (
        vertices[v2].0 - vertices[v1].0,
        vertices[v2].1 - vertices[v1].1,
    );

    let mut above: Vec<usize> = Vec::new();
    let mut below: Vec<usize> = Vec::new();

    for &vi in &cavity_vertices {
        if vi == v1 || vi == v2 {
            continue;
        }
        let rel = (
            vertices[vi].0 - vertices[v1].0,
            vertices[vi].1 - vertices[v1].1,
        );
        let cross = edge_dir.0 * rel.1 - edge_dir.1 * rel.0;
        if cross > 0.0 {
            above.push(vi);
        } else {
            below.push(vi);
        }
    }

    // Triangulate each side: fan from constraint edge to cavity boundary vertices.
    // Sort vertices by angle from v1→v2 to produce a correct polygon ordering.
    let retriangulate_side = |side: &mut Vec<usize>, tris: &mut Vec<[usize; 3]>| {
        if side.is_empty() {
            return;
        }

        // Sort by angle relative to v1, measured from v1→v2 direction
        let base_angle = edge_dir.1.atan2(edge_dir.0);
        side.sort_by(|&a, &b| {
            let da = (
                vertices[a].0 - vertices[v1].0,
                vertices[a].1 - vertices[v1].1,
            );
            let db = (
                vertices[b].0 - vertices[v1].0,
                vertices[b].1 - vertices[v1].1,
            );
            let angle_a = da.1.atan2(da.0) - base_angle;
            let angle_b = db.1.atan2(db.0) - base_angle;
            // Normalize to [0, 2π)
            let na = if angle_a < 0.0 {
                angle_a + 2.0 * std::f64::consts::PI
            } else {
                angle_a
            };
            let nb = if angle_b < 0.0 {
                angle_b + 2.0 * std::f64::consts::PI
            } else {
                angle_b
            };
            na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Build polygon: v1, sorted vertices..., v2
        let mut polygon = Vec::with_capacity(side.len() + 2);
        polygon.push(v1);
        polygon.extend_from_slice(side);
        polygon.push(v2);

        // Ear-clipping triangulation of the polygon
        ear_clip_2d(vertices, &polygon, tris);
    };

    retriangulate_side(&mut above, triangles);
    retriangulate_side(&mut below, triangles);
}

/// Triangulate a simple polygon in 2D using ear clipping.
/// Appends resulting triangles to `output`.
fn ear_clip_2d(vertices: &[(f64, f64)], polygon: &[usize], output: &mut Vec<[usize; 3]>) {
    if polygon.len() < 3 {
        return;
    }
    if polygon.len() == 3 {
        output.push([polygon[0], polygon[1], polygon[2]]);
        return;
    }

    let mut remaining: Vec<usize> = polygon.to_vec();

    let mut max_iterations = remaining.len() * remaining.len();
    let mut i = 0;

    while remaining.len() > 3 && max_iterations > 0 {
        max_iterations -= 1;
        let n = remaining.len();
        let prev = remaining[(i + n - 1) % n];
        let curr = remaining[i % n];
        let next = remaining[(i + 1) % n];

        let p0 = vertices[prev];
        let p1 = vertices[curr];
        let p2 = vertices[next];

        // Check if this is a convex (ear) vertex
        let cross = (p1.0 - p0.0) * (p2.1 - p0.1) - (p1.1 - p0.1) * (p2.0 - p0.0);
        if cross <= 1e-14 {
            // Not convex, skip
            i = (i + 1) % remaining.len();
            continue;
        }

        // Check that no other vertex lies inside this ear triangle
        let mut is_ear = true;
        for &vi in &remaining {
            if vi == prev || vi == curr || vi == next {
                continue;
            }
            if point_in_triangle_2d(&vertices[vi], &p0, &p1, &p2) {
                is_ear = false;
                break;
            }
        }

        if is_ear {
            output.push([prev, curr, next]);
            remaining.remove(i % remaining.len());
            if i >= remaining.len() && !remaining.is_empty() {
                i = 0;
            }
        } else {
            i = (i + 1) % remaining.len();
        }
    }

    // Emit final triangle
    if remaining.len() == 3 {
        output.push([remaining[0], remaining[1], remaining[2]]);
    }
}

/// Check if point p is inside triangle (a, b, c) using barycentric coordinates
fn point_in_triangle_2d(p: &(f64, f64), a: &(f64, f64), b: &(f64, f64), c: &(f64, f64)) -> bool {
    let v0 = (c.0 - a.0, c.1 - a.1);
    let v1 = (b.0 - a.0, b.1 - a.1);
    let v2 = (p.0 - a.0, p.1 - a.1);

    let dot00 = v0.0 * v0.0 + v0.1 * v0.1;
    let dot01 = v0.0 * v1.0 + v0.1 * v1.1;
    let dot02 = v0.0 * v2.0 + v0.1 * v2.1;
    let dot11 = v1.0 * v1.0 + v1.1 * v1.1;
    let dot12 = v1.0 * v2.0 + v1.1 * v2.1;

    let inv_denom = 1.0 / (dot00 * dot11 - dot01 * dot01);
    let u = (dot11 * dot02 - dot01 * dot12) * inv_denom;
    let v = (dot00 * dot12 - dot01 * dot02) * inv_denom;

    // Point is inside if u >= 0, v >= 0, u + v <= 1 (with tolerance)
    u > 1e-10 && v > 1e-10 && (u + v) < 1.0 - 1e-10
}

/// Check if an edge intersects a triangle
fn edge_intersects_triangle(
    vertices: &[(f64, f64)],
    e1: usize,
    e2: usize,
    triangle: [usize; 3],
) -> bool {
    let p1 = vertices[e1];
    let p2 = vertices[e2];

    for i in 0..3 {
        let j = (i + 1) % 3;
        let p3 = vertices[triangle[i]];
        let p4 = vertices[triangle[j]];

        if segments_intersect_2d(&p1, &p2, &p3, &p4) {
            return true;
        }
    }

    false
}

/// Check if two line segments intersect
fn segments_intersect_2d(
    p1: &(f64, f64),
    p2: &(f64, f64),
    p3: &(f64, f64),
    p4: &(f64, f64),
) -> bool {
    let d1 = orientation_2d(p3, p4, p1);
    let d2 = orientation_2d(p3, p4, p2);
    let d3 = orientation_2d(p1, p2, p3);
    let d4 = orientation_2d(p1, p2, p4);

    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }

    // Check for collinear points
    if d1.abs() < 1e-10 && on_segment_2d(p3, p1, p4) {
        return true;
    }
    if d2.abs() < 1e-10 && on_segment_2d(p3, p2, p4) {
        return true;
    }
    if d3.abs() < 1e-10 && on_segment_2d(p1, p3, p2) {
        return true;
    }
    if d4.abs() < 1e-10 && on_segment_2d(p1, p4, p2) {
        return true;
    }

    false
}

/// Compute orientation of ordered triplet (p, q, r)
fn orientation_2d(p: &(f64, f64), q: &(f64, f64), r: &(f64, f64)) -> f64 {
    (q.1 - p.1) * (r.0 - q.0) - (q.0 - p.0) * (r.1 - q.1)
}

/// Check if point q lies on segment pr
fn on_segment_2d(p: &(f64, f64), q: &(f64, f64), r: &(f64, f64)) -> bool {
    q.0 <= p.0.max(r.0) && q.0 >= p.0.min(r.0) && q.1 <= p.1.max(r.1) && q.1 >= p.1.min(r.1)
}

/// Calculate centroid of a 2D triangle
fn calculate_triangle_centroid_2d(p1: &(f64, f64), p2: &(f64, f64), p3: &(f64, f64)) -> (f64, f64) {
    ((p1.0 + p2.0 + p3.0) / 3.0, (p1.1 + p2.1 + p3.1) / 3.0)
}

/// Check if a point is inside a 2D polygon using winding number
fn is_point_inside_polygon_2d(point: &(f64, f64), polygon: &[(f64, f64)]) -> bool {
    let winding = calculate_winding_number(point, polygon);
    winding.abs() > 0.5
}

/// Compute bounding box of 2D points
fn compute_bounds_2d(points: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;

    for &(x, y) in points {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }

    (min_x, min_y, max_x, max_y)
}

/// Tessellate a cylindrical face
fn tessellate_cylindrical_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    // Tessellation is void-return; if the face's surface has gone missing
    // from the model (invariant violation), we skip silently rather than
    // panicking the entire tessellation pass.
    let Some(surface) = model.surfaces.get(face.surface_id) else {
        return;
    };

    // Get parameter bounds from face loops
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // Extract actual cylinder radius from surface
    let radius = surface
        .as_any()
        .downcast_ref::<crate::primitives::surface::Cylinder>()
        .map(|c| c.radius)
        .unwrap_or(1.0);
    let u_span = u_max - u_min;
    let v_span = v_max - v_min;

    let u_steps = ((radius * u_span) / params.max_edge_length).ceil() as usize + 1;
    let v_steps = (v_span / params.max_edge_length).ceil() as usize + 1;

    // Generate vertices
    let mut vertex_grid = Vec::new();
    for v_idx in 0..=v_steps {
        let v = v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64);
        let mut row = Vec::new();

        for u_idx in 0..=u_steps {
            let u = u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64);

            if let (Ok(point), Ok(normal)) = (
                surface.point_at(u, v),
                face.normal_at(u, v, &model.surfaces),
            ) {
                let index = mesh.add_vertex(MeshVertex {
                    position: point,
                    normal,
                    uv: Some((u, v)),
                });
                row.push(index);
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            if vertex_grid[v_idx].len() > u_idx + 1 && vertex_grid[v_idx + 1].len() > u_idx + 1 {
                let v0 = vertex_grid[v_idx][u_idx];
                let v1 = vertex_grid[v_idx][u_idx + 1];
                let v2 = vertex_grid[v_idx + 1][u_idx];
                let v3 = vertex_grid[v_idx + 1][u_idx + 1];

                mesh.add_triangle(v0, v1, v2);
                mesh.add_triangle(v1, v3, v2);
            }
        }
    }
}

/// Tessellate a spherical face with adaptive refinement
fn tessellate_spherical_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Get parameter bounds from face loops
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // Sphere-specific: detect if we're near poles
    let near_north_pole = v_max > std::f64::consts::PI * 0.9;
    let near_south_pole = v_min < std::f64::consts::PI * 0.1;

    // Adaptive tessellation based on angular span
    let u_span = u_max - u_min;
    let v_span = v_max - v_min;

    // Calculate resolution based on angular deviation
    let _radius = estimate_sphere_radius(surface);
    let u_steps = ((u_span / params.max_angle_deviation) as usize).max(3);
    let v_steps = ((v_span / params.max_angle_deviation) as usize).max(3);

    // Special handling for poles to avoid degeneracies
    if near_north_pole || near_south_pole {
        tessellate_spherical_with_poles(
            face,
            model,
            surface,
            u_min,
            u_max,
            v_min,
            v_max,
            u_steps,
            v_steps,
            near_north_pole,
            near_south_pole,
            mesh,
        );
    } else {
        // Regular grid tessellation for non-polar regions
        tessellate_spherical_regular(
            face, model, surface, u_min, u_max, v_min, v_max, u_steps, v_steps, mesh,
        );
    }
}

/// Tessellate spherical surface with pole handling
fn tessellate_spherical_with_poles(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    u_steps: usize,
    v_steps: usize,
    near_north_pole: bool,
    near_south_pole: bool,
    mesh: &mut TriangleMesh,
) {
    let mut vertex_grid = Vec::new();

    // Generate vertices with special pole handling
    for v_idx in 0..=v_steps {
        let v = v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64);
        let mut row = Vec::new();

        // Check if we're at a pole
        let at_pole = (near_north_pole && v_idx == v_steps) || (near_south_pole && v_idx == 0);

        if at_pole {
            // Single vertex at pole
            let u = (u_min + u_max) / 2.0; // Any u value works at pole
            if let (Ok(point), Ok(normal)) = (
                surface.point_at(u, v),
                face.normal_at(u, v, &model.surfaces),
            ) {
                if is_point_inside_face(u, v, face, model) {
                    let index = mesh.add_vertex(MeshVertex {
                        position: point,
                        normal,
                        uv: Some((u, v)),
                    });
                    row.push(Some(index));
                }
            }
        } else {
            // Regular row of vertices
            for u_idx in 0..=u_steps {
                let u = u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64);

                if is_point_inside_face(u, v, face, model) {
                    if let (Ok(point), Ok(normal)) = (
                        surface.point_at(u, v),
                        face.normal_at(u, v, &model.surfaces),
                    ) {
                        let index = mesh.add_vertex(MeshVertex {
                            position: point,
                            normal,
                            uv: Some((u, v)),
                        });
                        row.push(Some(index));
                    } else {
                        row.push(None);
                    }
                } else {
                    row.push(None);
                }
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles with special handling for poles
    for v_idx in 0..v_steps {
        let at_south_pole = near_south_pole && v_idx == 0;
        let at_north_pole = near_north_pole && v_idx == v_steps - 1;

        if at_south_pole && vertex_grid[0].len() == 1 && vertex_grid[0][0].is_some() {
            // Triangles from south pole
            let pole_vertex = vertex_grid[0][0]
                .expect("south pole vertex presence verified by is_some() guard above");
            for u_idx in 0..u_steps {
                if let (Some(v1), Some(v2)) = (
                    vertex_grid[1].get(u_idx).and_then(|&v| v),
                    vertex_grid[1].get(u_idx + 1).and_then(|&v| v),
                ) {
                    mesh.add_triangle(pole_vertex, v1, v2);
                }
            }
        } else if at_north_pole
            && vertex_grid[v_steps].len() == 1
            && vertex_grid[v_steps][0].is_some()
        {
            // Triangles to north pole
            let pole_vertex = vertex_grid[v_steps][0]
                .expect("north pole vertex presence verified by is_some() guard above");
            for u_idx in 0..u_steps {
                if let (Some(v1), Some(v2)) = (
                    vertex_grid[v_idx].get(u_idx).and_then(|&v| v),
                    vertex_grid[v_idx].get(u_idx + 1).and_then(|&v| v),
                ) {
                    mesh.add_triangle(v1, v2, pole_vertex);
                }
            }
        } else {
            // Regular quad tessellation
            for u_idx in 0..u_steps {
                let v0 = vertex_grid[v_idx].get(u_idx).and_then(|&v| v);
                let v1 = vertex_grid[v_idx].get(u_idx + 1).and_then(|&v| v);
                let v2 = vertex_grid[v_idx + 1].get(u_idx).and_then(|&v| v);
                let v3 = vertex_grid[v_idx + 1].get(u_idx + 1).and_then(|&v| v);

                match (v0, v1, v2, v3) {
                    (Some(a), Some(b), Some(c), Some(d)) => {
                        mesh.add_triangle(a, b, c);
                        mesh.add_triangle(b, d, c);
                    }
                    // Handle degenerate cases
                    (Some(a), Some(b), Some(c), None) => mesh.add_triangle(a, b, c),
                    (Some(a), Some(b), None, Some(d)) => mesh.add_triangle(a, b, d),
                    (Some(a), None, Some(c), Some(d)) => mesh.add_triangle(a, d, c),
                    (None, Some(b), Some(c), Some(d)) => mesh.add_triangle(b, d, c),
                    _ => {}
                }
            }
        }
    }
}

/// Regular spherical tessellation for non-polar regions
fn tessellate_spherical_regular(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    _u_steps: usize,
    _v_steps: usize,
    mesh: &mut TriangleMesh,
) {
    // Use adaptive tessellation for better quality
    let tessellator = AdaptiveTessellator::new(TessellationParams::default());
    let temp_mesh = tessellator.tessellate_patch(surface, (u_min, u_max), (v_min, v_max));

    // Convert to ThreeJS mesh with face normal
    let _normal = face
        .normal_at(
            (u_min + u_max) / 2.0,
            (v_min + v_max) / 2.0,
            &model.surfaces,
        )
        .unwrap_or(Vector3::Z);

    let mut vertex_map = Vec::new();
    for vertex in &temp_mesh.vertices {
        // Check if vertex is inside face boundaries
        if let Some((u, v)) = vertex.uv {
            if is_point_inside_face(u, v, face, model) {
                let index = mesh.add_vertex(MeshVertex {
                    position: vertex.position,
                    normal: vertex.normal,
                    uv: Some((u, v)),
                });
                vertex_map.push(Some(index));
            } else {
                vertex_map.push(None);
            }
        } else {
            vertex_map.push(None);
        }
    }

    // Add triangles with mapping
    for triangle in &temp_mesh.triangles {
        if let (Some(v0), Some(v1), Some(v2)) = (
            vertex_map.get(triangle[0] as usize).and_then(|&v| v),
            vertex_map.get(triangle[1] as usize).and_then(|&v| v),
            vertex_map.get(triangle[2] as usize).and_then(|&v| v),
        ) {
            mesh.add_triangle(v0, v1, v2);
        }
    }
}

/// Estimate sphere radius from surface
fn estimate_sphere_radius(surface: &dyn Surface) -> f64 {
    // Sample center point and estimate radius
    let (u_range, v_range) = surface.parameter_bounds();
    let u_mid = (u_range.0 + u_range.1) / 2.0;
    let v_mid = (v_range.0 + v_range.1) / 2.0;

    if let Ok(center_point) = surface.point_at(u_mid, v_mid) {
        // Sample another point to estimate radius
        if let Ok(edge_point) = surface.point_at(u_mid + 0.1, v_mid) {
            center_point.distance(&edge_point) / 0.1 // Approximate radius
        } else {
            1.0 // Default radius
        }
    } else {
        1.0
    }
}

/// Tessellate a conical face with special handling for apex
fn tessellate_conical_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Get parameter bounds from face loops
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // Detect if we include the apex (v = 0 for typical cone parameterization)
    let includes_apex = v_min.abs() < 1e-6;

    // Calculate tessellation resolution
    let u_span = u_max - u_min;
    let _v_span = v_max - v_min;

    // Angular resolution for u (around axis)
    let u_steps = ((u_span / params.max_angle_deviation) as usize).max(8);

    // Linear resolution for v (along slant)
    let cone_height = estimate_cone_height(surface, v_min, v_max);
    let v_steps = ((cone_height / params.max_edge_length) as usize).max(3);

    if includes_apex {
        tessellate_conical_with_apex(
            face, model, surface, u_min, u_max, v_min, v_max, u_steps, v_steps, mesh,
        );
    } else {
        tessellate_conical_regular(
            face, model, surface, u_min, u_max, v_min, v_max, u_steps, v_steps, mesh,
        );
    }
}

/// Tessellate cone with apex handling
fn tessellate_conical_with_apex(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    u_steps: usize,
    v_steps: usize,
    mesh: &mut TriangleMesh,
) {
    let mut vertex_grid = Vec::new();

    // First row is the apex
    if v_min.abs() < 1e-6 {
        let u = (u_min + u_max) / 2.0; // Any u value at apex
        let v = v_min;

        if let (Ok(point), Ok(normal)) = (
            surface.point_at(u, v),
            face.normal_at(u, v, &model.surfaces),
        ) {
            let index = mesh.add_vertex(MeshVertex {
                position: point,
                normal,
                uv: Some((u, v)),
            });
            vertex_grid.push(vec![Some(index)]); // Single vertex at apex
        }
    }

    // Generate remaining rows
    let v_start = if v_min.abs() < 1e-6 { 1 } else { 0 };
    for v_idx in v_start..=v_steps {
        let v = v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64);
        let mut row = Vec::new();

        for u_idx in 0..=u_steps {
            let u = u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64);

            if is_point_inside_face(u, v, face, model) {
                if let (Ok(point), Ok(normal)) = (
                    surface.point_at(u, v),
                    face.normal_at(u, v, &model.surfaces),
                ) {
                    let index = mesh.add_vertex(MeshVertex {
                        position: point,
                        normal,
                        uv: Some((u, v)),
                    });
                    row.push(Some(index));
                } else {
                    row.push(None);
                }
            } else {
                row.push(None);
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles
    for v_idx in 0..vertex_grid.len() - 1 {
        if v_idx == 0 && vertex_grid[0].len() == 1 {
            // Triangles from apex
            if let Some(apex) = vertex_grid[0][0] {
                for u_idx in 0..u_steps {
                    if let (Some(v1), Some(v2)) = (
                        vertex_grid[1].get(u_idx).and_then(|&v| v),
                        vertex_grid[1].get(u_idx + 1).and_then(|&v| v),
                    ) {
                        mesh.add_triangle(apex, v1, v2);
                    }
                }
            }
        } else {
            // Regular quads
            for u_idx in 0..u_steps {
                if let (Some(v0), Some(v1), Some(v2), Some(v3)) = (
                    vertex_grid[v_idx].get(u_idx).and_then(|&v| v),
                    vertex_grid[v_idx].get(u_idx + 1).and_then(|&v| v),
                    vertex_grid[v_idx + 1].get(u_idx).and_then(|&v| v),
                    vertex_grid[v_idx + 1].get(u_idx + 1).and_then(|&v| v),
                ) {
                    mesh.add_triangle(v0, v1, v2);
                    mesh.add_triangle(v1, v3, v2);
                }
            }
        }
    }
}

/// Regular conical tessellation (truncated cone)
fn tessellate_conical_regular(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    u_steps: usize,
    v_steps: usize,
    mesh: &mut TriangleMesh,
) {
    // Standard grid tessellation
    tessellate_surface_grid(
        face, model, surface, u_min, u_max, v_min, v_max, u_steps, v_steps, mesh,
    );
}

/// Estimate cone height from v parameter range
fn estimate_cone_height(surface: &dyn Surface, v_min: f64, v_max: f64) -> f64 {
    if let (Ok(p1), Ok(p2)) = (surface.point_at(0.0, v_min), surface.point_at(0.0, v_max)) {
        p1.distance(&p2)
    } else {
        v_max - v_min
    }
}

/// Tessellate a toroidal face with proper handling of both parameters
fn tessellate_toroidal_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Get parameter bounds from face loops
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // Torus has two circular parameters
    let u_span = u_max - u_min;
    let v_span = v_max - v_min;

    // Get torus radii
    let (major_radius, minor_radius) = estimate_torus_radii(surface);

    // Calculate steps based on angular deviation
    // u goes around major circle, v goes around minor circle
    let u_steps = ((u_span / params.max_angle_deviation) as usize).max(8);
    let v_steps = ((v_span / params.max_angle_deviation) as usize).max(6);

    // Also consider chord tolerance
    let u_steps_chord = ((major_radius * u_span) / params.max_edge_length) as usize;
    let v_steps_chord = ((minor_radius * v_span) / params.max_edge_length) as usize;

    let _u_steps = u_steps.max(u_steps_chord).min(params.max_segments);
    let _v_steps = v_steps.max(v_steps_chord).min(params.max_segments / 2);

    // Use adaptive tessellation for high quality
    let tessellator = AdaptiveTessellator::new(params.clone());
    let temp_mesh = tessellator.tessellate_patch(surface, (u_min, u_max), (v_min, v_max));

    // Convert to ThreeJS mesh with trimming
    let mut vertex_map = Vec::new();
    for vertex in &temp_mesh.vertices {
        if let Some((u, v)) = vertex.uv {
            if is_point_inside_face(u, v, face, model) {
                let index = mesh.add_vertex(MeshVertex {
                    position: vertex.position,
                    normal: vertex.normal,
                    uv: Some((u, v)),
                });
                vertex_map.push(Some(index));
            } else {
                vertex_map.push(None);
            }
        } else {
            vertex_map.push(None);
        }
    }

    // Add triangles
    for triangle in &temp_mesh.triangles {
        if let (Some(v0), Some(v1), Some(v2)) = (
            vertex_map.get(triangle[0] as usize).and_then(|&v| v),
            vertex_map.get(triangle[1] as usize).and_then(|&v| v),
            vertex_map.get(triangle[2] as usize).and_then(|&v| v),
        ) {
            if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                mesh.add_triangle(v0, v1, v2);
            } else {
                mesh.add_triangle(v0, v2, v1);
            }
        }
    }
}

/// Estimate torus radii from surface
fn estimate_torus_radii(surface: &dyn Surface) -> (f64, f64) {
    // Sample points to estimate major and minor radii
    let (u_range, v_range) = surface.parameter_bounds();

    // Points on major circle (v = 0 and v = π)
    if let (Ok(p1), Ok(p2)) = (
        surface.point_at(u_range.0, v_range.0),
        surface.point_at(u_range.0, (v_range.0 + v_range.1) / 2.0),
    ) {
        let minor_radius = p1.distance(&p2) / 2.0;

        // Points around major circle
        if let (Ok(p3), Ok(p4)) = (
            surface.point_at(u_range.0, v_range.0),
            surface.point_at((u_range.0 + u_range.1) / 2.0, v_range.0),
        ) {
            let major_radius = p3.distance(&p4) / std::f64::consts::PI;
            (major_radius, minor_radius)
        } else {
            (1.0, minor_radius)
        }
    } else {
        (1.0, 0.25) // Default radii
    }
}

/// Generic grid tessellation helper
fn tessellate_surface_grid(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    u_steps: usize,
    v_steps: usize,
    mesh: &mut TriangleMesh,
) {
    let mut vertex_grid = Vec::new();

    // Generate vertices
    for v_idx in 0..=v_steps {
        let v = v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64);
        let mut row = Vec::new();

        for u_idx in 0..=u_steps {
            let u = u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64);

            if is_point_inside_face(u, v, face, model) {
                if let (Ok(point), Ok(normal)) = (
                    surface.point_at(u, v),
                    face.normal_at(u, v, &model.surfaces),
                ) {
                    let index = mesh.add_vertex(MeshVertex {
                        position: point,
                        normal,
                        uv: Some((u, v)),
                    });
                    row.push(Some(index));
                } else {
                    row.push(None);
                }
            } else {
                row.push(None);
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            if let (Some(v0), Some(v1), Some(v2), Some(v3)) = (
                vertex_grid[v_idx].get(u_idx).and_then(|&v| v),
                vertex_grid[v_idx].get(u_idx + 1).and_then(|&v| v),
                vertex_grid[v_idx + 1].get(u_idx).and_then(|&v| v),
                vertex_grid[v_idx + 1].get(u_idx + 1).and_then(|&v| v),
            ) {
                if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                    mesh.add_triangle(v0, v1, v2);
                    mesh.add_triangle(v1, v3, v2);
                } else {
                    mesh.add_triangle(v0, v2, v1);
                    mesh.add_triangle(v1, v2, v3);
                }
            }
        }
    }
}

/// Tessellate a NURBS face with world-class adaptive refinement
fn tessellate_nurbs_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Get parameter bounds for the face
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // For NURBS surfaces, we need adaptive tessellation based on curvature
    // Use a more sophisticated approach than generic grid
    tessellate_nurbs_adaptive(
        surface, face, model, params, mesh, u_min, u_max, v_min, v_max,
    );
}

/// Generic surface tessellation using uniform grid
fn tessellate_generic_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Get parameter bounds
    let (_u_range, _v_range) = surface.parameter_bounds();
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // Simple uniform grid tessellation
    let u_steps = ((u_max - u_min) / params.max_edge_length * 10.0).ceil() as usize + 1;
    let v_steps = ((v_max - v_min) / params.max_edge_length * 10.0).ceil() as usize + 1;

    let u_steps = u_steps.min(50).max(3);
    let v_steps = v_steps.min(50).max(3);

    // Generate vertices
    let mut vertex_grid = Vec::new();
    for v_idx in 0..=v_steps {
        let v = v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64);
        let mut row = Vec::new();

        for u_idx in 0..=u_steps {
            let u = u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64);

            // Check if point is inside face boundaries
            if is_point_inside_face(u, v, face, model) {
                if let (Ok(point), Ok(normal)) = (
                    surface.point_at(u, v),
                    face.normal_at(u, v, &model.surfaces),
                ) {
                    let index = mesh.add_vertex(MeshVertex {
                        position: point,
                        normal,
                        uv: None,
                    });
                    row.push(Some(index));
                } else {
                    row.push(None);
                }
            } else {
                row.push(None);
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            if let (Some(v0), Some(v1), Some(v2), Some(v3)) = (
                vertex_grid[v_idx].get(u_idx).and_then(|&v| v),
                vertex_grid[v_idx].get(u_idx + 1).and_then(|&v| v),
                vertex_grid[v_idx + 1].get(u_idx).and_then(|&v| v),
                vertex_grid[v_idx + 1].get(u_idx + 1).and_then(|&v| v),
            ) {
                mesh.add_triangle(v0, v1, v2);
                mesh.add_triangle(v1, v3, v2);
            }
        }
    }
}

/// Get parameter bounds for a face from its loops
fn get_face_parameter_bounds(face: &Face, model: &BRepModel) -> (f64, f64, f64, f64) {
    let mut u_min = f64::MAX;
    let mut u_max = f64::MIN;
    let mut v_min = f64::MAX;
    let mut v_max = f64::MIN;

    // Get surface for parameter evaluation. The original `None` arm
    // re-queried the same missing surface and unwrapped it, which would
    // have panicked. Since the surface is genuinely missing, return a
    // neutral zero-extent bound rather than panicking mid-tessellation.
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return (0.0, 0.0, 0.0, 0.0),
    };

    // Process outer loop
    if let Some(outer_loop) = model.loops.get(face.outer_loop) {
        update_bounds_from_loop(
            outer_loop, model, surface, &mut u_min, &mut u_max, &mut v_min, &mut v_max,
        );
    }

    // Process inner loops (holes)
    for &inner_loop_id in &face.inner_loops {
        if let Some(inner_loop) = model.loops.get(inner_loop_id) {
            update_bounds_from_loop(
                inner_loop, model, surface, &mut u_min, &mut u_max, &mut v_min, &mut v_max,
            );
        }
    }

    // Ensure valid bounds
    if u_min > u_max || v_min > v_max {
        // Fallback to surface bounds
        let (u_range, v_range) = surface.parameter_bounds();
        (u_range.0, u_range.1, v_range.0, v_range.1)
    } else {
        // Add small margin for numerical stability
        let u_margin = (u_max - u_min) * 0.01;
        let v_margin = (v_max - v_min) * 0.01;
        (
            u_min - u_margin,
            u_max + u_margin,
            v_min - v_margin,
            v_max + v_margin,
        )
    }
}

/// Update parameter bounds from a loop
fn update_bounds_from_loop(
    loop_data: &crate::primitives::r#loop::Loop,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: &mut f64,
    u_max: &mut f64,
    v_min: &mut f64,
    v_max: &mut f64,
) {
    // Sample edges in the loop
    for &edge_id in &loop_data.edges {
        if let Some(edge) = model.edges.get(edge_id) {
            if let Some(curve) = model.curves.get(edge.curve_id) {
                // Sample points along the edge
                let num_samples = 10;
                for i in 0..=num_samples {
                    let t = edge.param_range.start
                        + (i as f64) * (edge.param_range.end - edge.param_range.start)
                            / (num_samples as f64);
                    if let Ok(point_3d) = curve.point_at(t) {
                        // Project to surface parameter space
                        if let Ok((u, v)) = surface.closest_point(&point_3d, Tolerance::default()) {
                            *u_min = u_min.min(u);
                            *u_max = u_max.max(u);
                            *v_min = v_min.min(v);
                            *v_max = v_max.max(v);
                        }
                    }
                }
            }
        }
    }
}

/// Check if a parameter point is inside face boundaries using winding number algorithm
fn is_point_inside_face(u: f64, v: f64, face: &Face, model: &BRepModel) -> bool {
    // First check outer loop - point must be inside
    if !is_point_inside_loop(u, v, face.outer_loop, face, model) {
        return false;
    }

    // Then check inner loops (holes) - point must be outside all holes
    for &inner_loop_id in &face.inner_loops {
        if is_point_inside_loop(u, v, inner_loop_id, face, model) {
            return false;
        }
    }

    true
}

/// Check if a point is inside a loop using winding number algorithm
fn is_point_inside_loop(
    u: f64,
    v: f64,
    loop_id: crate::primitives::r#loop::LoopId,
    face: &Face,
    model: &BRepModel,
) -> bool {
    let loop_data = match model.loops.get(loop_id) {
        Some(l) => l,
        None => return false,
    };

    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return false,
    };

    // Get loop as 2D polygon in parameter space
    let polygon = get_loop_polygon_2d(loop_data, model, surface);
    if polygon.len() < 3 {
        return false;
    }

    // Use winding number algorithm
    let winding_number = calculate_winding_number(&(u, v), &polygon);

    // Point is inside if winding number is non-zero
    winding_number.abs() > 0.5
}

/// Get loop as 2D polygon in parameter space
fn get_loop_polygon_2d(
    loop_data: &crate::primitives::r#loop::Loop,
    model: &BRepModel,
    surface: &dyn Surface,
) -> Vec<(f64, f64)> {
    let mut polygon = Vec::new();

    for &edge_id in &loop_data.edges {
        if let Some(edge) = model.edges.get(edge_id) {
            if let Some(curve) = model.curves.get(edge.curve_id) {
                // Sample edge to get points
                let num_samples = 20; // Higher sampling for better accuracy
                for i in 0..num_samples {
                    let t = edge.param_range.start
                        + (i as f64) * (edge.param_range.end - edge.param_range.start)
                            / (num_samples as f64);
                    if let Ok(point_3d) = curve.point_at(t) {
                        // Project to surface parameter space
                        if let Ok((u, v)) = surface.closest_point(&point_3d, Tolerance::default()) {
                            polygon.push((u, v));
                        }
                    }
                }
            }
        }
    }

    polygon
}

/// Calculate winding number for point-in-polygon test
fn calculate_winding_number(point: &(f64, f64), polygon: &[(f64, f64)]) -> f64 {
    let mut winding_number = 0.0;
    let n = polygon.len();

    for i in 0..n {
        let p1 = polygon[i];
        let p2 = polygon[(i + 1) % n];

        // Calculate angle subtended by edge at the point
        let v1 = (p1.0 - point.0, p1.1 - point.1);
        let v2 = (p2.0 - point.0, p2.1 - point.1);

        // Use atan2 for robust angle calculation
        let angle1 = v1.1.atan2(v1.0);
        let angle2 = v2.1.atan2(v2.0);

        let mut delta = angle2 - angle1;

        // Normalize to [-π, π]
        while delta > std::f64::consts::PI {
            delta -= 2.0 * std::f64::consts::PI;
        }
        while delta < -std::f64::consts::PI {
            delta += 2.0 * std::f64::consts::PI;
        }

        winding_number += delta;
    }

    // Normalize to winding number
    winding_number / (2.0 * std::f64::consts::PI)
}

/// Tessellate a surface patch with adaptive refinement
pub fn tessellate_surface(
    surface: &dyn Surface,
    u_range: (f64, f64),
    v_range: (f64, f64),
    _params: &TessellationParams,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    // Simple uniform tessellation for now
    let u_steps = 10;
    let v_steps = 10;

    // Generate vertices
    for v_idx in 0..=v_steps {
        let v = v_range.0 + (v_idx as f64) * (v_range.1 - v_range.0) / (v_steps as f64);

        for u_idx in 0..=u_steps {
            let u = u_range.0 + (u_idx as f64) * (u_range.1 - u_range.0) / (u_steps as f64);

            if let Ok(eval) = surface.evaluate_full(u, v) {
                mesh.add_vertex(MeshVertex {
                    position: eval.position,
                    normal: eval.normal,
                    uv: Some((u, v)),
                });
            }
        }
    }

    // Generate triangles
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            let v0 = (v_idx * (u_steps + 1) + u_idx) as u32;
            let v1 = v0 + 1;
            let v2 = v0 + (u_steps + 1) as u32;
            let v3 = v2 + 1;

            mesh.add_triangle(v0, v1, v2);
            mesh.add_triangle(v1, v3, v2);
        }
    }

    mesh
}

/// Adaptive NURBS tessellation with curvature-based refinement
fn tessellate_nurbs_adaptive(
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
) {
    // Initial quadtree subdivision based on surface complexity
    let mut quad_tree = QuadTree::new(u_min, u_max, v_min, v_max);

    // Perform adaptive subdivision
    subdivide_nurbs_quad(
        &mut quad_tree,
        surface,
        face,
        model,
        params,
        u_min,
        u_max,
        v_min,
        v_max,
        0,
    );

    // Convert quadtree to triangles
    let vertices = quad_tree_to_mesh(&quad_tree, surface, face, model, mesh);

    // Post-process for watertight mesh
    ensure_watertight_mesh(mesh, &vertices);
}

/// Quadtree structure for adaptive subdivision
struct QuadTree {
    nodes: Vec<QuadNode>,
}

struct QuadNode {
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    children: Option<[usize; 4]>,
}

impl QuadTree {
    fn new(u_min: f64, u_max: f64, v_min: f64, v_max: f64) -> Self {
        let root = QuadNode {
            u_min,
            u_max,
            v_min,
            v_max,
            children: None,
        };
        Self { nodes: vec![root] }
    }

    fn subdivide(&mut self, node_idx: usize) -> [usize; 4] {
        // Copy the node data to avoid borrowing issues
        let (u_min, u_max, v_min, v_max) = {
            let node = &self.nodes[node_idx];
            (node.u_min, node.u_max, node.v_min, node.v_max)
        };

        let u_mid = (u_min + u_max) / 2.0;
        let v_mid = (v_min + v_max) / 2.0;

        // Create 4 child nodes
        let children = [
            self.add_node(u_min, u_mid, v_min, v_mid), // SW
            self.add_node(u_mid, u_max, v_min, v_mid), // SE
            self.add_node(u_mid, u_max, v_mid, v_max), // NE
            self.add_node(u_min, u_mid, v_mid, v_max), // NW
        ];

        self.nodes[node_idx].children = Some(children);
        children
    }

    fn add_node(&mut self, u_min: f64, u_max: f64, v_min: f64, v_max: f64) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(QuadNode {
            u_min,
            u_max,
            v_min,
            v_max,
            children: None,
        });
        idx
    }
}

/// Recursive subdivision based on curvature
fn subdivide_nurbs_quad(
    quad_tree: &mut QuadTree,
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    depth: usize,
) {
    const MAX_DEPTH: usize = 12;

    // Check if we should subdivide based on curvature
    if depth >= MAX_DEPTH
        || !should_subdivide_nurbs(surface, face, model, params, u_min, u_max, v_min, v_max)
    {
        return;
    }

    // Get current node
    let node_idx = quad_tree.nodes.len() - 1;

    // Subdivide into 4 children
    let _children = quad_tree.subdivide(node_idx);

    // Recursively subdivide children
    let u_mid = (u_min + u_max) / 2.0;
    let v_mid = (v_min + v_max) / 2.0;

    subdivide_nurbs_quad(
        quad_tree,
        surface,
        face,
        model,
        params,
        u_min,
        u_mid,
        v_min,
        v_mid,
        depth + 1,
    );
    subdivide_nurbs_quad(
        quad_tree,
        surface,
        face,
        model,
        params,
        u_mid,
        u_max,
        v_min,
        v_mid,
        depth + 1,
    );
    subdivide_nurbs_quad(
        quad_tree,
        surface,
        face,
        model,
        params,
        u_mid,
        u_max,
        v_mid,
        v_max,
        depth + 1,
    );
    subdivide_nurbs_quad(
        quad_tree,
        surface,
        face,
        model,
        params,
        u_min,
        u_mid,
        v_mid,
        v_max,
        depth + 1,
    );
}

/// Check if a quad should be subdivided based on curvature
fn should_subdivide_nurbs(
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
) -> bool {
    // Sample curvature at multiple points
    let sample_points = [
        (u_min, v_min),
        (u_max, v_min),
        (u_max, v_max),
        (u_min, v_max),
        ((u_min + u_max) / 2.0, (v_min + v_max) / 2.0),
    ];

    let mut max_curvature = 0.0f64;
    let mut max_normal_deviation = 0.0f64;
    let mut normals = Vec::new();

    for &(u, v) in &sample_points {
        if !is_point_inside_face(u, v, face, model) {
            continue;
        }

        if let Ok(eval) = surface.evaluate_full(u, v) {
            // Check curvature
            let k = eval.k1.abs().max(eval.k2.abs());
            max_curvature = max_curvature.max(k);

            // Collect normals for deviation check
            normals.push(eval.normal);
        }
    }

    // Check normal deviation
    for i in 0..normals.len() {
        for j in i + 1..normals.len() {
            if let Ok(angle) = normals[i].angle(&normals[j]) {
                max_normal_deviation = max_normal_deviation.max(angle);
            }
        }
    }

    // Subdivision criteria
    let patch_size = ((u_max - u_min).powi(2) + (v_max - v_min).powi(2)).sqrt();

    // Curvature-based criterion
    if max_curvature > 1e-10 {
        let required_size = (8.0 * params.chord_tolerance / max_curvature).sqrt();
        if patch_size > required_size {
            return true;
        }
    }

    // Normal deviation criterion
    if max_normal_deviation > params.max_angle_deviation {
        return true;
    }

    // Edge length criterion
    let estimated_edge_length = patch_size
        * surface
            .point_at(u_min, v_min)
            .map(|p1| {
                surface
                    .point_at(u_max, v_max)
                    .map(|p2| p1.distance(&p2) / patch_size)
                    .unwrap_or(1.0)
            })
            .unwrap_or(1.0);

    estimated_edge_length > params.max_edge_length
}

/// Convert quadtree to triangle mesh
fn quad_tree_to_mesh(
    quad_tree: &QuadTree,
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    mesh: &mut TriangleMesh,
) -> HashMap<(usize, usize), u32> {
    let mut vertex_map = HashMap::new();

    // Process all leaf nodes
    for (_idx, node) in quad_tree.nodes.iter().enumerate() {
        if node.children.is_none() {
            // This is a leaf node - tessellate it
            let vertices = [
                (node.u_min, node.v_min),
                (node.u_max, node.v_min),
                (node.u_max, node.v_max),
                (node.u_min, node.v_max),
            ];

            let mut indices = Vec::new();

            for &(u, v) in &vertices {
                if is_point_inside_face(u, v, face, model) {
                    let key = discretize_uv(u, v);
                    let vertex_idx = *vertex_map.entry(key).or_insert_with(|| {
                        if let (Ok(point), Ok(normal)) = (
                            surface.point_at(u, v),
                            face.normal_at(u, v, &model.surfaces),
                        ) {
                            mesh.add_vertex(MeshVertex {
                                position: point,
                                normal,
                                uv: Some((u, v)),
                            })
                        } else {
                            0 // Should not happen with proper face boundaries
                        }
                    });
                    indices.push(vertex_idx);
                }
            }

            // Create triangles if we have all 4 vertices
            if indices.len() == 4 {
                mesh.add_triangle(indices[0], indices[1], indices[2]);
                mesh.add_triangle(indices[0], indices[2], indices[3]);
            } else if indices.len() == 3 {
                mesh.add_triangle(indices[0], indices[1], indices[2]);
            }
        }
    }

    vertex_map
}

/// Discretize UV coordinates for vertex sharing
fn discretize_uv(u: f64, v: f64) -> (usize, usize) {
    const RESOLUTION: f64 = 1e6;
    (
        (u * RESOLUTION).round() as usize,
        (v * RESOLUTION).round() as usize,
    )
}

/// Ensure the mesh is watertight by welding vertices
fn ensure_watertight_mesh(mesh: &mut TriangleMesh, _vertex_map: &HashMap<(usize, usize), u32>) {
    // Build spatial hash for fast vertex lookup
    let mut spatial_hash: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    const GRID_SIZE: f64 = 0.001; // 1mm grid

    // Hash all vertices
    for (i, vertex) in mesh.vertices.iter().enumerate() {
        let x = vertex.position.x;
        let y = vertex.position.y;
        let z = vertex.position.z;

        let grid_x = (x / GRID_SIZE).floor() as i32;
        let grid_y = (y / GRID_SIZE).floor() as i32;
        let grid_z = (z / GRID_SIZE).floor() as i32;

        spatial_hash
            .entry((grid_x, grid_y, grid_z))
            .or_default()
            .push(i as u32);
    }

    // Build vertex remapping for welding
    let mut vertex_remap: HashMap<u32, u32> = HashMap::new();
    let weld_tolerance = 1e-6;

    for i in 0..mesh.vertices.len() {
        if vertex_remap.contains_key(&(i as u32)) {
            continue;
        }

        let pos = mesh.vertices[i].position;

        // Check neighboring grid cells
        let grid_x = (pos.x / GRID_SIZE).floor() as i32;
        let grid_y = (pos.y / GRID_SIZE).floor() as i32;
        let grid_z = (pos.z / GRID_SIZE).floor() as i32;

        let mut found_match = false;

        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(candidates) =
                        spatial_hash.get(&(grid_x + dx, grid_y + dy, grid_z + dz))
                    {
                        for &candidate in candidates {
                            if candidate >= i as u32 {
                                continue;
                            }

                            let candidate_pos = mesh.vertices[candidate as usize].position;

                            if pos.distance(&candidate_pos) < weld_tolerance {
                                vertex_remap.insert(i as u32, candidate);
                                found_match = true;
                                break;
                            }
                        }
                    }
                    if found_match {
                        break;
                    }
                }
                if found_match {
                    break;
                }
            }
            if found_match {
                break;
            }
        }

        if !found_match {
            vertex_remap.insert(i as u32, i as u32);
        }
    }

    // Remap indices
    for triangle in &mut mesh.triangles {
        for vertex_idx in triangle {
            if let Some(&new_idx) = vertex_remap.get(vertex_idx) {
                *vertex_idx = new_idx;
            }
        }
    }

    // Log welding statistics
    let welded_count = vertex_remap
        .iter()
        .filter(|(&key, &value)| key != value)
        .count();
    if welded_count > 0 {
        tracing::debug!("Welded {} vertices for watertight mesh", welded_count);
    }
}

/*
#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::builder::Builder;

    #[test]
    fn test_planar_face_tessellation() {
        let mut builder = Builder::new();
        let solid_id = builder.box_primitive(1.0, 1.0, 1.0, None).unwrap();

        let mut mesh = ThreeJsMesh::new();
        let params = TessellationParams::default();

        // Get first face
        let solid = builder.model.solids.get(solid_id).unwrap();
        let shell = builder.model.shells.get(solid.outer_shell).unwrap();
        if let Some(&face_id) = shell.faces.get(0) {
            if let Some(face) = builder.model.faces.get(face_id) {
                tessellate_face(face, &builder.model, &params, &mut mesh);

                // A planar face should have at least one triangle
                assert!(mesh.triangle_count() > 0);
            }
        }
    }
}
*/
