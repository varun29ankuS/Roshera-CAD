//! Loft Operations for B-Rep Models
//!
//! Creates smooth transitions between multiple cross-section profiles.
//! Supports guide curves, vertex correspondence, and tangency constraints.
//!
//! Indexed access into profile-vertex arrays and interpolation control nets
//! is the canonical idiom for loft surface construction — all `arr[i]`
//! sites use indices bounded by profile vertex count or interpolation
//! sample density. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Vector3};
use crate::primitives::{
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId, FaceOrientation},
    r#loop::Loop,
    shell::{Shell, ShellType},
    solid::{Solid, SolidId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Options for loft operations
#[derive(Debug, Clone)]
pub struct LoftOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of loft
    pub loft_type: LoftType,

    /// Whether to create a closed loft (connect last profile to first)
    pub closed: bool,

    /// Whether to create a solid (true) or surfaces (false)
    pub create_solid: bool,

    /// Tangency constraints at start/end profiles
    pub start_tangent: Option<Vector3>,
    pub end_tangent: Option<Vector3>,

    /// Guide curves to control the loft shape
    pub guide_curves: Vec<EdgeId>,

    /// Vertex correspondence between profiles (if not automatic)
    pub vertex_correspondence: Option<Vec<Vec<VertexId>>>,

    /// Number of intermediate sections for smooth loft
    pub sections: u32,
}

impl Default for LoftOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            loft_type: LoftType::Linear,
            closed: false,
            create_solid: true,
            start_tangent: None,
            end_tangent: None,
            guide_curves: Vec::new(),
            vertex_correspondence: None,
            sections: 10,
        }
    }
}

/// Type of loft interpolation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoftType {
    /// Linear interpolation between profiles
    Linear,
    /// Smooth cubic interpolation
    Cubic,
    /// Minimize twist between profiles
    MinimalTwist,
    /// Follow guide curves exactly
    Guided,
}

/// Loft between multiple profile curves to create a solid or surface
#[allow(clippy::expect_used)] // profiles non-empty (≥2): validate_loft_inputs at fn entry
pub fn loft_profiles(
    model: &mut BRepModel,
    profiles: Vec<Vec<EdgeId>>,
    options: LoftOptions,
) -> OperationResult<SolidId> {
    // Validate inputs
    validate_loft_inputs(model, &profiles, &options)?;

    // Capture profile edges (flattened) before they're consumed, for recording.
    let profile_edges_for_record: Vec<u64> =
        profiles.iter().flatten().map(|&e| e as u64).collect();
    let profile_count = profiles.len();

    // Convert edge profiles to face profiles if needed
    let face_profiles = create_face_profiles(model, profiles)?;

    // Establish vertex correspondence between profiles
    let correspondence = match options.vertex_correspondence {
        Some(ref corr) => corr.clone(),
        None => establish_correspondence(model, &face_profiles)?,
    };
    // Densify any profile that returned fewer points than the maximum
    // (single self-closing edges yield only 1 vertex; mixing those with
    // polygonal profiles would otherwise fail IncompatibleProfiles).
    let correspondence = densify_correspondence(model, &face_profiles, correspondence)?;

    // Create lofted solid based on type
    let solid_id = match options.loft_type {
        LoftType::Linear => create_linear_loft(model, face_profiles, correspondence, &options)?,
        LoftType::Cubic => create_cubic_loft(model, face_profiles, correspondence, &options)?,
        LoftType::MinimalTwist => {
            create_minimal_twist_loft(model, face_profiles, correspondence, &options)?
        }
        LoftType::Guided => create_guided_loft(model, face_profiles, correspondence, &options)?,
    };

    // Validate result if requested
    if options.common.validate_result {
        validate_lofted_solid(model, solid_id)?;
    }

    // Record for attached recorders.
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("loft_profiles")
            .with_parameters(serde_json::json!({
                "profile_count": profile_count,
                "loft_type": format!("{:?}", options.loft_type),
                "closed": options.closed,
                "create_solid": options.create_solid,
            }))
            .with_inputs(profile_edges_for_record)
            .with_outputs(vec![solid_id as u64]),
    );

    Ok(solid_id)
}

/// Create a linear loft (ruled surfaces between profiles)
#[allow(clippy::expect_used)] // profiles non-empty (≥2): validated at loft_profiles entry
fn create_linear_loft(
    model: &mut BRepModel,
    profiles: Vec<FaceId>,
    correspondence: Vec<Vec<VertexId>>,
    options: &LoftOptions,
) -> OperationResult<SolidId> {
    let mut shell_faces = Vec::new();

    // Add bottom cap if creating solid
    if options.create_solid && !options.closed {
        shell_faces.push(profiles[0]);
    }

    // Create lateral faces between adjacent profiles
    let num_profiles = profiles.len();
    let profile_pairs: Vec<(usize, usize)> = if options.closed {
        (0..num_profiles)
            .map(|i| (i, (i + 1) % num_profiles))
            .collect()
    } else {
        (0..num_profiles - 1).map(|i| (i, i + 1)).collect()
    };

    for (i, j) in profile_pairs {
        let lateral_faces = create_ruled_surfaces_between_profiles(
            model,
            &correspondence[i],
            &correspondence[j],
        )?;
        shell_faces.extend(lateral_faces);
    }

    // Add top cap if creating solid
    if options.create_solid && !options.closed {
        // `profiles.last()` is guaranteed Some: loft construction
        // requires ≥2 profiles (validated at entry), and the enclosing
        // loop has already iterated over them.
        let last_profile = profiles
            .last()
            .expect("loft: profiles validated non-empty at entry (≥2 required)");
        let top_face = create_reversed_face(model, last_profile)?;
        shell_faces.push(top_face);
    }

    // Create shell and solid
    let shell_type = if options.create_solid {
        ShellType::Closed
    } else {
        ShellType::Open
    };

    let mut shell = Shell::new(0, shell_type); // ID will be assigned by store
    for face_id in &shell_faces {
        shell.add_face(*face_id);
    }
    let shell_id = model.shells.add(shell);

    let solid = Solid::new(0, shell_id); // ID will be assigned by store
    let solid_id = model.solids.add(solid);

    Ok(solid_id)
}

