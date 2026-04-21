//! Adaptive tessellation strategies

use super::{MeshVertex, TessellationParams, TriangleMesh};
use crate::math::{Point3, Vector3};
use crate::primitives::surface::Surface;

/// Adaptive tessellation based on surface curvature
pub struct AdaptiveTessellator {
    params: TessellationParams,
    max_depth: usize,
}

impl AdaptiveTessellator {
    pub fn new(params: TessellationParams) -> Self {
        Self {
            params,
            max_depth: 8,
        }
    }

    /// Tessellate a surface patch adaptively
    pub fn tessellate_patch(
        &self,
        surface: &dyn Surface,
        u_range: (f64, f64),
        v_range: (f64, f64),
    ) -> TriangleMesh {
        let mut mesh = TriangleMesh::new();

        // Start with corners of the patch
        let corners = [
            (u_range.0, v_range.0),
            (u_range.1, v_range.0),
            (u_range.1, v_range.1),
            (u_range.0, v_range.1),
        ];

        // Evaluate corners
        let mut corner_vertices = Vec::new();
        for &(u, v) in &corners {
            if let Ok(eval) = surface.evaluate_full(u, v) {
                let vertex = MeshVertex {
                    position: eval.position,
                    normal: eval.normal,
                    uv: Some((u, v)),
                };
                let idx = mesh.add_vertex(vertex);
                corner_vertices.push(idx);
            }
        }

        if corner_vertices.len() == 4 {
            // Recursively subdivide the patch
            self.subdivide_quad(
                surface,
                &mut mesh,
                [
                    corner_vertices[0],
                    corner_vertices[1],
                    corner_vertices[2],
                    corner_vertices[3],
                ],
                [corners[0], corners[1], corners[2], corners[3]],
                0,
            );
        }

        mesh
    }

    /// Recursively subdivide a quadrilateral
    fn subdivide_quad(
        &self,
        surface: &dyn Surface,
        mesh: &mut TriangleMesh,
        vertices: [u32; 4],
        params: [(f64, f64); 4],
        depth: usize,
    ) {
        if depth >= self.max_depth
            || self.should_stop_subdivision(surface, &params, mesh, &vertices)
        {
            // Create two triangles
            mesh.add_triangle(vertices[0], vertices[1], vertices[2]);
            mesh.add_triangle(vertices[0], vertices[2], vertices[3]);
            return;
        }

        // Calculate midpoints
        let mid_params = [
            (
                (params[0].0 + params[1].0) / 2.0,
                (params[0].1 + params[1].1) / 2.0,
            ), // Bottom
            (
                (params[1].0 + params[2].0) / 2.0,
                (params[1].1 + params[2].1) / 2.0,
            ), // Right
            (
                (params[2].0 + params[3].0) / 2.0,
                (params[2].1 + params[3].1) / 2.0,
            ), // Top
            (
                (params[3].0 + params[0].0) / 2.0,
                (params[3].1 + params[0].1) / 2.0,
            ), // Left
            (
                (params[0].0 + params[2].0) / 2.0,
                (params[0].1 + params[2].1) / 2.0,
            ), // Center
        ];

        // Create or find midpoint vertices
        let mut mid_vertices = Vec::new();
        for &(u, v) in &mid_params {
            if let Ok(eval) = surface.evaluate_full(u, v) {
                let vertex = MeshVertex {
                    position: eval.position,
                    normal: eval.normal,
                    uv: Some((u, v)),
                };
                mid_vertices.push(mesh.add_vertex(vertex));
            }
        }

        if mid_vertices.len() == 5 {
            // Subdivide into 4 quads
            self.subdivide_quad(
                surface,
                mesh,
                [
                    vertices[0],
                    mid_vertices[0],
                    mid_vertices[4],
                    mid_vertices[3],
                ],
                [params[0], mid_params[0], mid_params[4], mid_params[3]],
                depth + 1,
            );

            self.subdivide_quad(
                surface,
                mesh,
                [
                    mid_vertices[0],
                    vertices[1],
                    mid_vertices[1],
                    mid_vertices[4],
                ],
                [mid_params[0], params[1], mid_params[1], mid_params[4]],
                depth + 1,
            );

            self.subdivide_quad(
                surface,
                mesh,
                [
                    mid_vertices[4],
                    mid_vertices[1],
                    vertices[2],
                    mid_vertices[2],
                ],
                [mid_params[4], mid_params[1], params[2], mid_params[2]],
                depth + 1,
            );

            self.subdivide_quad(
                surface,
                mesh,
                [
                    mid_vertices[3],
                    mid_vertices[4],
                    mid_vertices[2],
                    vertices[3],
                ],
                [mid_params[3], mid_params[4], mid_params[2], params[3]],
                depth + 1,
            );
        }
    }

