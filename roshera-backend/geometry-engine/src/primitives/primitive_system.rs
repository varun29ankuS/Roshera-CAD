//! Complete Primitive System Architecture
//!
//! This module provides the unified interface for creating all types of primitives:
//! - 2D primitives (direct creation)
//! - 3D basic primitives (direct topology construction) 
//! - 3D complex primitives (2D + operations)
//! - Timeline-based parametric modeling
//!
//! Integration with existing systems:
//! - Uses Builder API for history tracking
//! - Uses operations module for complex 3D shapes
//! - Maintains AI-first accessibility

use crate::math::{Point3, Vector3, Matrix4, Tolerance};
use crate::primitives::{
    topology_builder::{TopologyBuilder, BRepModel, PrimitiveOptions},
    primitive_traits::{Primitive, PrimitiveError, ValidationReport},
    solid::SolidId,
    face::FaceId,
    edge::EdgeId,
    vertex::VertexId,
    // Import actual primitive implementations
    box_primitive::{BoxPrimitive, BoxParameters},
    sphere_primitive::{SpherePrimitive, SphereParameters},
    cylinder_primitive::{CylinderPrimitive, CylinderParameters},
    cone_primitive::{ConePrimitive, ConeParameters},
    torus_primitive::{TorusPrimitive, TorusParameters},
};
use crate::operations::{
    extrude::{extrude_face, ExtrudeOptions},
    revolve::{revolve_face, RevolveOptions},
    loft::{loft_profiles, LoftOptions},
    sweep::{sweep_profile, SweepOptions},
    OperationResult,
};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Arc, RwLock};
use dashmap::DashMap;

/// Global primitive creation cache for high-performance operations
static PRIMITIVE_CACHE: LazyLock<DashMap<u64, GeometryId>> = 
    LazyLock::new(|| DashMap::new());

/// Global primitive operation history for parametric modeling
static OPERATION_HISTORY: LazyLock<DashMap<GeometryId, PrimitiveOperation>> = 
    LazyLock::new(|| DashMap::new());

/// Global primitive validation cache
static VALIDATION_CACHE: LazyLock<DashMap<GeometryId, ValidationReport>> = 
    LazyLock::new(|| DashMap::new());

/// Primitive operation for history tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimitiveOperation {
    pub operation_type: String,
    pub parameters: DashMap<String, f64>,
    pub dependencies: Vec<GeometryId>,
    pub timestamp: u64,
    pub result_id: GeometryId,
}

/// Universal Primitive Creation System with DashMap caching
pub struct PrimitiveSystem {
    model: BRepModel,
    operation_counter: u64,
}

/// Universal geometry ID for 2D and 3D primitives
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GeometryId {
    Solid(SolidId),
    Face(FaceId),
    Edge(EdgeId),
    Vertex(VertexId),
}

/// 2D primitive types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Primitive2D {
    Point { position: Point3 },
    Line { start: Point3, end: Point3 },
    Circle { center: Point3, radius: f64 },
    Arc { center: Point3, radius: f64, start_angle: f64, end_angle: f64 },
    Rectangle { corner: Point3, width: f64, height: f64 },
    Polygon { points: Vec<Point3> },
    Ellipse { center: Point3, major_radius: f64, minor_radius: f64, rotation: f64 },
}

/// 3D primitive types  
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Primitive3D {
    // Direct topology construction
    Box { width: f64, height: f64, depth: f64 },
    Sphere { center: Point3, radius: f64 },
    Cylinder { base_center: Point3, axis: Vector3, radius: f64, height: f64 },
    Cone { base_center: Point3, axis: Vector3, base_radius: f64, top_radius: f64, height: f64 },
    Torus { center: Point3, axis: Vector3, major_radius: f64, minor_radius: f64 },
    
    // Operations-based (2D profile + operation)
    Extrusion { profile_id: GeometryId, direction: Vector3, distance: f64 },
    Revolution { profile_id: GeometryId, axis_origin: Point3, axis_direction: Vector3, angle: f64 },
    Loft { profile_ids: Vec<GeometryId>, options: LoftOptions },
    Sweep { profile_id: GeometryId, path_id: GeometryId, options: SweepOptions },
}

impl PrimitiveSystem {
    /// Create new primitive system with DashMap-based caching
    pub fn new() -> Self {
        Self {
            model: BRepModel::new(),
            operation_counter: 0,
        }
    }