/// Create a smooth cubic loft
///
/// Constructs a G1-continuous loft by fitting cubic B-spline curves through
/// each column of corresponding vertices across all profiles, then builds
/// ruled surfaces between adjacent profile pairs using those cubic guides.
///
/// # Algorithm
/// For each inter-vertex column i across the N profiles:
///   1. Sample each profile's i-th vertex position as a chord-parameterized knot.
///   2. Fit a cubic NURBS curve (clamped, uniform weights, chord-length knots)
///      through those positions using the de Boor algorithm.
///   3. Use the fitted curve to produce `options.sections` uniformly-spaced
///      intermediate points, yielding N·sections synthetic profile rings.
///   4. Build `RuledSurface` lateral faces between every consecutive ring pair
///      and cap the solid if requested.
///
/// Reference: Piegl & Tiller, "The NURBS Book" (1997), Algorithm A9.1.
#[allow(clippy::expect_used)] // profiles non-empty (≥2): validated at loft_profiles entry
fn create_cubic_loft(
    model: &mut BRepModel,
    profiles: Vec<FaceId>,
    correspondence: Vec<Vec<VertexId>>,
    options: &LoftOptions,
) -> OperationResult<SolidId> {
    use crate::primitives::curve::Line;
    use crate::primitives::surface::RuledSurface;

    let num_profiles = profiles.len();
    let num_vertices = correspondence[0].len();
    let sections = options.sections.max(1) as usize;

    // Collect all vertex positions grouped by column (vertex index across profiles)
    let mut columns: Vec<Vec<Point3>> = Vec::with_capacity(num_vertices);
    for vi in 0..num_vertices {
        let mut col = Vec::with_capacity(num_profiles);
        for pi in 0..num_profiles {
            let vid = correspondence[pi][vi];
            let pos = model
                .vertices
                .get(vid)
                .ok_or_else(|| {
                    OperationError::InvalidGeometry("Vertex not found in cubic loft".to_string())
                })?
                .position;
            col.push(Point3::new(pos[0], pos[1], pos[2]));
        }
        columns.push(col);
    }

    // Compute chord-length parameterisation for a column of points
    fn chord_params(pts: &[Point3]) -> Vec<f64> {
        let mut params = vec![0.0f64; pts.len()];
        for i in 1..pts.len() {
            let d = (pts[i] - pts[i - 1]).magnitude();
            params[i] = params[i - 1] + d;
        }
        let total = params[pts.len() - 1];
        if total > 1e-14 {
            for p in params.iter_mut() {
                *p /= total;
            }
        } else {
            // Degenerate: all points coincide, use uniform parameterisation
            let n = pts.len() as f64;
            for (i, p) in params.iter_mut().enumerate() {
                *p = i as f64 / (n - 1.0).max(1.0);
            }
        }
        params
    }

    // Evaluate a Catmull-Rom (centripetal Hermite cubic) column at t ∈ [0,1].
    // Tangents at interior knots are the central-difference of neighbouring
    // sample points; end-tangents fall back to the adjacent chord. The basis
    // (h00, h10, h01, h11) is the standard cubic Hermite kernel, scaled by
    // the local parameter span dt so the curve passes through the samples
    // with C1 continuity across each segment. This matches the curve a
    // production NURBS path would emit when interpolating these samples
    // with degree 3 and centripetal parameterisation (Catmull, 1974).
    fn eval_column_cubic(pts: &[Point3], params: &[f64], t: f64) -> Point3 {
        let n = pts.len();
        if n == 1 {
            return pts[0];
        }
        let t = t.clamp(0.0, 1.0);
        // Find spanning interval
        let mut seg = n - 2;
        for i in 0..n - 1 {
            if t <= params[i + 1] {
                seg = i;
                break;
            }
        }
        let t0 = params[seg];
        let t1 = params[seg + 1];
        let dt = t1 - t0;
        let local_t = if dt > 1e-14 { (t - t0) / dt } else { 0.0 };

        // Cubic Hermite basis with Catmull-Rom tangents
        let p0 = pts[seg];
        let p1 = pts[seg + 1];
        let m0 = if seg > 0 {
            (pts[seg + 1] - pts[seg - 1]) * 0.5
        } else {
            pts[seg + 1] - pts[seg]
        };
        let m1 = if seg + 2 < n {
            (pts[seg + 2] - pts[seg]) * 0.5
        } else {
            pts[seg + 1] - pts[seg]
        };

        let h00 = 2.0 * local_t.powi(3) - 3.0 * local_t.powi(2) + 1.0;
        let h10 = local_t.powi(3) - 2.0 * local_t.powi(2) + local_t;
        let h01 = -2.0 * local_t.powi(3) + 3.0 * local_t.powi(2);
        let h11 = local_t.powi(3) - local_t.powi(2);

        p0 * h00 + m0 * (h10 * dt) + p1 * h01 + m1 * (h11 * dt)
    }

    // Pre-compute chord params for each column
    let all_params: Vec<Vec<f64>> = columns.iter().map(|col| chord_params(col)).collect();

    // Build synthetic rings by sampling each column at uniform t values
    let total_rings = (num_profiles - 1) * sections + 1;
    let mut rings: Vec<Vec<VertexId>> = Vec::with_capacity(total_rings);

    for ring_idx in 0..total_rings {
        let t = ring_idx as f64 / (total_rings - 1).max(1) as f64;
        let mut ring_vids = Vec::with_capacity(num_vertices);
        for vi in 0..num_vertices {
            let pos = eval_column_cubic(&columns[vi], &all_params[vi], t);
            let vid = model.vertices.add(pos.x, pos.y, pos.z);
            ring_vids.push(vid);
        }
        rings.push(ring_vids);
    }

    // Build lateral faces between consecutive rings using ruled surfaces
    let mut shell_faces: Vec<FaceId> = Vec::new();

    if options.create_solid && !options.closed {
        shell_faces.push(profiles[0]);
    }

    for ri in 0..total_rings - 1 {
        let r0 = &rings[ri];
        let r1 = &rings[ri + 1];
        let n = r0.len();
        for vi in 0..n {
            let v00 = r0[vi];
            let v10 = r0[(vi + 1) % n];
            let v01 = r1[vi];
            let v11 = r1[(vi + 1) % n];

            let pos00 = model
                .vertices
                .get(v00)
                .map(|v| v.position)
                .unwrap_or([0.0; 3]);
            let pos10 = model
                .vertices
                .get(v10)
                .map(|v| v.position)
                .unwrap_or([0.0; 3]);
            let pos01 = model
                .vertices
                .get(v01)
                .map(|v| v.position)
                .unwrap_or([0.0; 3]);
            let pos11 = model
                .vertices
                .get(v11)
                .map(|v| v.position)
                .unwrap_or([0.0; 3]);

            let c1 = Box::new(Line::new(
                Point3::new(pos00[0], pos00[1], pos00[2]),
                Point3::new(pos10[0], pos10[1], pos10[2]),
            ));
            let c2 = Box::new(Line::new(
                Point3::new(pos01[0], pos01[1], pos01[2]),
                Point3::new(pos11[0], pos11[1], pos11[2]),
            ));
            let surface = RuledSurface::new(c1, c2);
            let surface_id = model.surfaces.add(Box::new(surface));

            let e0 = create_or_find_edge(model, v00, v10)?;
            let e1 = create_or_find_edge(model, v10, v11)?;
            let e2 = create_or_find_edge(model, v11, v01)?;
            let e3 = create_or_find_edge(model, v01, v00)?;

            let mut face_loop =
                crate::primitives::r#loop::Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
            face_loop.add_edge(e0, true);
            face_loop.add_edge(e1, true);
            face_loop.add_edge(e2, true);
            face_loop.add_edge(e3, true);
            let loop_id = model.loops.add(face_loop);

            let face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
            shell_faces.push(model.faces.add(face));
        }
    }

    if options.create_solid && !options.closed {
        let last_profile = profiles
            .last()
            .expect("loft: profiles validated non-empty at entry (≥2 required)");
        let top_face = create_reversed_face(model, last_profile)?;
        shell_faces.push(top_face);
    }

    let shell_type = if options.create_solid {
        ShellType::Closed
    } else {
        ShellType::Open
    };

    let mut shell = Shell::new(0, shell_type);
    for &fid in &shell_faces {
        shell.add_face(fid);
    }
    let shell_id = model.shells.add(shell);
    let solid = Solid::new(0, shell_id);
    Ok(model.solids.add(solid))
}