    /// Determine if subdivision should stop
    fn should_stop_subdivision(
        &self,
        surface: &dyn Surface,
        params: &[(f64, f64); 4],
        mesh: &TriangleMesh,
        vertices: &[u32; 4],
    ) -> bool {
        // Check edge lengths
        for i in 0..4 {
            let j = (i + 1) % 4;
            let v1 = &mesh.vertices[vertices[i] as usize];
            let v2 = &mesh.vertices[vertices[j] as usize];
            let edge_length = v1.position.distance(&v2.position);

            if edge_length > self.params.max_edge_length {
                return false;
            }
        }

        // Check flatness (deviation from plane)
        let flatness = self.calculate_flatness(mesh, vertices);
        if flatness > self.params.chord_tolerance {
            return false;
        }

        // Check curvature-based criteria
        if !self.check_curvature_criteria(surface, params) {
            return false;
        }

        // Check normal deviation
        if !self.check_normal_deviation(surface, params) {
            return false;
        }

        true
    }

    /// Check curvature-based subdivision criteria
    fn check_curvature_criteria(&self, surface: &dyn Surface, params: &[(f64, f64); 4]) -> bool {
        // Sample at center of patch
        let center_u = (params[0].0 + params[2].0) / 2.0;
        let center_v = (params[0].1 + params[2].1) / 2.0;

        if let Ok(eval) = surface.evaluate_full(center_u, center_v) {
            // Get principal curvatures
            let k1 = eval.k1.abs();
            let k2 = eval.k2.abs();
            let max_curvature = k1.max(k2);

            // Estimate required edge length based on curvature
            // For a curve with curvature k, chord error ≈ L²k/8
            // So L ≈ sqrt(8 * chord_tolerance / k)
            if max_curvature > 1e-10 {
                let required_edge_length =
                    (8.0 * self.params.chord_tolerance / max_curvature).sqrt();

                // Check current patch size
                let patch_size_u = (params[1].0 - params[0].0).abs();
                let patch_size_v = (params[2].1 - params[1].1).abs();
                let max_patch_size = patch_size_u.max(patch_size_v);

                if max_patch_size > required_edge_length {
                    return false; // Need more subdivision
                }
            }
        }

        true
    }

    /// Check normal deviation across the patch
    fn check_normal_deviation(&self, surface: &dyn Surface, params: &[(f64, f64); 4]) -> bool {
        // Sample normals at corners and center
        let sample_points = [
            params[0],
            params[1],
            params[2],
            params[3],
            (
                (params[0].0 + params[2].0) / 2.0,
                (params[0].1 + params[2].1) / 2.0,
            ),
        ];

        let mut normals = Vec::new();
        for &(u, v) in &sample_points {
            if let Ok(eval) = surface.evaluate_full(u, v) {
                normals.push(eval.normal);
            }
        }

        if normals.len() < 2 {
            return true; // Can't check, assume OK
        }

        // Check angle between all pairs of normals
        for i in 0..normals.len() {
            for j in i + 1..normals.len() {
                match normals[i].angle(&normals[j]) {
                    Ok(angle) if angle > self.params.max_angle_deviation => {
                        return false; // Too much normal variation
                    }
                    Err(_) => {
                        // Degenerate normal (zero-length) — force subdivision
                        return false;
                    }
                    _ => {}
                }
            }
        }

        true
    }

    /// Calculate flatness of a quad (maximum distance from plane)
    fn calculate_flatness(&self, mesh: &TriangleMesh, vertices: &[u32; 4]) -> f64 {
        let vcount = mesh.vertices.len();
        let i0 = vertices[0] as usize;
        let i1 = vertices[1] as usize;
        let i2 = vertices[2] as usize;
        let i3 = vertices[3] as usize;
        if i0 >= vcount || i1 >= vcount || i2 >= vcount || i3 >= vcount {
            return f64::MAX; // Out-of-bounds → treat as maximally non-flat
        }
        let v0 = &mesh.vertices[i0].position;
        let v1 = &mesh.vertices[i1].position;
        let v2 = &mesh.vertices[i2].position;
        let v3 = &mesh.vertices[i3].position;

        // Calculate plane through first three vertices
        let edge1 = *v1 - *v0;
        let edge2 = *v2 - *v0;
        let normal = match edge1.cross(&edge2).normalize() {
            Ok(n) => n,
            Err(_) => {
                // Degenerate triangle (collinear vertices) — report max flatness
                // so the caller treats this patch as maximally non-flat
                return f64::MAX;
            }
        };

        // Check distance of fourth vertex from plane
        let to_v3 = *v3 - *v0;
        to_v3.dot(&normal).abs()
    }
}