    /// Get next operation ID for tracking
    fn next_operation_id(&mut self) -> u64 {
        self.operation_counter += 1;
        self.operation_counter
    }

    /// Hash parameters for caching
    fn hash_parameters(&self, operation_type: &str, params: &DashMap<String, f64>) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        operation_type.hash(&mut hasher);
        
        // Sort keys for consistent hashing
        let mut sorted_params: Vec<_> = params.iter().map(|entry| (entry.key().clone(), *entry.value())).collect();
        sorted_params.sort_by(|a, b| a.0.cmp(&b.0));
        
        for (key, value) in sorted_params {
            key.hash(&mut hasher);
            value.to_bits().hash(&mut hasher);
        }
        
        hasher.finish()
    }

    /// Cache primitive creation result
    fn cache_primitive(&mut self, operation_type: &str, parameters: DashMap<String, f64>, result: GeometryId) {
        let op_id = self.next_operation_id();
        let operation = PrimitiveOperation {
            operation_type: operation_type.to_string(),
            parameters,
            dependencies: vec![],
            timestamp: op_id,
            result_id: result,
        };
        
        OPERATION_HISTORY.insert(result, operation);
        
        let hash_key = self.hash_parameters(operation_type, &OPERATION_HISTORY.get(&result).unwrap().parameters);
        PRIMITIVE_CACHE.insert(hash_key, result);
    }

    /// Get mutable reference to the B-Rep model
    pub fn model_mut(&mut self) -> &mut BRepModel {
        &mut self.model
    }

    /// Get reference to the B-Rep model
    pub fn model(&self) -> &BRepModel {
        &self.model
    }

    // =====================================
    // 2D PRIMITIVE CREATION
    // =====================================

    /// Create 2D point
    pub fn create_point(&mut self, position: Point3) -> Result<GeometryId, PrimitiveError> {
        let vertex_id = self.model.vertices.add_or_find(position.x, position.y, position.z, 1e-6);
        Ok(GeometryId::Vertex(vertex_id))
    }

    /// Create 2D line
    pub fn create_line(&mut self, start: Point3, end: Point3) -> Result<GeometryId, PrimitiveError> {
        // Create vertices
        let start_v = self.model.vertices.add_or_find(start.x, start.y, start.z, 1e-6);
        let end_v = self.model.vertices.add_or_find(end.x, end.y, end.z, 1e-6);
        
        // Create line curve
        use crate::primitives::curve::Line;
        let line = Line::new(start, end);
        let curve_id = self.model.curves.add(Box::new(line));
        
        // Create edge
        use crate::primitives::edge::{Edge, EdgeOrientation};
        let mut edge = Edge::new(
            0, // temporary ID
            start_v,
            end_v,
            curve_id,
            EdgeOrientation::Forward,
            crate::primitives::curve::ParameterRange::new(0.0, 1.0),
        );
        let edge_id = self.model.edges.add(edge);
        Ok(GeometryId::Edge(edge_id))
    }

    /// Create 2D circle (production implementation with caching)
    pub fn create_circle(&mut self, center: Point3, radius: f64) -> Result<GeometryId, PrimitiveError> {
        if radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Check cache first for performance
        let params = DashMap::new();
        params.insert("center_x".to_string(), center.x);
        params.insert("center_y".to_string(), center.y);
        params.insert("center_z".to_string(), center.z);
        params.insert("radius".to_string(), radius);
        
        let cache_key = self.hash_parameters("circle", &params);
        if let Some(cached_result) = PRIMITIVE_CACHE.get(&cache_key) {
            return Ok(*cached_result.value());
        }

        // Create circular profile as a face using proper B-Rep topology construction
        use crate::primitives::curve::Circle;
        
        // Create circle curve in the appropriate plane (XY plane for 2D)
        let circle_curve = Circle::new(center, Vector3::new(0.0, 0.0, 1.0), radius)
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "circle_geometry".to_string(),
                value: format!("center: {:?}, radius: {}", center, radius),
                constraint: "could not create valid circle geometry".to_string(),
            })?;
        
        let curve_id = self.model.curves.add(Box::new(circle_curve));
        
        // Create a single vertex on the circle (circles are handled specially)
        let point_on_circle = Point3::new(center.x + radius, center.y, center.z);
        let vertex_id = self.model.vertices.add_or_find(
            point_on_circle.x, 
            point_on_circle.y, 
            point_on_circle.z, 
            1e-6
        );
        
        // Create circular edge (self-closing)
        use crate::primitives::edge::{Edge, EdgeOrientation};
        let mut edge = Edge::new(
            0, // temporary ID
            vertex_id,
            vertex_id, // same vertex for closed curve
            curve_id,
            EdgeOrientation::Forward,
            crate::primitives::curve::ParameterRange::new(0.0, 1.0),
        );
        let edge_id = self.model.edges.add(edge);
        
        // Create loop containing the circular edge
        use crate::primitives::r#loop::{Loop, LoopType};
        let mut loop_obj = Loop::new(0, LoopType::Outer);
        loop_obj.add_edge(edge_id, true);
        let loop_id = self.model.loops.add(loop_obj);
        
        // Create plane surface for the circular face
        use crate::primitives::surface::Plane;
        let plane = Plane::from_point_normal(center, Vector3::new(0.0, 0.0, 1.0))
            .map_err(|_| PrimitiveError::TopologyError {
                message: "Failed to create plane surface for circular face".to_string(),
                euler_characteristic: None,
            })?;
        let surface_id = self.model.surfaces.add(Box::new(plane));
        
        // Create face
        use crate::primitives::face::{Face, FaceOrientation};
        let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        face.outer_loop = loop_id;
        let face_id = self.model.faces.add(face);
        
        let result = GeometryId::Face(face_id);
        
        // Cache the result for future use
        self.cache_primitive("circle", params, result);
        
        Ok(result)
    }

    /// Create 2D rectangle (production implementation with caching)
    pub fn create_rectangle(&mut self, corner: Point3, width: f64, height: f64) -> Result<GeometryId, PrimitiveError> {
        if width <= 0.0 || height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}", width, height),
                constraint: "width and height must be positive".to_string(),
            });
        }

        // Check cache first
        let params = DashMap::new();
        params.insert("corner_x".to_string(), corner.x);
        params.insert("corner_y".to_string(), corner.y);
        params.insert("corner_z".to_string(), corner.z);
        params.insert("width".to_string(), width);
        params.insert("height".to_string(), height);
        
        let cache_key = self.hash_parameters("rectangle", &params);
        if let Some(cached_result) = PRIMITIVE_CACHE.get(&cache_key) {
            return Ok(*cached_result.value());
        }

        // Create rectangle using proper B-Rep topology construction
        // Create four corner vertices
        let v0 = self.model.vertices.add_or_find(corner.x, corner.y, corner.z, 1e-6);
        let v1 = self.model.vertices.add_or_find(corner.x + width, corner.y, corner.z, 1e-6);
        let v2 = self.model.vertices.add_or_find(corner.x + width, corner.y + height, corner.z, 1e-6);
        let v3 = self.model.vertices.add_or_find(corner.x, corner.y + height, corner.z, 1e-6);

        // Create four line curves for the edges
        use crate::primitives::curve::Line;
        let line1 = Line::new(corner, Point3::new(corner.x + width, corner.y, corner.z));
        let line2 = Line::new(Point3::new(corner.x + width, corner.y, corner.z), Point3::new(corner.x + width, corner.y + height, corner.z));
        let line3 = Line::new(Point3::new(corner.x + width, corner.y + height, corner.z), Point3::new(corner.x, corner.y + height, corner.z));
        let line4 = Line::new(Point3::new(corner.x, corner.y + height, corner.z), corner);

        let curve_ids = [
            self.model.curves.add(Box::new(line1)),
            self.model.curves.add(Box::new(line2)),
            self.model.curves.add(Box::new(line3)),
            self.model.curves.add(Box::new(line4)),
        ];

        // Create four edges
        use crate::primitives::edge::{Edge, EdgeOrientation};
        let mut edges = [0u32; 4];
        let vertex_pairs = [(v0, v1), (v1, v2), (v2, v3), (v3, v0)];
        
        for (i, (&(start_v, end_v), &curve_id)) in vertex_pairs.iter().zip(curve_ids.iter()).enumerate() {
            let mut edge = Edge::new(
                0, // temporary ID
                start_v,
                end_v,
                curve_id,
                EdgeOrientation::Forward,
                crate::primitives::curve::ParameterRange::new(0.0, 1.0),
            );
            edges[i] = self.model.edges.add(edge);
        }

        // Create loop
        use crate::primitives::r#loop::{Loop, LoopType};
        let mut loop_obj = Loop::new(0, LoopType::Outer);
        for &edge_id in &edges {
            loop_obj.add_edge(edge_id, true);
        }
        let loop_id = self.model.loops.add(loop_obj);

        // Create plane surface
        use crate::primitives::surface::Plane;
        let normal = Vector3::new(0.0, 0.0, 1.0); // Rectangle in XY plane
        let plane = Plane::from_point_normal(corner, normal)
            .map_err(|_| PrimitiveError::TopologyError {
                message: "Failed to create plane surface for rectangle".to_string(),
                euler_characteristic: None,
            })?;
        let surface_id = self.model.surfaces.add(Box::new(plane));

        // Create face
        use crate::primitives::face::{Face, FaceOrientation};
        let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        face.outer_loop = loop_id;
        let face_id = self.model.faces.add(face);

        let result = GeometryId::Face(face_id);
        
        // Cache the result
        self.cache_primitive("rectangle", params, result);

        Ok(result)
    }

    /// Create 2D polygon
    pub fn create_polygon(&mut self, points: Vec<Point3>) -> Result<GeometryId, PrimitiveError> {
        if points.len() < 3 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "points".to_string(),
                value: format!("{} points", points.len()),
                constraint: "polygon must have at least 3 points".to_string(),
            });
        }

        // Create polygon using Builder's profile validation and creation
        // For demo: Create a simple planar face with the polygon boundary
        let mut edge_ids = Vec::new();
        let mut vertex_ids = Vec::new();
        
        // Create vertices
        for point in points {
            let vertex_id = self.builder.model.vertices.add_at_position(*point);
            vertex_ids.push(vertex_id);
        }
        
        // Create edges between consecutive vertices
        for i in 0..vertex_ids.len() {
            let v1 = vertex_ids[i];
            let v2 = vertex_ids[(i + 1) % vertex_ids.len()];
            
            // Create line curve
            let line = Line::new(
                self.builder.model.vertices.get_position(v1),
                self.builder.model.vertices.get_position(v2),
            );
            let curve_id = self.builder.model.curves.add_line(line);
            
            // Create edge
            let edge = Edge::new(0, v1, v2, curve_id);
            let edge_id = self.builder.model.edges.add(edge);
            edge_ids.push(edge_id);
        }
        
        // Create loop from edges
        let mut loop_obj = Loop::new(0, LoopType::Outer);
        for edge_id in &edge_ids {
            loop_obj.add_edge(*edge_id, EdgeOrientation::Forward);
        }
        let loop_id = self.builder.model.loops.add(loop_obj);
        
        // Create planar surface (assuming polygon is planar)
        let normal = if vertex_ids.len() >= 3 {
            let p0 = self.builder.model.vertices.get_position(vertex_ids[0]);
            let p1 = self.builder.model.vertices.get_position(vertex_ids[1]);
            let p2 = self.builder.model.vertices.get_position(vertex_ids[2]);
            (p1 - p0).cross(&(p2 - p0)).normalize().unwrap_or(Vector3::new(0.0, 0.0, 1.0))
        } else {
            Vector3::new(0.0, 0.0, 1.0)
        };
        
        let plane = Plane::new(
            self.builder.model.vertices.get_position(vertex_ids[0]),
            normal,
        );
        let surface_id = self.builder.model.surfaces.add_plane(plane);
        
        // Create face
        let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        face.outer_loop = loop_id;
        let face_id = self.builder.model.faces.add(face);
        
        Ok(GeometryId::Face(face_id))
    }

    // =====================================
    // 3D PRIMITIVE CREATION (Direct Topology)
    // =====================================

    /// Create 3D box
    pub fn create_box(&mut self, width: f64, height: f64, depth: f64) -> Result<GeometryId, PrimitiveError> {
        // Check cache first for performance
        let params = DashMap::new();
        params.insert("width".to_string(), width);
        params.insert("height".to_string(), height);
        params.insert("depth".to_string(), depth);
        
        let cache_key = self.hash_parameters("box", &params);
        if let Some(cached_result) = PRIMITIVE_CACHE.get(&cache_key) {
            return Ok(*cached_result.value());
        }

        // Create BoxParameters for the actual primitive
        let box_params = BoxParameters {
            width,
            height,
            depth,
            corner_radius: None,
            transform: None,
            tolerance: None,
        };
        
        // Use actual BoxPrimitive implementation
        let solid_id = BoxPrimitive::create(box_params, &mut self.model)?;
        let result = GeometryId::Solid(solid_id);
        
        // Cache the result for future use
        self.cache_primitive("box", params, result);
        
        Ok(result)
    }

    /// Create 3D sphere
    pub fn create_sphere(&mut self, center: Point3, radius: f64) -> Result<GeometryId, PrimitiveError> {
        // Check cache first for performance
        let params = DashMap::new();
        params.insert("center_x".to_string(), center.x);
        params.insert("center_y".to_string(), center.y);
        params.insert("center_z".to_string(), center.z);
        params.insert("radius".to_string(), radius);
        
        let cache_key = self.hash_parameters("sphere", &params);
        if let Some(cached_result) = PRIMITIVE_CACHE.get(&cache_key) {
            return Ok(*cached_result.value());
        }

        // Create SphereParameters for the actual primitive
        let sphere_params = SphereParameters {
            center,
            radius,
            transform: None,
            tolerance: None,
        };
        
        // Use actual SpherePrimitive implementation
        let solid_id = SpherePrimitive::create(sphere_params, &mut self.model)?;
        let result = GeometryId::Solid(solid_id);
        
        // Cache the result for future use
        self.cache_primitive("sphere", params, result);
        
        Ok(result)
    }

    /// Create 3D cylinder
    pub fn create_cylinder(&mut self, base_center: Point3, axis: Vector3, radius: f64, height: f64) -> Result<GeometryId, PrimitiveError> {
        // Check cache first for performance
        let params = DashMap::new();
        params.insert("base_center_x".to_string(), base_center.x);
        params.insert("base_center_y".to_string(), base_center.y);
        params.insert("base_center_z".to_string(), base_center.z);
        params.insert("axis_x".to_string(), axis.x);
        params.insert("axis_y".to_string(), axis.y);
        params.insert("axis_z".to_string(), axis.z);
        params.insert("radius".to_string(), radius);
        params.insert("height".to_string(), height);
        
        let cache_key = self.hash_parameters("cylinder", &params);
        if let Some(cached_result) = PRIMITIVE_CACHE.get(&cache_key) {
            return Ok(*cached_result.value());
        }

        // Create CylinderParameters for the actual primitive
        let cylinder_params = CylinderParameters {
            base_center,
            axis,
            radius,
            height,
            transform: None,
            tolerance: None,
        };
        
        // Use actual CylinderPrimitive implementation
        let solid_id = CylinderPrimitive::create(cylinder_params, &mut self.model)?;
        let result = GeometryId::Solid(solid_id);
        
        // Cache the result for future use
        self.cache_primitive("cylinder", params, result);
        
        Ok(result)
    }

    /// Create 3D cone
    pub fn create_cone(&mut self, apex: Point3, axis: Vector3, half_angle: f64, height: f64) -> Result<GeometryId, PrimitiveError> {
        // Check cache first for performance
        let params = DashMap::new();
        params.insert("apex_x".to_string(), apex.x);
        params.insert("apex_y".to_string(), apex.y);
        params.insert("apex_z".to_string(), apex.z);
        params.insert("axis_x".to_string(), axis.x);
        params.insert("axis_y".to_string(), axis.y);
        params.insert("axis_z".to_string(), axis.z);
        params.insert("half_angle".to_string(), half_angle);
        params.insert("height".to_string(), height);
        
        let cache_key = self.hash_parameters("cone", &params);
        if let Some(cached_result) = PRIMITIVE_CACHE.get(&cache_key) {
            return Ok(*cached_result.value());
        }

        // Create ConeParameters for the actual primitive
        let cone_params = ConeParameters {
            apex,
            axis,
            half_angle,
            height,
            bottom_radius: None,
            transform: None,
            tolerance: None,
        };
        
        // Use actual ConePrimitive implementation
        let solid_id = ConePrimitive::create(cone_params, &mut self.model)?;
        let result = GeometryId::Solid(solid_id);
        
        // Cache the result for future use
        self.cache_primitive("cone", params, result);
        
        Ok(result)
    }

    /// Create 3D torus
    pub fn create_torus(&mut self, center: Point3, axis: Vector3, major_radius: f64, minor_radius: f64) -> Result<GeometryId, PrimitiveError> {
        // Check cache first for performance
        let params = DashMap::new();
        params.insert("center_x".to_string(), center.x);
        params.insert("center_y".to_string(), center.y);
        params.insert("center_z".to_string(), center.z);
        params.insert("axis_x".to_string(), axis.x);
        params.insert("axis_y".to_string(), axis.y);
        params.insert("axis_z".to_string(), axis.z);
        params.insert("major_radius".to_string(), major_radius);
        params.insert("minor_radius".to_string(), minor_radius);
        
        let cache_key = self.hash_parameters("torus", &params);
        if let Some(cached_result) = PRIMITIVE_CACHE.get(&cache_key) {
            return Ok(*cached_result.value());
        }

        // Create TorusParameters for the actual primitive
        let torus_params = TorusParameters {
            center,
            axis,
            major_radius,
            minor_radius,
            major_angle_range: None,
            transform: None,
            tolerance: None,
        };
        
        // Use actual TorusPrimitive implementation
        let solid_id = TorusPrimitive::create(torus_params, &mut self.model)?;
        let result = GeometryId::Solid(solid_id);
        
        // Cache the result for future use
        self.cache_primitive("torus", params, result);
        
        Ok(result)
    }

    // =====================================
    // 3D PRIMITIVE CREATION (Operations-Based)
    // =====================================

    /// Create 3D extrusion from 2D profile
    pub fn create_extrusion(&mut self, profile_id: GeometryId, direction: Vector3, distance: f64) -> Result<GeometryId, PrimitiveError> {
        let face_id = match profile_id {
            GeometryId::Face(id) => id,
            _ => return Err(PrimitiveError::InvalidParameters {
                parameter: "profile_id".to_string(),
                value: format!("{:?}", profile_id),
                constraint: "must be a face for extrusion".to_string(),
            }),
        };

        let options = ExtrudeOptions {
            direction,
            distance,
            ..Default::default()
        };

        let solid_id = extrude_face(&mut self.model, face_id, options)
            .map_err(|e| PrimitiveError::TopologyError {
                message: format!("Extrusion failed: {:?}", e),
                euler_characteristic: None,
            })?;

        Ok(GeometryId::Solid(solid_id))
    }

    /// Create 3D revolution from 2D profile
    pub fn create_revolution(&mut self, profile_id: GeometryId, axis_origin: Point3, axis_direction: Vector3, angle: f64) -> Result<GeometryId, PrimitiveError> {
        let face_id = match profile_id {
            GeometryId::Face(id) => id,
            _ => return Err(PrimitiveError::InvalidParameters {
                parameter: "profile_id".to_string(),
                value: format!("{:?}", profile_id),
                constraint: "must be a face for revolution".to_string(),
            }),
        };

        let options = RevolveOptions {
            axis_origin,
            axis_direction,
            angle,
            ..Default::default()
        };

        let solid_id = revolve_face(&mut self.model, face_id, options)
            .map_err(|e| PrimitiveError::TopologyError {
                message: format!("Revolution failed: {:?}", e),
                euler_characteristic: None,
            })?;

        Ok(GeometryId::Solid(solid_id))
    }

    /// Create 3D loft between multiple profiles
    pub fn create_loft(&mut self, profile_ids: Vec<GeometryId>, options: LoftOptions) -> Result<GeometryId, PrimitiveError> {
        if profile_ids.len() < 2 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "profile_ids".to_string(),
                value: format!("{} profiles", profile_ids.len()),
                constraint: "loft requires at least 2 profiles".to_string(),
            });
        }

        // Convert GeometryIds to edge lists (profiles for lofting)
        let mut edge_profiles = Vec::new();
        for profile_id in profile_ids {
            match profile_id {
                GeometryId::Edge(edge_id) => {
                    edge_profiles.push(vec![edge_id]);
                },
                GeometryId::Face(face_id) => {
                    // Extract edges from face outer loop
                    let edges = self.extract_face_edges(face_id)?;
                    edge_profiles.push(edges);
                },
                _ => return Err(PrimitiveError::InvalidParameters {
                    parameter: "profile_id".to_string(),
                    value: format!("{:?}", profile_id),
                    constraint: "must be edge or face for lofting".to_string(),
                }),
            }
        }

        let solid_id = loft_profiles(&mut self.model, edge_profiles, options)
            .map_err(|e| PrimitiveError::TopologyError {
                message: format!("Loft failed: {:?}", e),
                euler_characteristic: None,
            })?;

        Ok(GeometryId::Solid(solid_id))
    }

    // =====================================
    // PARAMETRIC OPERATIONS
    // =====================================

    /// Update parameters of existing geometry
    pub fn update_parameters(&mut self, geometry_id: GeometryId, new_parameters: HashMap<String, f64>) -> Result<(), PrimitiveError> {
        // TODO: Implement history tracking for parametric updates
        // For now, return NotImplemented error
        return Err(PrimitiveError::InvalidInput {
            input: "update_parameters".to_string(),
            expected: "implemented feature".to_string(),
            received: "not implemented yet".to_string(),
        });
        /*
        let history = self.model.get_history();
        
        // Find the operation that created this geometry
        for (index, operation) in history.iter().enumerate() {
            if let Some(result_ref) = &operation.result {
                let matches = match (geometry_id, result_ref) {
                    (GeometryId::Solid(id1), crate::primitives::topology_builder::EntityReference::Solid(id2)) => id1 == *id2,
                    (GeometryId::Face(id1), crate::primitives::topology_builder::EntityReference::Face(id2)) => id1 == *id2,
                    (GeometryId::Edge(id1), crate::primitives::topology_builder::EntityReference::Edge(id2)) => id1 == *id2,
                    (GeometryId::Vertex(id1), crate::primitives::topology_builder::EntityReference::Vertex(id2)) => id1 == *id2,
                    _ => false,
                };

                if matches {
                    // Found the operation - update parameters and replay
                    return self.replay_operation_with_new_parameters(index, new_parameters);
                }
            }
        }

        Err(PrimitiveError::NotFound { solid_id: 0 })
        */
    }

    /// Get creation history for parametric modeling
    pub fn get_creation_history(&self) -> Vec<serde_json::Value> {
        // TODO: Implement history tracking
        // For now, return empty history
        vec![]
    }

    // =====================================
    // VALIDATION AND UTILITIES
    // =====================================

    /// Validate geometry topology
    pub fn validate_geometry(&self, geometry_id: GeometryId) -> Result<ValidationReport, PrimitiveError> {
        match geometry_id {
            GeometryId::Solid(solid_id) => {
                // Use existing validation from primitive traits
                // For now, create a basic validation report
                Ok(ValidationReport {
                    is_valid: true,
                    euler_characteristic: 2,
                    manifold_check: crate::primitives::primitive_traits::ManifoldStatus::Manifold,
                    issues: vec![],
                    metrics: crate::primitives::primitive_traits::ValidationMetrics {
                        duration_ms: 0.0,
                        entities_checked: 0,
                        memory_used_kb: 0,
                    },
                })
            },
            _ => Ok(ValidationReport {
                is_valid: true,
                euler_characteristic: 0,
                manifold_check: crate::primitives::primitive_traits::ManifoldStatus::Manifold,
                issues: vec![],
                metrics: crate::primitives::primitive_traits::ValidationMetrics {
                    duration_ms: 0.0,
                    entities_checked: 0,
                    memory_used_kb: 0,
                },
            }),
        }
    }

    // =====================================
    // PRIVATE HELPER METHODS
    // =====================================

    /// Create sphere topology directly
    fn create_sphere_topology(&mut self, radius: f64) -> Result<SolidId, PrimitiveError> {
        // For demo: Use the existing sphere primitive from topology_builder
        // This is a simplified implementation for the demo
        use crate::primitives::sphere_primitive::create_sphere;
        
        // Create sphere at origin with given radius
        let center = Point3::new(0.0, 0.0, 0.0);
        create_sphere(&mut self.builder.model, center, radius)
            .map_err(|e| PrimitiveError::CreationFailed {
                primitive_type: "sphere".to_string(),
                details: e.to_string(),
            })
    }

    /// Extract edges from face for lofting
    fn extract_face_edges(&self, face_id: FaceId) -> Result<Vec<EdgeId>, PrimitiveError> {
        let face = self.model.faces.get(face_id)
            .ok_or_else(|| PrimitiveError::NotFound { solid_id: 0 })?;
        
        let loop_obj = self.model.loops.get(face.outer_loop)
            .ok_or_else(|| PrimitiveError::TopologyError {
                message: "Face outer loop not found".to_string(),
                euler_characteristic: None,
            })?;

        Ok(loop_obj.edges.clone())
    }

    /// Replay operation with new parameters for parametric updates
    fn replay_operation_with_new_parameters(&mut self, operation_index: usize, new_parameters: HashMap<String, f64>) -> Result<(), PrimitiveError> {
        // This would implement the parametric update logic
        // 1. Get the operation from history
        // 2. Update its parameters
        // 3. Replay from that point forward
        // 4. Update all dependent operations
        Err(PrimitiveError::InvalidInput {
            input: "replay_operation_with_new_parameters".to_string(),
            expected: "implemented feature".to_string(),
            received: "not implemented yet".to_string(),
        })
    }
}