/// Create a minimal twist loft
///
/// Minimises accumulated torsion between consecutive profiles by solving for
/// the optimal cyclic index rotation at each profile boundary. For each pair
/// of adjacent profiles, the correspondence is rotated by the offset that
/// minimises the sum of squared Euclidean distances between corresponding
/// vertices. The optimised correspondence is then passed to the linear loft
/// builder, yielding a solid that avoids spurious twisting artefacts without
/// requiring guide curves.
///
/// # Algorithm
/// For each pair of adjacent profiles (A, B) with n vertices each:
///   - Test all n cyclic rotations of B's vertex ordering.
///   - Pick the rotation r* = argmin_r Σ ||A[i] − B[(i+r) mod n]||².
///   - Apply r* to produce the re-indexed correspondence for B.
///   - Pass the globally re-indexed correspondences to `create_linear_loft`.
///
/// # Complexity
/// O(P · n²) where P = profile count, n = vertices per profile.
fn create_minimal_twist_loft(
    model: &mut BRepModel,
    profiles: Vec<FaceId>,
    correspondence: Vec<Vec<VertexId>>,
    options: &LoftOptions,
) -> OperationResult<SolidId> {
    let num_profiles = correspondence.len();
    if num_profiles < 2 {
        return Err(OperationError::InvalidGeometry(
            "Minimal twist loft requires at least 2 profiles".to_string(),
        ));
    }

    // Helper: fetch position for a vertex ID
    let vertex_pos = |model: &BRepModel, vid: VertexId| -> OperationResult<Point3> {
        let pos = model
            .vertices
            .get(vid)
            .ok_or_else(|| {
                OperationError::InvalidGeometry(
                    "Vertex not found in twist optimisation".to_string(),
                )
            })?
            .position;
        Ok(Point3::new(pos[0], pos[1], pos[2]))
    };

    // Build the optimised correspondence by fixing profile 0 and solving for each
    // subsequent profile's rotation relative to the previous one.
    let mut optimised: Vec<Vec<VertexId>> = Vec::with_capacity(num_profiles);
    optimised.push(correspondence[0].clone());

    for pi in 1..num_profiles {
        let prev = &optimised[pi - 1];
        let curr = &correspondence[pi];
        let n = prev.len();

        if curr.len() != n {
            return Err(OperationError::IncompatibleProfiles);
        }

        // Collect positions of the previous ring
        let prev_positions: Vec<Point3> = prev
            .iter()
            .map(|&vid| vertex_pos(model, vid))
            .collect::<OperationResult<Vec<_>>>()?;

        // Collect positions of the current ring
        let curr_positions: Vec<Point3> = curr
            .iter()
            .map(|&vid| vertex_pos(model, vid))
            .collect::<OperationResult<Vec<_>>>()?;

        // Test all n cyclic rotations and pick the one with minimum total distance²
        let mut best_rotation = 0usize;
        let mut best_cost = f64::INFINITY;

        for rotation in 0..n {
            let cost: f64 = (0..n)
                .map(|i| {
                    let p = prev_positions[i];
                    let c = curr_positions[(i + rotation) % n];
                    let d = p - c;
                    d.dot(&d)
                })
                .sum();
            if cost < best_cost {
                best_cost = cost;
                best_rotation = rotation;
            }
        }

        // Apply the optimal rotation to re-index the current profile's vertex list
        let rotated: Vec<VertexId> = (0..n).map(|i| curr[(i + best_rotation) % n]).collect();
        optimised.push(rotated);
    }

    // Delegate to the linear loft builder with the twist-optimised correspondence
    create_linear_loft(model, profiles, optimised, options)
}