/// Delaunay triangulation for planar regions
pub fn delaunay_triangulate(points: &[Point3], normal: &Vector3) -> Vec<[u32; 3]> {
    if points.len() < 3 {
        return Vec::new();
    }

    if points.len() == 3 {
        return vec![[0, 1, 2]];
    }

    // Project points to 2D plane for triangulation
    let (u_axis, v_axis) = compute_plane_axes(normal);
    let origin = points[0];

    let points_2d: Vec<(f64, f64)> = points
        .iter()
        .map(|p| {
            let relative = *p - origin;
            (relative.dot(&u_axis), relative.dot(&v_axis))
        })
        .collect();

    // Perform Bowyer-Watson algorithm for Delaunay triangulation
    let triangles = bowyer_watson_2d(&points_2d);

    triangles
}

/// Compute orthonormal axes for a plane given its normal
///
/// Uses a robust two-candidate strategy: picks the cardinal axis least aligned
/// with `normal` to guarantee a non-degenerate cross product.
pub fn compute_plane_axes(normal: &Vector3) -> (Vector3, Vector3) {
    // Pick the cardinal axis least aligned with normal for maximum cross-product magnitude
    let initial = if normal.x.abs() <= normal.y.abs() && normal.x.abs() <= normal.z.abs() {
        Vector3::X
    } else if normal.y.abs() <= normal.z.abs() {
        Vector3::Y
    } else {
        Vector3::Z
    };

    // These normalizations cannot fail: `initial` is chosen to be non-parallel to `normal`
    let u_axis = normal.cross(&initial).normalize().unwrap_or(Vector3::X);
    let v_axis = normal.cross(&u_axis).normalize().unwrap_or(Vector3::Y);

    (u_axis, v_axis)
}

/// Bowyer-Watson algorithm for 2D Delaunay triangulation
fn bowyer_watson_2d(points: &[(f64, f64)]) -> Vec<[u32; 3]> {
    if points.is_empty() {
        return Vec::new();
    }

    // Create super-triangle that contains all points
    let (min_x, min_y, max_x, max_y) = compute_bounds_2d(points);
    let dx = max_x - min_x;
    let dy = max_y - min_y;
    let delta_max = dx.max(dy);
    let mid_x = (min_x + max_x) / 2.0;
    let mid_y = (min_y + max_y) / 2.0;

    // Super-triangle vertices
    let p1 = (mid_x - 2.0 * delta_max, mid_y - delta_max);
    let p2 = (mid_x, mid_y + 2.0 * delta_max);
    let p3 = (mid_x + 2.0 * delta_max, mid_y - delta_max);

    let mut vertices = vec![p1, p2, p3];
    vertices.extend_from_slice(points);

    let mut triangles = vec![[0, 1, 2]];

    // Add points one by one
    for (i, &point) in points.iter().enumerate() {
        let vertex_idx = (i + 3) as u32;
        let mut bad_triangles = Vec::new();

        // Find triangles whose circumcircle contains the point
        for (tri_idx, &triangle) in triangles.iter().enumerate() {
            let circumcircle = compute_circumcircle_2d(
                &vertices[triangle[0] as usize],
                &vertices[triangle[1] as usize],
                &vertices[triangle[2] as usize],
            );

            if point_in_circle_2d(&point, &circumcircle) {
                bad_triangles.push(tri_idx);
            }
        }

        // Find boundary of the polygonal hole
        let mut polygon = Vec::new();
        for &tri_idx in &bad_triangles {
            let triangle = triangles[tri_idx];
            for j in 0..3 {
                let edge = [triangle[j], triangle[(j + 1) % 3]];

                // Check if edge is shared with another bad triangle
                let mut is_shared = false;
                for &other_tri_idx in &bad_triangles {
                    if other_tri_idx != tri_idx {
                        let other_tri = triangles[other_tri_idx];
                        if triangle_contains_edge(&other_tri, edge[0], edge[1]) {
                            is_shared = true;
                            break;
                        }
                    }
                }

                if !is_shared {
                    polygon.push(edge);
                }
            }
        }

        // Remove bad triangles
        bad_triangles.sort_unstable_by(|a, b| b.cmp(a));
        for tri_idx in bad_triangles {
            triangles.swap_remove(tri_idx);
        }

        // Re-triangulate the polygonal hole
        for edge in polygon {
            triangles.push([edge[0], edge[1], vertex_idx]);
        }
    }

    // Remove triangles that use super-triangle vertices
    triangles.retain(|triangle| triangle[0] >= 3 && triangle[1] >= 3 && triangle[2] >= 3);

    // Adjust indices to account for super-triangle removal
    for triangle in &mut triangles {
        triangle[0] -= 3;
        triangle[1] -= 3;
        triangle[2] -= 3;
    }

    triangles
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

/// Circumcircle representation
struct Circumcircle {
    center: (f64, f64),
    radius_squared: f64,
}

/// Compute circumcircle of three 2D points
fn compute_circumcircle_2d(p1: &(f64, f64), p2: &(f64, f64), p3: &(f64, f64)) -> Circumcircle {
    let ax = p1.0;
    let ay = p1.1;
    let bx = p2.0;
    let by = p2.1;
    let cx = p3.0;
    let cy = p3.1;

    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));

    if d.abs() < 1e-10 {
        // Points are collinear, return large circle
        return Circumcircle {
            center: ((ax + bx + cx) / 3.0, (ay + by + cy) / 3.0),
            radius_squared: f64::MAX,
        };
    }

    let ux = ((ax * ax + ay * ay) * (by - cy)
        + (bx * bx + by * by) * (cy - ay)
        + (cx * cx + cy * cy) * (ay - by))
        / d;

    let uy = ((ax * ax + ay * ay) * (cx - bx)
        + (bx * bx + by * by) * (ax - cx)
        + (cx * cx + cy * cy) * (bx - ax))
        / d;

    let dx = ux - ax;
    let dy = uy - ay;

    Circumcircle {
        center: (ux, uy),
        radius_squared: dx * dx + dy * dy,
    }
}

