//! Geometry serialization for LLM reasoning.
//!
//! Converts B-Rep solids into structured summaries that LLMs can consume
//! in their context window. Produces both human-readable text and JSON.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::math::{MathResult, Point3, Vector3};
use crate::primitives::topology_builder::BRepModel;

use super::feature_recognition::{recognize_features, RecognizedFeature};

/// Complete geometry summary for a solid, suitable for LLM context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometrySummary {
    pub solid_id: u32,
    pub name: Option<String>,
    pub topology: TopologyCounts,
    pub dimensions: BoundingBoxInfo,
    pub mass_properties: MassInfo,
    pub surface_types: Vec<SurfaceTypeCount>,
    pub features: Vec<RecognizedFeature>,
    pub aspect_ratios: AspectRatios,
    pub classification: ShapeClassification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyCounts {
    pub shells: usize,
    pub faces: usize,
    pub edges: usize,
    pub vertices: usize,
    pub inner_loops: usize,
    pub euler_characteristic: i32,
    pub genus: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBoxInfo {
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub size_x: f64,
    pub size_y: f64,
    pub size_z: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MassInfo {
    pub volume: f64,
    pub surface_area: f64,
    pub center_of_mass: [f64; 3],
    pub compactness: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceTypeCount {
    pub surface_type: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AspectRatios {
    /// max_dim / min_dim
    pub max_ratio: f64,
    /// Sorted dimensions: [smallest, middle, largest]
    pub sorted_dims: [f64; 3],
    /// Sphericity: (pi^(1/3) * (6V)^(2/3)) / A — 1.0 for perfect sphere
    pub sphericity: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeClassification {
    /// All dimensions roughly equal (max_ratio < 2)
    Compact,
    /// One dimension much smaller than others (plate, sheet)
    Flat,
    /// One dimension much larger than others (rod, beam)
    Elongated,
    /// Two dimensions much larger than one (slab)
    Slab,
    /// Cannot classify
    Irregular,
}

impl fmt::Display for ShapeClassification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShapeClassification::Compact => write!(f, "compact block"),
            ShapeClassification::Flat => write!(f, "flat/thin part"),
            ShapeClassification::Elongated => write!(f, "elongated/rod-like"),
            ShapeClassification::Slab => write!(f, "slab-like"),
            ShapeClassification::Irregular => write!(f, "irregular shape"),
        }
    }
}

/// Summarize a solid's geometry for LLM consumption.
///
/// Walks the B-Rep topology, computes mass properties, classifies surface types,
/// recognizes features, and produces a structured summary.
pub fn summarize_solid(solid_id: u32, model: &BRepModel) -> MathResult<GeometrySummary> {
    let solid = model.solids.get(solid_id).ok_or_else(|| {
        crate::math::MathError::InvalidParameter(format!("Solid {} not found", solid_id))
    })?;

    let name = solid.name.clone();

    // --- Topology counts ---
    let outer_shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| crate::math::MathError::InvalidParameter("Outer shell not found".into()))?;

    let mut face_count = 0usize;
    let mut edge_set = std::collections::HashSet::new();
    let mut vertex_set = std::collections::HashSet::new();
    let mut inner_loop_count = 0usize;
    let mut surface_type_map: HashMap<String, usize> = HashMap::new();

    let all_shell_ids = solid.all_shells();
    let shell_count = all_shell_ids.len();

    for &shell_id in &all_shell_ids {
        if let Some(shell) = model.shells.get(shell_id) {
            for &face_id in &shell.faces {
                face_count += 1;

                if let Some(face) = model.faces.get(face_id) {
                    // Count surface types
                    if let Some(surface) = model.surfaces.get(face.surface_id) {
                        let type_name = format!("{:?}", surface.surface_type());
                        *surface_type_map.entry(type_name).or_insert(0) += 1;
                    }

                    // Count inner loops (holes)
                    inner_loop_count += face.inner_loops.len();

                    // Collect edges and vertices from all loops
                    for loop_id in face.all_loops() {
                        if let Some(loop_data) = model.loops.get(loop_id) {
                            for &edge_id in &loop_data.edges {
                                edge_set.insert(edge_id);
                                if let Some(edge) = model.edges.get(edge_id) {
                                    vertex_set.insert(edge.start_vertex);
                                    vertex_set.insert(edge.end_vertex);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let edge_count = edge_set.len();
    let vertex_count = vertex_set.len();
    let euler_characteristic = vertex_count as i32 - edge_count as i32 + face_count as i32;
    // genus = (2 - χ) / 2 for closed orientable surface
    let genus = if shell_count > 0 {
        (2 * shell_count as i32 - euler_characteristic) / 2
    } else {
        0
    };

    let topology = TopologyCounts {
        shells: shell_count,
        faces: face_count,
        edges: edge_count,
        vertices: vertex_count,
        inner_loops: inner_loop_count,
        euler_characteristic,
        genus,
    };

    // --- Bounding box ---
    let dimensions = compute_bounding_box(solid_id, model, &vertex_set)?;

    // --- Mass properties ---
    let volume = model.calculate_solid_volume(solid_id).unwrap_or(0.0);
    let surface_area = model.calculate_solid_surface_area(solid_id).unwrap_or(0.0);

    let center_of_mass = compute_center_of_mass(&vertex_set, model, &dimensions);

    let compactness = if surface_area > 1e-12 {
        let v_23 = volume.abs().powf(2.0 / 3.0);
        std::f64::consts::PI.cbrt() * (6.0 * v_23) / surface_area
    } else {
        0.0
    };

    let mass_properties = MassInfo {
        volume: volume.abs(),
        surface_area,
        center_of_mass,
        compactness,
    };

    // --- Surface type distribution ---
    let mut surface_types: Vec<SurfaceTypeCount> = surface_type_map
        .into_iter()
        .map(|(surface_type, count)| SurfaceTypeCount {
            surface_type,
            count,
        })
        .collect();
    surface_types.sort_by(|a, b| b.count.cmp(&a.count));

    // --- Features ---
    let features = recognize_features(solid_id, model);

    // --- Aspect ratios ---
    let mut dims = [dimensions.size_x, dimensions.size_y, dimensions.size_z];
    dims.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let max_ratio = if dims[0] > 1e-12 {
        dims[2] / dims[0]
    } else {
        f64::INFINITY
    };

    let sphericity = if surface_area > 1e-12 {
        let numerator = std::f64::consts::PI.cbrt() * (6.0 * volume.abs()).powf(2.0 / 3.0);
        numerator / surface_area
    } else {
        0.0
    };

    let aspect_ratios = AspectRatios {
        max_ratio,
        sorted_dims: dims,
        sphericity,
    };

    // --- Classification ---
    let classification = classify_shape(&dims, max_ratio);

    Ok(GeometrySummary {
        solid_id,
        name,
        topology,
        dimensions,
        mass_properties,
        surface_types,
        features,
        aspect_ratios,
        classification,
    })
}

fn compute_bounding_box(
    solid_id: u32,
    model: &BRepModel,
    vertex_set: &std::collections::HashSet<u32>,
) -> MathResult<BoundingBoxInfo> {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];

    // First try vertices
    for &v_id in vertex_set {
        if let Some(vertex) = model.vertices.get(v_id) {
            for i in 0..3 {
                if vertex.position[i] < min[i] {
                    min[i] = vertex.position[i];
                }
                if vertex.position[i] > max[i] {
                    max[i] = vertex.position[i];
                }
            }
        }
    }

    // If no vertices (e.g., sphere, degenerate topology), sample surface points
    if min[0] == f64::INFINITY {
        if let Some(solid) = model.solids.get(solid_id) {
            for &shell_id in &solid.all_shells() {
                if let Some(shell) = model.shells.get(shell_id) {
                    for &face_id in &shell.faces {
                        if let Some(face) = model.faces.get(face_id) {
                            if let Some(surface) = model.surfaces.get(face.surface_id) {
                                let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
                                // Sample a grid of points on each surface
                                let n = 8;
                                for ui in 0..=n {
                                    for vi in 0..=n {
                                        let u = u_min + (u_max - u_min) * (ui as f64 / n as f64);
                                        let v = v_min + (v_max - v_min) * (vi as f64 / n as f64);
                                        if let Ok(pt) = surface.point_at(u, v) {
                                            let coords = [pt.x, pt.y, pt.z];
                                            for i in 0..3 {
                                                if coords[i] < min[i] {
                                                    min[i] = coords[i];
                                                }
                                                if coords[i] > max[i] {
                                                    max[i] = coords[i];
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if min[0] == f64::INFINITY {
        return Err(crate::math::MathError::InvalidParameter(
            "No geometry found for bounding box".into(),
        ));
    }

    Ok(BoundingBoxInfo {
        min,
        max,
        size_x: max[0] - min[0],
        size_y: max[1] - min[1],
        size_z: max[2] - min[2],
    })
}

fn compute_center_of_mass(
    vertex_set: &std::collections::HashSet<u32>,
    model: &BRepModel,
    bbox: &BoundingBoxInfo,
) -> [f64; 3] {
    let mut sum = [0.0f64; 3];
    let mut count = 0usize;

    for &v_id in vertex_set {
        if let Some(vertex) = model.vertices.get(v_id) {
            for i in 0..3 {
                sum[i] += vertex.position[i];
            }
            count += 1;
        }
    }

    if count > 0 {
        let n = count as f64;
        [sum[0] / n, sum[1] / n, sum[2] / n]
    } else {
        // Fallback: use bounding box center
        [
            (bbox.min[0] + bbox.max[0]) / 2.0,
            (bbox.min[1] + bbox.max[1]) / 2.0,
            (bbox.min[2] + bbox.max[2]) / 2.0,
        ]
    }
}

fn classify_shape(sorted_dims: &[f64; 3], max_ratio: f64) -> ShapeClassification {
    if max_ratio < 2.0 {
        return ShapeClassification::Compact;
    }

    let min = sorted_dims[0];
    let mid = sorted_dims[1];
    let big = sorted_dims[2];

    if min < 1e-12 {
        return ShapeClassification::Irregular;
    }

    let ratio_mid_min = mid / min;
    let ratio_big_mid = big / mid;

    // Flat: two big dimensions, one small (min << mid ≈ big)
    if ratio_mid_min > 3.0 && ratio_big_mid < 2.0 {
        return ShapeClassification::Flat;
    }

    // Elongated: one big dimension, two small (min ≈ mid << big)
    if ratio_mid_min < 2.0 && ratio_big_mid > 3.0 {
        return ShapeClassification::Elongated;
    }

    // Slab: graduated dimensions
    if ratio_mid_min > 2.0 && ratio_big_mid > 2.0 {
        return ShapeClassification::Slab;
    }

    ShapeClassification::Irregular
}

impl GeometrySummary {
    /// Produce human-readable text for LLM context.
    pub fn to_llm_text(&self) -> String {
        let mut lines = Vec::new();

        // Header
        if let Some(ref name) = self.name {
            lines.push(format!("Solid \"{}\" (id: {})", name, self.solid_id));
        } else {
            lines.push(format!("Solid (id: {})", self.solid_id));
        }

        // Topology
        let surface_desc: Vec<String> = self
            .surface_types
            .iter()
            .map(|s| format!("{} {}", s.count, s.surface_type.to_lowercase()))
            .collect();
        let surface_str = if surface_desc.is_empty() {
            String::new()
        } else {
            format!(" ({})", surface_desc.join(", "))
        };

        lines.push(format!(
            "Topology: {} faces{}, {} edges, {} vertices, {} shells",
            self.topology.faces,
            surface_str,
            self.topology.edges,
            self.topology.vertices,
            self.topology.shells,
        ));

        if self.topology.inner_loops > 0 {
            lines.push(format!(
                "  {} inner loops (holes in faces), genus {}",
                self.topology.inner_loops, self.topology.genus
            ));
        }

        // Dimensions
        lines.push(format!(
            "Bounding box: {:.1} x {:.1} x {:.1} mm",
            self.dimensions.size_x, self.dimensions.size_y, self.dimensions.size_z,
        ));

        // Mass properties
        lines.push(format!(
            "Volume: {:.1} mm³, Surface area: {:.1} mm²",
            self.mass_properties.volume, self.mass_properties.surface_area,
        ));

        if self.mass_properties.compactness > 0.0 {
            lines.push(format!(
                "Compactness: {:.3} (1.0 = sphere), Sphericity: {:.3}",
                self.mass_properties.compactness, self.aspect_ratios.sphericity,
            ));
        }

        // Features
        if !self.features.is_empty() {
            let feature_strs: Vec<String> =
                self.features.iter().map(|f| f.to_description()).collect();
            lines.push(format!("Features: {}", feature_strs.join("; ")));
        } else {
            lines.push("Features: none detected".into());
        }

        // Classification
        lines.push(format!("Classification: {}", self.classification));

        lines.join("\n")
    }

    /// Produce JSON for structured tool responses.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::primitive_traits::Primitive;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> u32 {
        let mut builder = TopologyBuilder::new(model);
        match builder.create_box_3d(w, h, d).unwrap() {
            GeometryId::Solid(id) => id,
            other => panic!("Expected Solid, got {:?}", other),
        }
    }

    fn make_cylinder(model: &mut BRepModel, r: f64, h: f64) -> u32 {
        use crate::primitives::cylinder_primitive::{CylinderParameters, CylinderPrimitive};
        let params = CylinderParameters::new(r, h).unwrap();
        CylinderPrimitive::create(params, model).unwrap()
    }

    fn make_sphere(model: &mut BRepModel, r: f64) -> u32 {
        use crate::primitives::sphere_primitive::{SphereParameters, SpherePrimitive};
        let params = SphereParameters::new(r, Point3::new(0.0, 0.0, 0.0)).unwrap();
        SpherePrimitive::create(params, model).unwrap()
    }

    #[test]
    fn test_box_summary() {
        let mut model = BRepModel::new();
        let solid_id = make_box(&mut model, 50.0, 30.0, 20.0);

        let summary = summarize_solid(solid_id, &model).unwrap();

        assert_eq!(summary.topology.faces, 6);
        assert_eq!(summary.topology.edges, 12);
        assert_eq!(summary.topology.vertices, 8);
        assert_eq!(summary.topology.shells, 1);
        assert_eq!(summary.topology.inner_loops, 0);

        // Euler: V - E + F = 8 - 12 + 6 = 2
        assert_eq!(summary.topology.euler_characteristic, 2);

        // Dimensions
        assert!((summary.dimensions.size_x - 50.0).abs() < 1e-6);
        assert!((summary.dimensions.size_y - 30.0).abs() < 1e-6);
        assert!((summary.dimensions.size_z - 20.0).abs() < 1e-6);

        // All surfaces should be planar
        assert_eq!(summary.surface_types.len(), 1);
        assert_eq!(summary.surface_types[0].surface_type, "Plane");
        assert_eq!(summary.surface_types[0].count, 6);

        // Classification: max_ratio = 50/20 = 2.5, not compact but not extremely elongated
        assert!(summary.classification != ShapeClassification::Compact);
    }

    #[test]
    fn test_cylinder_summary() {
        let mut model = BRepModel::new();
        let solid_id = make_cylinder(&mut model, 10.0, 30.0);

        let summary = summarize_solid(solid_id, &model).unwrap();

        // Cylinder primitive creates segmented topology (default 16 segments)
        // so it has planar caps + multiple cylindrical side faces
        let plane_count: usize = summary
            .surface_types
            .iter()
            .filter(|s| s.surface_type == "Plane")
            .map(|s| s.count)
            .sum();
        let cyl_count: usize = summary
            .surface_types
            .iter()
            .filter(|s| s.surface_type == "Cylinder")
            .map(|s| s.count)
            .sum();

        assert!(
            plane_count >= 2,
            "Cylinder should have at least 2 planar caps, got {}",
            plane_count
        );
        assert!(
            cyl_count >= 1,
            "Cylinder should have cylindrical faces, got {}",
            cyl_count
        );
        assert!(
            summary.topology.faces > 2,
            "Cylinder should have more than 2 faces"
        );
    }

    #[test]
    fn test_sphere_summary() {
        let mut model = BRepModel::new();
        let solid_id = make_sphere(&mut model, 15.0);

        let summary = summarize_solid(solid_id, &model).unwrap();

        let sphere_count: usize = summary
            .surface_types
            .iter()
            .filter(|s| s.surface_type == "Sphere")
            .map(|s| s.count)
            .sum();

        assert!(
            sphere_count >= 1,
            "Sphere should have at least 1 spherical face"
        );
    }

    #[test]
    fn test_summary_to_llm_text() {
        let mut model = BRepModel::new();
        let solid_id = make_box(&mut model, 100.0, 100.0, 50.0);

        let summary = summarize_solid(solid_id, &model).unwrap();
        let text = summary.to_llm_text();

        assert!(text.contains("6 faces"));
        assert!(text.contains("12 edges"));
        assert!(text.contains("8 vertices"));
        assert!(text.contains("100.0 x 100.0 x 50.0"));
        assert!(text.contains("plane"));
    }

    #[test]
    fn test_summary_to_json() {
        let mut model = BRepModel::new();
        let solid_id = make_box(&mut model, 20.0, 20.0, 20.0);

        let summary = summarize_solid(solid_id, &model).unwrap();
        let json = summary.to_json();

        assert!(json.is_object());
        assert!(json["topology"]["faces"].as_u64().unwrap() == 6);
        assert!(json["solid_id"].as_u64().unwrap() == solid_id as u64);
    }
}