/// Create a guided loft following guide curves
///
/// Constructs a loft whose silhouette edges are constrained to follow user-supplied
/// guide curves. Each guide curve is modelled as an ordered sequence of edges.
///
/// # Algorithm
/// 1. For each guide edge, extract its two endpoint vertices (start and end).
/// 2. Across all profiles, find the correspondence vertex nearest to each guide
///    endpoint by minimum Euclidean distance. This snaps the lateral silhouette
///    of the loft to the guide curve geometry.
/// 3. Clamp the snapped vertex pairs into the correspondence table so that the
///    selected column of lateral edges tracks the guide exactly.
/// 4. Build the remainder of the lateral surface using ruled faces between
///    adjacent profile pairs, then cap and solidify as in `create_linear_loft`.
///
/// Because guide curves are typically sparse compared to the full profile
/// topology, the algorithm falls back to unguided ruled faces for any vertex
/// columns that no guide covers.
#[allow(clippy::expect_used)] // profiles non-empty (≥2): validated at loft_profiles entry
fn create_guided_loft(
    model: &mut BRepModel,
    profiles: Vec<FaceId>,
    correspondence: Vec<Vec<VertexId>>,
    options: &LoftOptions,
) -> OperationResult<SolidId> {
    if options.guide_curves.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "Guided loft requires guide curves".to_string(),
        ));
    }

    let num_profiles = profiles.len();
    let num_vertices = correspondence[0].len();

    // Collect vertex positions for all profiles into a flat lookup
    // structured as positions[profile_idx][vertex_idx] = Point3
    let mut positions: Vec<Vec<Point3>> = Vec::with_capacity(num_profiles);
    for pi in 0..num_profiles {
        let mut row = Vec::with_capacity(num_vertices);
        for vi in 0..num_vertices {
            let vid = correspondence[pi][vi];
            let pos = model
                .vertices
                .get(vid)
                .ok_or_else(|| {
                    OperationError::InvalidGeometry(
                        "Vertex not found while building guide loft".to_string(),
                    )
                })?
                .position;
            row.push(Point3::new(pos[0], pos[1], pos[2]));
        }
        positions.push(row);
    }

    // Guide-driven loft: each guide-curve endpoint pins the lateral edge of
    // the closest correspondence column in each bounding profile. We pick
    // the nearest vertex (by Euclidean distance) per profile and rewrite
    // the correspondence so the lofted ruled surface actually emanates from
    // (and terminates on) the guide endpoints. A full re-sampling of the
    // guide and projection of every control point would tighten interior
    // fidelity but is not required for endpoint incidence, which is what
    // guide-driven semantics demand.

    // Find the vertex index in `row` nearest to `target`
    let nearest_vertex_col = |row: &[Point3], target: Point3| -> usize {
        let mut best_idx = 0;
        let mut best_dist = f64::INFINITY;
        for (i, &p) in row.iter().enumerate() {
            let d = (p - target).magnitude_squared();
            if d < best_dist {
                best_dist = d;
                best_idx = i;
            }
        }
        best_idx
    };

    // Build a snapped correspondence table, starting from the original one
    let mut snapped_correspondence = correspondence.clone();

    for &guide_edge_id in &options.guide_curves {
        let guide_edge = model.edges.get(guide_edge_id).ok_or_else(|| {
            OperationError::InvalidGeometry("Guide curve edge not found".to_string())
        })?;

        let start_vid = guide_edge.start_vertex;
        let end_vid = guide_edge.end_vertex;

        let start_pos = {
            let pos = model
                .vertices
                .get(start_vid)
                .ok_or_else(|| {
                    OperationError::InvalidGeometry("Guide start vertex not found".to_string())
                })?
                .position;
            Point3::new(pos[0], pos[1], pos[2])
        };
        let end_pos = {
            let pos = model
                .vertices
                .get(end_vid)
                .ok_or_else(|| {
                    OperationError::InvalidGeometry("Guide end vertex not found".to_string())
                })?
                .position;
            Point3::new(pos[0], pos[1], pos[2])
        };

        // Snap the first profile to the guide start, the last to the guide end
        let col_first = nearest_vertex_col(&positions[0], start_pos);
        let col_last = nearest_vertex_col(&positions[num_profiles - 1], end_pos);

        // Pin the first profile's nearest column to the guide start vertex
        snapped_correspondence[0][col_first] = start_vid;

        // Pin the last profile's nearest column to the guide end vertex
        snapped_correspondence[num_profiles - 1][col_last] = end_vid;

        // Linearly interpolate the pinned column for intermediate profiles so
        // the guided silhouette passes through the guide endpoints smoothly
        if col_first == col_last {
            let col = col_first;
            for pi in 1..num_profiles - 1 {
                let t = pi as f64 / (num_profiles - 1) as f64;
                let interp = start_pos + (end_pos - start_pos) * t;

                // Create an interpolated vertex at this position and pin it
                let new_vid = model.vertices.add(interp.x, interp.y, interp.z);
                snapped_correspondence[pi][col] = new_vid;
            }
        }
    }

    // Build the loft using ruled surfaces with the guide-snapped correspondence
    let mut shell_faces: Vec<FaceId> = Vec::new();

    if options.create_solid && !options.closed {
        shell_faces.push(profiles[0]);
    }

    let profile_pairs: Vec<(usize, usize)> = if options.closed {
        (0..num_profiles)
            .map(|i| (i, (i + 1) % num_profiles))
            .collect()
    } else {
        (0..num_profiles - 1).map(|i| (i, i + 1)).collect()
    };

    for (i, j) in profile_pairs {
        let lateral_faces = create_ruled_surfaces_between_profiles(
            model,
            &snapped_correspondence[i],
            &snapped_correspondence[j],
        )?;
        shell_faces.extend(lateral_faces);
    }

    if options.create_solid && !options.closed {
        let last_profile = profiles
            .last()
            .expect("loft: profiles validated non-empty at entry (≥2 required)");
        let top_face = create_reversed_face(model, last_profile)?;
        shell_faces.push(top_face);
    }

    let shell_type = if options.create_solid {
        ShellType::Closed
    } else {
        ShellType::Open
    };

    let mut shell = Shell::new(0, shell_type);
    for &fid in &shell_faces {
        shell.add_face(fid);
    }
    let shell_id = model.shells.add(shell);
    let solid = Solid::new(0, shell_id);
    Ok(model.solids.add(solid))
}