/// Check if point is inside circumcircle
fn point_in_circle_2d(point: &(f64, f64), circle: &Circumcircle) -> bool {
    let dx = point.0 - circle.center.0;
    let dy = point.1 - circle.center.1;
    dx * dx + dy * dy < circle.radius_squared - 1e-10
}

/// Check if triangle contains edge
fn triangle_contains_edge(triangle: &[u32; 3], v1: u32, v2: u32) -> bool {
    let mut count = 0;
    for i in 0..3 {
        let j = (i + 1) % 3;
        if (triangle[i] == v1 && triangle[j] == v2) || (triangle[i] == v2 && triangle[j] == v1) {
            count += 1;
        }
    }
    count > 0
}

/// Edge flip optimization for mesh quality
pub fn optimize_mesh(mesh: &mut TriangleMesh) {
    // Build edge-to-triangle adjacency
    let mut edge_triangles: std::collections::HashMap<(u32, u32), Vec<usize>> =
        std::collections::HashMap::new();

    for (tri_idx, triangle) in mesh.triangles.iter().enumerate() {
        for i in 0..3 {
            let v1 = triangle[i];
            let v2 = triangle[(i + 1) % 3];
            let edge = if v1 < v2 { (v1, v2) } else { (v2, v1) };
            edge_triangles
                .entry(edge)
                .or_insert_with(Vec::new)
                .push(tri_idx);
        }
    }

    // Find edges that can be flipped
    let mut flips_performed = 0;
    let max_iterations = 10;

    for _ in 0..max_iterations {
        let mut edges_to_flip = Vec::new();

        for (edge, triangles) in &edge_triangles {
            if triangles.len() == 2 {
                // Check if flipping improves quality
                let tri1_idx = triangles[0];
                let tri2_idx = triangles[1];

                if should_flip_edge(mesh, tri1_idx, tri2_idx, *edge) {
                    edges_to_flip.push((*edge, tri1_idx, tri2_idx));
                }
            }
        }

        if edges_to_flip.is_empty() {
            break;
        }

        // Perform flips
        for (edge, tri1_idx, tri2_idx) in edges_to_flip {
            flip_edge(mesh, tri1_idx, tri2_idx, edge);
            flips_performed += 1;
        }

        // Rebuild adjacency for next iteration
        edge_triangles.clear();
        for (tri_idx, triangle) in mesh.triangles.iter().enumerate() {
            for i in 0..3 {
                let v1 = triangle[i];
                let v2 = triangle[(i + 1) % 3];
                let edge = if v1 < v2 { (v1, v2) } else { (v2, v1) };
                edge_triangles
                    .entry(edge)
                    .or_insert_with(Vec::new)
                    .push(tri_idx);
            }
        }
    }
}