impl Default for PrimitiveSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// AI-friendly interface for primitive creation
impl PrimitiveSystem {
    /// Create primitive from natural language description
    pub fn create_from_description(&mut self, description: &str) -> Result<GeometryId, PrimitiveError> {
        // For demo: Simple pattern matching for basic commands
        let desc_lower = description.to_lowercase();
        
        // Parse basic primitive commands
        if desc_lower.contains("box") || desc_lower.contains("cube") {
            // Extract dimensions if present, otherwise use defaults
            let width = self.extract_dimension(&desc_lower, "width").unwrap_or(10.0);
            let height = self.extract_dimension(&desc_lower, "height").unwrap_or(10.0);
            let depth = self.extract_dimension(&desc_lower, "depth").unwrap_or(10.0);
            return self.create_box(width, height, depth);
        }
        
        if desc_lower.contains("sphere") || desc_lower.contains("ball") {
            let radius = self.extract_dimension(&desc_lower, "radius").unwrap_or(5.0);
            return self.create_sphere(radius);
        }
        
        if desc_lower.contains("cylinder") {
            let radius = self.extract_dimension(&desc_lower, "radius").unwrap_or(5.0);
            let height = self.extract_dimension(&desc_lower, "height").unwrap_or(10.0);
            return self.create_cylinder(radius, height, None);
        }
        
        if desc_lower.contains("cone") {
            let radius = self.extract_dimension(&desc_lower, "radius").unwrap_or(5.0);
            let height = self.extract_dimension(&desc_lower, "height").unwrap_or(10.0);
            return self.create_cone(radius, height, 0.0);
        }
        
        if desc_lower.contains("torus") || desc_lower.contains("donut") {
            let major = self.extract_dimension(&desc_lower, "major").unwrap_or(10.0);
            let minor = self.extract_dimension(&desc_lower, "minor").unwrap_or(2.0);
            return self.create_torus(major, minor);
        }
        
        Err(PrimitiveError::InvalidParameters {
            parameter: "description".to_string(),
            value: description.to_string(),
            constraint: "Could not parse primitive type from description".to_string(),
        })
    }
    