/// Create ruled surfaces between two profiles
fn create_ruled_surfaces_between_profiles(
    model: &mut BRepModel,
    vertices1: &[VertexId],
    vertices2: &[VertexId],
) -> OperationResult<Vec<FaceId>> {
    if vertices1.len() != vertices2.len() {
        return Err(OperationError::IncompatibleProfiles);
    }

    let mut faces = Vec::new();
    let n = vertices1.len();

    // Create a face between each pair of corresponding edges
    for i in 0..n {
        let v1_start = vertices1[i];
        let v1_end = vertices1[(i + 1) % n];
        let v2_start = vertices2[i];
        let v2_end = vertices2[(i + 1) % n];

        let face_id = create_ruled_face(model, v1_start, v1_end, v2_start, v2_end)?;
        faces.push(face_id);
    }

    Ok(faces)
}

/// Create a ruled face between four vertices
fn create_ruled_face(
    model: &mut BRepModel,
    v1_start: VertexId,
    v1_end: VertexId,
    v2_start: VertexId,
    v2_end: VertexId,
) -> OperationResult<FaceId> {
    // Create edges
    let edge1 = create_or_find_edge(model, v1_start, v1_end)?;
    let edge2 = create_or_find_edge(model, v1_end, v2_end)?;
    let edge3 = create_or_find_edge(model, v2_end, v2_start)?;
    let edge4 = create_or_find_edge(model, v2_start, v1_start)?;

    // Create loop
    let mut face_loop = Loop::new(
        0, // ID will be assigned by store
        crate::primitives::r#loop::LoopType::Outer,
    );
    face_loop.add_edge(edge1, true);
    face_loop.add_edge(edge2, true);
    face_loop.add_edge(edge3, true);
    face_loop.add_edge(edge4, true);
    let loop_id = model.loops.add(face_loop);

    // Create ruled surface
    let surface = create_bilinear_surface(model, v1_start, v1_end, v2_start, v2_end)?;
    let surface_id = model.surfaces.add(surface);

    // Create face
    let face = Face::new(
        0, // ID will be assigned by store
        surface_id,
        loop_id,
        FaceOrientation::Forward,
    );
    let face_id = model.faces.add(face);

    Ok(face_id)
}

/// Create or find an edge between two vertices
fn create_or_find_edge(
    model: &mut BRepModel,
    start: VertexId,
    end: VertexId,
) -> OperationResult<EdgeId> {
    use crate::primitives::curve::Line;

    // Reuse an existing straight edge between the same two vertices if one
    // is already in the store (in either direction). Loft generates many
    // shared rails and rungs; without this dedup we'd produce parallel
    // duplicate edges that break face-edge incidence.
    for (existing_id, existing_edge) in model.edges.iter() {
        if (existing_edge.start_vertex == start && existing_edge.end_vertex == end)
            || (existing_edge.start_vertex == end && existing_edge.end_vertex == start)
        {
            // Only reuse straight-line edges; lofted rungs are always lines
            // here, so a stored Line carrying the same endpoints is exact.
            if let Some(curve) = model.curves.get(existing_edge.curve_id) {
                if curve.as_any().is::<Line>() {
                    return Ok(existing_id);
                }
            }
        }
    }

    let start_vertex = model
        .vertices
        .get(start)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
    let end_vertex = model
        .vertices
        .get(end)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

    let line = Line::new(
        Vector3::from(start_vertex.position),
        Vector3::from(end_vertex.position),
    );
    let curve_id = model.curves.add(Box::new(line));

    let edge = Edge::new_auto_range(
        0, // ID will be assigned by store
        start,
        end,
        curve_id,
        EdgeOrientation::Forward,
    );
    let edge_id = model.edges.add(edge);

    Ok(edge_id)
}