/// Check if edge should be flipped for better quality
fn should_flip_edge(
    mesh: &TriangleMesh,
    tri1_idx: usize,
    tri2_idx: usize,
    edge: (u32, u32),
) -> bool {
    let tri1 = &mesh.triangles[tri1_idx];
    let tri2 = &mesh.triangles[tri2_idx];

    // Find the vertices not on the shared edge
    let mut v1_opposite = 0u32;
    let mut v2_opposite = 0u32;

    for &v in tri1 {
        if v != edge.0 && v != edge.1 {
            v1_opposite = v;
            break;
        }
    }

    for &v in tri2 {
        if v != edge.0 && v != edge.1 {
            v2_opposite = v;
            break;
        }
    }

    // Check Delaunay condition: v2_opposite should not be inside circumcircle of tri1
    let p1 = &mesh.vertices[edge.0 as usize].position;
    let p2 = &mesh.vertices[edge.1 as usize].position;
    let p3 = &mesh.vertices[v1_opposite as usize].position;
    let p4 = &mesh.vertices[v2_opposite as usize].position;

    // Use in-circle test
    in_circle_test_3d(p1, p2, p3, p4)
}

/// In-circle test for 3D points (project to best-fit plane)
fn in_circle_test_3d(p1: &Point3, p2: &Point3, p3: &Point3, p4: &Point3) -> bool {
    // Project to best-fit plane
    let v1 = *p2 - *p1;
    let v2 = *p3 - *p1;
    let normal = v1.cross(&v2);

    if normal.magnitude_squared() < 1e-10 {
        return false; // Degenerate triangle
    }

    // Magnitude is guaranteed > sqrt(1e-10) by the guard above.
    let normal = normal
        .normalize()
        .expect("non-degenerate normal verified by magnitude_squared guard above");
    let (u_axis, v_axis) = compute_plane_axes(&normal);

    // Project points to 2D
    let origin = *p1;
    let p1_2d = (0.0, 0.0);
    let p2_2d = ((*p2 - origin).dot(&u_axis), (*p2 - origin).dot(&v_axis));
    let p3_2d = ((*p3 - origin).dot(&u_axis), (*p3 - origin).dot(&v_axis));
    let p4_2d = ((*p4 - origin).dot(&u_axis), (*p4 - origin).dot(&v_axis));

    // Compute circumcircle of p1, p2, p3
    let circle = compute_circumcircle_2d(&p1_2d, &p2_2d, &p3_2d);

    // Check if p4 is inside
    point_in_circle_2d(&p4_2d, &circle)
}

/// Flip an edge between two triangles
fn flip_edge(mesh: &mut TriangleMesh, tri1_idx: usize, tri2_idx: usize, edge: (u32, u32)) {
    let tri1 = mesh.triangles[tri1_idx];
    let tri2 = mesh.triangles[tri2_idx];

    // Find opposite vertices
    let mut v1_opposite = 0u32;
    let mut v2_opposite = 0u32;

    for &v in &tri1 {
        if v != edge.0 && v != edge.1 {
            v1_opposite = v;
            break;
        }
    }

    for &v in &tri2 {
        if v != edge.0 && v != edge.1 {
            v2_opposite = v;
            break;
        }
    }

    // Create new triangles with flipped edge
    mesh.triangles[tri1_idx] = [v1_opposite, v2_opposite, edge.0];
    mesh.triangles[tri2_idx] = [v1_opposite, edge.1, v2_opposite];
}