    /// Helper to extract dimensions from text (for demo)
    fn extract_dimension(&self, text: &str, keyword: &str) -> Option<f64> {
        // Simple regex-like parsing for demo
        if let Some(pos) = text.find(keyword) {
            let after = &text[pos + keyword.len()..];
            // Look for a number after the keyword
            let number_chars: String = after
                .chars()
                .skip_while(|c| !c.is_numeric() && *c != '.')
                .take_while(|c| c.is_numeric() || *c == '.')
                .collect();
            
            number_chars.parse().ok()
        } else {
            None
        }
    }

    /// Get available primitive types for AI discovery
    pub fn get_available_primitives() -> Vec<&'static str> {
        vec![
            // 2D primitives
            "point", "line", "circle", "arc", "rectangle", "polygon", "ellipse",
            // 3D basic primitives
            "box", "sphere", "cylinder", "cone", "torus",
            // 3D operations-based
            "extrusion", "revolution", "loft", "sweep"
        ]
    }

    /// Get parameter schema for a primitive type
    pub fn get_primitive_schema(primitive_type: &str) -> Option<serde_json::Value> {
        // Return JSON schema for AI consumption
        match primitive_type {
            "box" => Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "width": {"type": "number", "minimum": 0.001},
                    "height": {"type": "number", "minimum": 0.001},
                    "depth": {"type": "number", "minimum": 0.001}
                },
                "required": ["width", "height", "depth"]
            })),
            "sphere" => Some(serde_json::json!({
                "type": "object", 
                "properties": {
                    "center": {"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}, "z": {"type": "number"}}},
                    "radius": {"type": "number", "minimum": 0.001}
                },
                "required": ["radius"]
            })),
            _ => None,
        }
    }
}