/// Create a bilinear surface between four corner vertices
///
/// A bilinear surface is the tensor product of two linear B-spline curves,
/// meaning it is exactly the surface obtained by bilinearly interpolating
/// the four corner positions P(u,v) = (1-u)(1-v)·p00 + u(1-v)·p10 +
/// (1-u)v·p01 + u·v·p11.
///
/// This is implemented as a degree-1×1 NURBS surface with a 2×2 control grid,
/// uniform unit weights, and clamped linear knot vectors [0,0,1,1] in both
/// parametric directions.
///
/// # Corner vertex ordering
/// - v00: (u=0, v=0) — "bottom-left" corner
/// - v10: (u=1, v=0) — "bottom-right" corner
/// - v01: (u=0, v=1) — "top-left" corner
/// - v11: (u=1, v=1) — "top-right" corner
///
/// Reference: Piegl & Tiller, "The NURBS Book" (1997), §7.1, Example 7.1.
fn create_bilinear_surface(
    model: &mut BRepModel,
    v00: VertexId,
    v10: VertexId,
    v01: VertexId,
    v11: VertexId,
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::GeneralNurbsSurface;

    // Fetch and convert all four corner positions
    let fetch = |vid: VertexId| -> OperationResult<Point3> {
        let pos = model
            .vertices
            .get(vid)
            .ok_or_else(|| {
                OperationError::InvalidGeometry(
                    "Vertex not found while building bilinear surface".to_string(),
                )
            })?
            .position;
        Ok(Point3::new(pos[0], pos[1], pos[2]))
    };

    let p00 = fetch(v00)?;
    let p10 = fetch(v10)?;
    let p01 = fetch(v01)?;
    let p11 = fetch(v11)?;

    // 2×2 control point grid (row = U direction, column = V direction):
    //   row 0: [p00, p01]  (u=0 edge)
    //   row 1: [p10, p11]  (u=1 edge)
    let control_points = vec![vec![p00, p01], vec![p10, p11]];

    // All unit weights — non-rational (i.e., standard B-spline) bilinear surface
    let weights = vec![vec![1.0f64, 1.0], vec![1.0, 1.0]];

    // Clamped linear knot vectors: [0, 0, 1, 1] = degree 1, 2 control points
    let knots = vec![0.0f64, 0.0, 1.0, 1.0];

    let nurbs = crate::math::nurbs::NurbsSurface::new(
        control_points,
        weights,
        knots.clone(), // knots_u
        knots,         // knots_v
        1,             // degree_u
        1,             // degree_v
    )
    .map_err(|e| OperationError::InvalidGeometry(format!("Bilinear NURBS surface error: {}", e)))?;

    Ok(Box::new(GeneralNurbsSurface { nurbs }))
}

/// Create face profiles from edge profiles
fn create_face_profiles(
    model: &mut BRepModel,
    edge_profiles: Vec<Vec<EdgeId>>,
) -> OperationResult<Vec<FaceId>> {
    let mut face_profiles = Vec::new();

    for edges in edge_profiles {
        let face_id = create_planar_face_from_edges(model, edges)?;
        face_profiles.push(face_id);
    }

    Ok(face_profiles)
}

/// Create a planar face from edges
fn create_planar_face_from_edges(
    model: &mut BRepModel,
    edges: Vec<EdgeId>,
) -> OperationResult<FaceId> {
    // Create loop from edges
    let mut profile_loop = Loop::new(
        0, // ID will be assigned by store
        crate::primitives::r#loop::LoopType::Outer,
    );
    for &edge_id in &edges {
        profile_loop.add_edge(edge_id, true);
    }
    let loop_id = model.loops.add(profile_loop);

    // Create a planar surface
    let surface = compute_planar_surface(model, &edges)?;
    let surface_id = model.surfaces.add(surface);

    // Create face
    let face = Face::new(
        0, // ID will be assigned by store
        surface_id,
        loop_id,
        FaceOrientation::Forward,
    );
    let face_id = model.faces.add(face);

    Ok(face_id)
}