/// Calculate mesh quality metrics
pub fn calculate_quality_metrics(mesh: &TriangleMesh) -> MeshQualityReport {
    let mut min_angle = std::f64::MAX;
    let mut max_angle = 0.0f64;
    let mut min_edge_length = std::f64::MAX;
    let mut max_edge_length = 0.0f64;
    let mut total_area = 0.0f64;

    for triangle in &mesh.triangles {
        let v0 = &mesh.vertices[triangle[0] as usize].position;
        let v1 = &mesh.vertices[triangle[1] as usize].position;
        let v2 = &mesh.vertices[triangle[2] as usize].position;

        // Edge lengths
        let e0 = (*v1 - *v0).magnitude();
        let e1 = (*v2 - *v1).magnitude();
        let e2 = (*v0 - *v2).magnitude();

        min_edge_length = min_edge_length.min(e0).min(e1).min(e2);
        max_edge_length = max_edge_length.max(e0).max(e1).max(e2);

        // Angles
        let angles = calculate_triangle_angles(v0, v1, v2);
        min_angle = min_angle.min(angles.0).min(angles.1).min(angles.2);
        max_angle = max_angle.max(angles.0).max(angles.1).max(angles.2);

        // Area
        let area = (*v1 - *v0).cross(&(*v2 - *v0)).magnitude() / 2.0;
        total_area += area;
    }

    MeshQualityReport {
        min_angle: min_angle.to_degrees(),
        max_angle: max_angle.to_degrees(),
        min_edge_length,
        max_edge_length,
        total_area,
        triangle_count: mesh.triangles.len(),
        vertex_count: mesh.vertices.len(),
    }
}

/// Calculate angles of a triangle in radians
fn calculate_triangle_angles(v0: &Point3, v1: &Point3, v2: &Point3) -> (f64, f64, f64) {
    let e0 = *v1 - *v0;
    let e1 = *v2 - *v1;
    let e2 = *v0 - *v2;

    let angle0 = (-e2).angle(&e0).unwrap_or(0.0);
    let angle1 = (-e0).angle(&e1).unwrap_or(0.0);
    let angle2 = (-e1).angle(&e2).unwrap_or(0.0);

    (angle0, angle1, angle2)
}

/// Mesh quality report
#[derive(Debug)]
pub struct MeshQualityReport {
    pub min_angle: f64,
    pub max_angle: f64,
    pub min_edge_length: f64,
    pub max_edge_length: f64,
    pub total_area: f64,
    pub triangle_count: usize,
    pub vertex_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::surface::Plane;

    #[test]
    fn test_adaptive_tessellation() {
        let plane = Plane::xy(0.0);
        let params = TessellationParams::default();
        let tessellator = AdaptiveTessellator::new(params);

        let mesh = tessellator.tessellate_patch(&plane, (0.0, 1.0), (0.0, 1.0));

        // Should have at least 2 triangles for a quad
        assert!(mesh.triangles.len() >= 2);
        assert!(mesh.vertices.len() >= 4);
    }

    #[test]
    fn test_mesh_quality_metrics() {
        let mut mesh = TriangleMesh::new();

        // Add a simple triangle
        let v0 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });

        let v1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(1.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });

        let v2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 1.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });

        mesh.add_triangle(v0, v1, v2);

        let quality = calculate_quality_metrics(&mesh);

        assert_eq!(quality.triangle_count, 1);
        assert_eq!(quality.vertex_count, 3);
        assert!((quality.total_area - 0.5).abs() < 1e-10);
    }

    // === Kernel hardening tests ===

    #[test]
    fn test_compute_plane_axes_x_aligned() {
        let (u, v) = compute_plane_axes(&Vector3::X);
        assert!(u.magnitude() > 0.99);
        assert!(v.magnitude() > 0.99);
        assert!(u.dot(&v).abs() < 1e-10, "Axes must be orthogonal");
        assert!(
            u.dot(&Vector3::X).abs() < 1e-10,
            "u must be perpendicular to normal"
        );
    }

    #[test]
    fn test_compute_plane_axes_y_aligned() {
        let (u, v) = compute_plane_axes(&Vector3::Y);
        assert!(u.dot(&v).abs() < 1e-10, "Axes must be orthogonal");
        assert!(u.dot(&Vector3::Y).abs() < 1e-10);
    }

    #[test]
    fn test_compute_plane_axes_z_aligned() {
        let (u, v) = compute_plane_axes(&Vector3::Z);
        assert!(u.dot(&v).abs() < 1e-10, "Axes must be orthogonal");
        assert!(u.dot(&Vector3::Z).abs() < 1e-10);
    }

    #[test]
    fn test_compute_plane_axes_diagonal() {
        let normal = Vector3::new(1.0, 1.0, 1.0).normalize().unwrap();
        let (u, v) = compute_plane_axes(&normal);
        assert!(u.dot(&v).abs() < 1e-10, "Axes must be orthogonal");
        assert!(
            u.dot(&normal).abs() < 1e-10,
            "u must be perpendicular to normal"
        );
        assert!(
            v.dot(&normal).abs() < 1e-10,
            "v must be perpendicular to normal"
        );
    }
}