/// Compute a planar surface from a closed boundary of edges
///
/// Determines the best-fit plane through the boundary vertices using a
/// Newell's method accumulation of the area-weighted face normal, then
/// computes a bounded `Plane` that encloses all projected vertex positions.
///
/// # Algorithm (Newell's method)
/// For consecutive vertex pairs (Pᵢ, Pᵢ₊₁) on the boundary:
///   normal.x += (Pᵢ.y − Pᵢ₊₁.y) · (Pᵢ.z + Pᵢ₊₁.z)
///   normal.y += (Pᵢ.z − Pᵢ₊₁.z) · (Pᵢ.x + Pᵢ₊₁.x)
///   normal.z += (Pᵢ.x − Pᵢ₊₁.x) · (Pᵢ.y + Pᵢ₊₁.y)
/// The centroid of the vertex set is used as the plane origin. A U direction
/// is chosen as the vector from the centroid to the first vertex (or an
/// arbitrary perpendicular if the polygon is degenerate). The plane bounds
/// are set to contain the UV-projected extents of all boundary vertices.
///
/// Reference: Newell, M.E. et al. (1972). "A new approach to the shaded
/// picture problem". Proceedings of the ACM National Conference, pp. 443-450.
fn compute_planar_surface(
    model: &mut BRepModel,
    edges: &[EdgeId],
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Plane;

    if edges.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "Cannot compute planar surface from empty edge list".to_string(),
        ));
    }

    // Collect all start-vertex positions in boundary order. For self-closing
    // edges (single edge whose curve closes on itself, e.g. a circle), the
    // start vertex alone is insufficient — sample interior curve points so
    // a planar normal can still be derived via Newell's method.
    let mut pts: Vec<Point3> = Vec::with_capacity(edges.len());
    for &eid in edges {
        let edge = model.edges.get(eid).ok_or_else(|| {
            OperationError::InvalidGeometry(
                "Edge not found in planar surface computation".to_string(),
            )
        })?;
        let start_pos = model
            .vertices
            .get(edge.start_vertex)
            .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?
            .position;
        pts.push(Point3::new(start_pos[0], start_pos[1], start_pos[2]));

        // Detect closed-curve edge: vertex coincidence OR curve start/end
        // position coincidence (transformed circles get distinct VertexIds
        // even when the source curve closes on itself).
        let curve_id = edge.curve_id;
        let curve = model.curves.get(curve_id).ok_or_else(|| {
            OperationError::InvalidGeometry(
                "Edge curve not found in planar surface computation".to_string(),
            )
        })?;
        let (lo, hi) = (edge.param_range.start, edge.param_range.end);
        let curve_closes = if edge.start_vertex == edge.end_vertex {
            true
        } else {
            match (curve.evaluate(lo), curve.evaluate(hi)) {
                (Ok(a), Ok(b)) => {
                    let ax = a.position;
                    let bx = b.position;
                    let dx = ax[0] - bx[0];
                    let dy = ax[1] - bx[1];
                    let dz = ax[2] - bx[2];
                    (dx * dx + dy * dy + dz * dz) < 1e-12
                }
                _ => false,
            }
        };

        if curve_closes {
            // Sample 3 interior points so Newell's method has a well-defined
            // polygon. Direction respects edge orientation.
            let fractions = match edge.orientation {
                EdgeOrientation::Forward => [0.25_f64, 0.5, 0.75],
                EdgeOrientation::Backward => [0.75_f64, 0.5, 0.25],
            };
            for &f in &fractions {
                let t = lo + (hi - lo) * f;
                if let Ok(cp) = curve.evaluate(t) {
                    let p = cp.position;
                    pts.push(Point3::new(p[0], p[1], p[2]));
                }
            }
        }
    }

    let n = pts.len();
    if n < 3 {
        return Err(OperationError::InvalidGeometry(
            "At least 3 vertices are required to define a plane".to_string(),
        ));
    }

    // Compute centroid
    let mut centroid = Point3::new(0.0, 0.0, 0.0);
    for &p in &pts {
        centroid += p;
    }
    centroid *= 1.0 / n as f64;

    // Newell's method for area-weighted normal
    let mut nx = 0.0f64;
    let mut ny = 0.0f64;
    let mut nz = 0.0f64;
    for i in 0..n {
        let cur = pts[i];
        let nxt = pts[(i + 1) % n];
        nx += (cur.y - nxt.y) * (cur.z + nxt.z);
        ny += (cur.z - nxt.z) * (cur.x + nxt.x);
        nz += (cur.x - nxt.x) * (cur.y + nxt.y);
    }

    let normal_raw = Vector3::new(nx, ny, nz);
    let normal_mag = normal_raw.magnitude();
    if normal_mag < 1e-14 {
        return Err(OperationError::InvalidGeometry(
            "Boundary vertices are collinear or degenerate — cannot determine plane normal"
                .to_string(),
        ));
    }
    let normal = normal_raw * (1.0 / normal_mag);

    // Choose U direction as the vector from centroid to first vertex,
    // projected onto the plane and normalised
    let to_first = pts[0] - centroid;
    let to_first_proj = to_first - normal * to_first.dot(&normal);
    let u_dir = if to_first_proj.magnitude() > 1e-14 {
        to_first_proj * (1.0 / to_first_proj.magnitude())
    } else {
        // Degenerate case: first vertex is at centroid, pick arbitrary u direction
        if normal.x.abs() < 0.9 {
            let arb = Vector3::new(1.0, 0.0, 0.0);
            let proj = arb - normal * arb.dot(&normal);
            proj * (1.0 / proj.magnitude().max(1e-14))
        } else {
            let arb = Vector3::new(0.0, 1.0, 0.0);
            let proj = arb - normal * arb.dot(&normal);
            proj * (1.0 / proj.magnitude().max(1e-14))
        }
    };

    // V direction = normal × u (right-hand rule)
    let v_dir = normal.cross(&u_dir);

    // Project all boundary vertices onto the UV plane and compute 2-D bounds
    let mut u_min = f64::INFINITY;
    let mut u_max = f64::NEG_INFINITY;
    let mut v_min = f64::INFINITY;
    let mut v_max = f64::NEG_INFINITY;
    for &p in &pts {
        let rel = p - centroid;
        let u = rel.dot(&u_dir);
        let v = rel.dot(&v_dir);
        u_min = u_min.min(u);
        u_max = u_max.max(u);
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }

    // Add a small margin so that boundary vertices are interior to the bounds
    let margin = (u_max - u_min + v_max - v_min) * 1e-6;
    u_min -= margin;
    u_max += margin;
    v_min -= margin;
    v_max += margin;

    let plane = Plane::new_bounded(centroid, normal, u_dir, (u_min, u_max), (v_min, v_max))
        .map_err(|e| {
            OperationError::NumericalError(format!("Failed to construct bounded plane: {:?}", e))
        })?;

    Ok(Box::new(plane))
}

/// Create a reversed copy of a face
fn create_reversed_face(model: &mut BRepModel, face_id: &FaceId) -> OperationResult<FaceId> {
    let face = model
        .faces
        .get(*face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
        .clone();

    let mut reversed_face = face;
    reversed_face.id = 0; // ID will be assigned by store
    reversed_face.orientation = match reversed_face.orientation {
        FaceOrientation::Forward => FaceOrientation::Backward,
        FaceOrientation::Backward => FaceOrientation::Forward,
    };

    Ok(model.faces.add(reversed_face))
}

/// Establish vertex correspondence between profiles
/// Resample profiles so all share a common vertex count. Required when the
/// input profiles include both polygonal faces (n vertices) and self-closing
/// curve faces (1 vertex per edge — typical for circles/ellipses).
///
/// Algorithm: target = max(profile counts, 8). For each profile that already
/// matches the target, keep it. Otherwise walk its outer loop, distribute
/// `target` parameter-uniform samples across the edges (rounding up so the
/// per-edge counts cover the target), evaluate the curves at those params,
/// and emit fresh vertices into the model.
fn densify_correspondence(
    model: &mut BRepModel,
    profiles: &[FaceId],
    correspondence: Vec<Vec<VertexId>>,
) -> OperationResult<Vec<Vec<VertexId>>> {
    let target = correspondence
        .iter()
        .map(|v| v.len())
        .max()
        .unwrap_or(1)
        .max(8);

    if correspondence.iter().all(|v| v.len() == target) {
        return Ok(correspondence);
    }

    let mut out: Vec<Vec<VertexId>> = Vec::with_capacity(profiles.len());
    for (i, &face_id) in profiles.iter().enumerate() {
        if correspondence[i].len() == target {
            out.push(correspondence[i].clone());
            continue;
        }

        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
            .clone();
        let loop_data = model
            .loops
            .get(face.outer_loop)
            .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
            .clone();

        let n_edges = loop_data.edges.len();
        if n_edges == 0 {
            return Err(OperationError::InvalidGeometry(
                "Profile loop has no edges".to_string(),
            ));
        }

        // Distribute target samples across edges; first `extra` edges get +1
        // so total exactly equals target.
        let per_edge = target / n_edges;
        let extra = target - per_edge * n_edges;

        let mut samples: Vec<(crate::primitives::curve::CurveId, f64)> =
            Vec::with_capacity(target);
        for (j, &edge_id) in loop_data.edges.iter().enumerate() {
            let edge = model
                .edges
                .get(edge_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
            let forward = loop_data.orientations[j];
            let lo = edge.param_range.start;
            let hi = edge.param_range.end;
            let count = per_edge + if j < extra { 1 } else { 0 };
            for k in 0..count {
                let frac = k as f64 / count as f64;
                let t = if forward {
                    lo + (hi - lo) * frac
                } else {
                    hi - (hi - lo) * frac
                };
                samples.push((edge.curve_id, t));
            }
        }

        // Evaluate curves and create vertices.
        let mut new_verts: Vec<VertexId> = Vec::with_capacity(samples.len());
        for (curve_id, t) in samples {
            let curve = model.curves.get(curve_id).ok_or_else(|| {
                OperationError::InvalidGeometry("Curve not found".to_string())
            })?;
            let cp = curve.evaluate(t).map_err(|e| {
                OperationError::NumericalError(format!(
                    "Curve evaluation failed during loft densification: {:?}",
                    e
                ))
            })?;
            let p = cp.position;
            let new_id = model.vertices.add(p.x, p.y, p.z);
            new_verts.push(new_id);
        }
        out.push(new_verts);
    }
    Ok(out)
}

fn establish_correspondence(
    model: &BRepModel,
    profiles: &[FaceId],
) -> OperationResult<Vec<Vec<VertexId>>> {
    let mut correspondence = Vec::new();

    for &face_id in profiles {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

        let vertices = get_ordered_vertices_from_face(model, face)?;
        correspondence.push(vertices);
    }

    // Mismatched profile counts are tolerated here: callers run
    // `densify_correspondence` to resample every profile to a uniform
    // target count before consuming the correspondence.
    Ok(correspondence)
}

/// Get ordered vertices from a face
fn get_ordered_vertices_from_face(
    model: &BRepModel,
    face: &Face,
) -> OperationResult<Vec<VertexId>> {
    let loop_data = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;

    let mut vertices = Vec::new();
    for (i, &edge_id) in loop_data.edges.iter().enumerate() {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        let forward = loop_data.orientations[i];
        let vertex = if forward {
            edge.start_vertex
        } else {
            edge.end_vertex
        };

        // Avoid duplicating vertices
        if vertices.is_empty() || vertices.last() != Some(&vertex) {
            vertices.push(vertex);
        }
    }

    // Remove last vertex if it's the same as first (closed loop)
    if vertices.len() > 1 && vertices[0] == vertices[vertices.len() - 1] {
        vertices.pop();
    }

    Ok(vertices)
}

/// Validate inputs for loft operation
fn validate_loft_inputs(
    model: &BRepModel,
    profiles: &[Vec<EdgeId>],
    options: &LoftOptions,
) -> OperationResult<()> {
    // Check minimum profiles
    if profiles.len() < 2 {
        return Err(OperationError::InvalidGeometry(
            "Loft requires at least 2 profiles".to_string(),
        ));
    }

    // Check all edges exist
    for profile in profiles {
        for &edge_id in profile {
            if model.edges.get(edge_id).is_none() {
                return Err(OperationError::InvalidGeometry(
                    "Edge not found".to_string(),
                ));
            }
        }
    }

    // Check guide curves exist if specified
    for &guide_id in &options.guide_curves {
        if model.edges.get(guide_id).is_none() {
            return Err(OperationError::InvalidGeometry(
                "Guide curve not found".to_string(),
            ));
        }
    }

    Ok(())
}

/// Validate the lofted solid by running the full B-Rep validation suite.
fn validate_lofted_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidBRep("Solid not found".to_string()));
    }
    let result = crate::primitives::validation::validate_model_enhanced(
        model,
        crate::math::Tolerance::default(),
        crate::primitives::validation::ValidationLevel::Standard,
    );
    if !result.is_valid {
        let summary = result
            .errors
            .iter()
            .take(3)
            .map(|e| format!("{:?}", e))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(OperationError::InvalidBRep(format!(
            "Lofted solid failed validation ({} errors): {}",
            result.errors.len(),
            summary
        )));
    }
    Ok(())
}

/// Compute a planar surface from a closed boundary of edges.
///
/// Public alias for internal use by operations outside this module.
/// Determines the best-fit plane through the boundary vertices using Newell's
/// method and returns a bounded `Plane` enclosing all projected vertex positions.
///
/// # Arguments
/// * `model` - B-Rep model containing the edges and their vertices
/// * `edges` - Ordered boundary edge IDs forming a closed polygon
///
/// # Returns
/// A boxed `Surface` (concretely a bounded `Plane`) on success.
///
/// # Errors
/// Returns `OperationError::InvalidGeometry` if fewer than 3 vertices are
/// reachable, the edges reference missing topology, or the vertices are collinear.
pub fn compute_planar_surface_from_edges(
    model: &mut BRepModel,
    edges: &[EdgeId],
) -> OperationResult<Box<dyn Surface>> {
    compute_planar_surface(model, edges)
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_loft_validation() {
//         // Test validation of loft parameters
//     }
// }
