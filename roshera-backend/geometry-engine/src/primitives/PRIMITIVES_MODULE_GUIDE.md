# Roshera CAD Primitives Module - Technical Guide

This document provides a comprehensive guide to every file in the primitives module, explaining their purpose, implementation details, and practical applications.

## Table of Contents
1. [Architecture Overview](#architecture-overview)
2. [Core Topology Elements](#core-topology-elements)
3. [Geometry Elements](#geometry-elements)
4. [Builder and Construction](#builder-and-construction)
5. [Primitive Implementations](#primitive-implementations)
6. [AI Integration](#ai-integration)
7. [Utilities and Support](#utilities-and-support)
8. [How to Use This Module](#how-to-use-this-module)

## Architecture Overview

The primitives module implements a B-Rep (Boundary Representation) topology system following this hierarchy:

```
Solid (3D volume)
  └── Shell (closed manifold)
       └── Face (trimmed surface)
            └── Loop (closed boundary)
                 └── Edge (curve segment)
                      └── Vertex (3D point)
```

## Core Topology Elements

### 1. vertex.rs - The Foundation
**Purpose**: Manages 3D points with deduplication.

**Key Features**:
- Structure of Arrays (SoA) design
- Spatial hashing for vertex deduplication
- Compact storage format
- Thread-safe concurrent access with DashMap

**Data Structures**:
```rust
pub struct VertexStore {
    positions: Vec<[f64; 3]>,      // X,Y,Z coordinates
    hashes: Vec<u64>,               // Precomputed hashes
    spatial_hash: HashMap<SpatialHashKey, Vec<u32>>,  // Spatial lookup
    tolerance: f64,                 // Deduplication tolerance
}
```

**Potential Applications**:
- Point storage and retrieval
- Vertex deduplication
- Spatial lookups

**Usage Example**:
```rust
let mut vertices = VertexStore::new();
let v1 = vertices.add_or_find(1.0, 2.0, 3.0, tolerance);
let position = vertices.get_position(v1);
```

### 2. edge.rs - Curve Connections
**Purpose**: Connects vertices with parametric curves.

**Key Features**:
- Supports parametric curves (line, arc, NURBS)
- Tracks orientation (forward/backward)
- Continuity tracking
- Tessellation support

**Data Structures**:
```rust
pub struct Edge {
    pub id: EdgeId,
    pub start_vertex: VertexId,
    pub end_vertex: VertexId,
    pub curve: CurveId,
    pub orientation: EdgeOrientation,
    pub parameter_range: ParameterRange,
}
```

**Potential Applications**:
- Wire sketches and profiles
- Edge representation
- Curve connections

### 3. loop.rs - Closed Boundaries
**Purpose**: Organizes edges into closed loops for face boundaries and holes.

**Key Features**:
- Supports outer loops (boundaries) and inner loops (holes)
- Orientation tracking
- Loop validation
- Boundary representation

**Data Structures**:
```rust
pub struct Loop {
    pub id: LoopId,
    pub loop_type: LoopType,       // Outer or Inner
    pub edges: Vec<EdgeId>,         // Ordered edge list
    pub orientations: Vec<bool>,    // Edge directions
}
```

**Potential Applications**:
- Face boundary definition
- Hole representation
- Closed contours

### 4. face.rs - Surface Regions
**Purpose**: Combines loops with surfaces to create oriented 2D regions in 3D space.

**Key Features**:
- Surface association
- UV parameter space
- Orientation tracking
- Loop management

**Data Structures**:
```rust
pub struct Face {
    pub id: FaceId,
    pub surface: SurfaceId,
    pub outer_loop: LoopId,
    pub inner_loops: Vec<LoopId>,
    pub orientation: FaceOrientation,
}
```

**Potential Applications**:
- Surface regions
- Trimmed patches
- Face representation

### 5. shell.rs - Manifold Collections
**Purpose**: Groups faces into manifolds.

**Key Features**:
- Face adjacency tracking
- Shell type management
- Manifold validation
- Topology queries

**Data Structures**:
```rust
pub struct Shell {
    pub id: ShellId,
    pub shell_type: ShellType,      // Closed or Open
    pub faces: Vec<FaceId>,
    pub adjacency: HashMap<(EdgeId, EdgeId), (FaceId, FaceId)>,
}
```

**Potential Applications**:
- Solid boundaries
- Shell structures
- Manifold collections

### 6. solid.rs - Complete Volumes
**Purpose**: Represents 3D volumes with optional internal voids.

**Key Features**:
- Multiple shell support (outer + voids)
- Feature tracking
- Material properties
- Solid operations

**Data Structures**:
```rust
pub struct Solid {
    pub id: SolidId,
    pub outer_shell: ShellId,
    pub void_shells: Vec<ShellId>,
    pub features: Vec<Feature>,
    pub material: Option<MaterialId>,
}
```

**Potential Applications**:
- 3D solid models
- Volume representation
- Complete parts

## Geometry Elements

### 7. curve.rs - Parametric Curves
**Purpose**: Curve representations for edges.

**Types Implemented**:
- **Line**: Linear segments between points
- **Arc**: Circular arcs with center, radius, angles
- **Circle**: Full circular curves
- **NURBS**: Free-form curves

**Functionality**:
- Curve evaluation
- Parameter calculations
- Point projection
- Intersection support

**Potential Applications**:
- Edge geometry
- Profile curves
- Path definitions

### 8. surface.rs - Parametric Surfaces
**Purpose**: Surface representations for faces.

**Types Implemented**:
- **Plane**: Flat surfaces (point + normal)
- **Cylinder**: Cylindrical surfaces (axis + radius)
- **Sphere**: Spherical surfaces (center + radius)
- **Cone**: Conical surfaces (apex + axis + angle)
- **Torus**: Toroidal surfaces (center + axis + radii)
- **NURBS**: Free-form surfaces

**Features**:
- Surface evaluation
- Normal computation
- UV parameterization
- Surface queries

**Potential Applications**:
- Face geometry
- Surface definitions
- Shape surfaces

### 9. fillet_surfaces.rs - Blend Surfaces
**Purpose**: Specialized surfaces for smooth transitions.

**Types**:
- **CylindricalFillet**: Constant radius along straight edges
- **ToroidalFillet**: Constant radius along curved edges
- **SphericalFillet**: Three-edge vertex blends
- **VariableRadiusFillet**: NURBS-based variable radius

**Potential Applications**:
- Edge rounding
- Smooth transitions
- Blend surfaces

## Builder and Construction

### 10. topology_builder.rs - Universal Constructor
**Purpose**: Builder for creating primitives with topology.

**Key Methods**:
- `create_box_3d()`: Creates box primitives
- `create_sphere_3d()`: Creates sphere primitives
- `create_cylinder_3d()`: Creates cylinder primitives
- `create_point_2d()`, `create_line_2d()`, etc.: 2D primitives

**Features**:
- Timeline support
- Topology validation
- Construction methods
- 2D and 3D primitive support

**Usage Example**:
```rust
let mut model = BRepModel::new();
let mut builder = TopologyBuilder::new(&mut model);
let solid_id = builder.create_box_3d(10.0, 5.0, 3.0)?;
```

### 11. primitive_traits.rs - Core Interfaces
**Purpose**: Defines traits that all primitives must implement.

**Key Traits**:
```rust
pub trait Primitive {
    type Parameters;
    fn create(params: Self::Parameters, model: &mut BRepModel) -> Result<SolidId>;
    fn update_parameters(solid_id: SolidId, params: Self::Parameters, model: &mut BRepModel) -> Result<()>;
    fn validate(solid_id: SolidId, model: &BRepModel) -> Result<ValidationReport>;
}
```

**Features**:
- Parametric update support
- Comprehensive validation
- Parameter schema for UI generation
- Error handling with recovery

## AI Integration

### 16. ai_primitive_registry.rs - Natural Language Interface
**Purpose**: Central hub for AI-CAD interaction with natural language processing.

**Key Features**:
- Command parsing with fuzzy matching
- Parameter extraction with unit conversion
- Confidence scoring
- Error recovery with suggestions
- Self-documenting API

**Example Commands**:
```
"create a box 10 by 5 by 3"
"make a sphere with radius 25mm"
"build a cylinder 50 high with 10 radius"
```

**Architecture**:
```rust
pub struct PrimitiveRegistry {
    primitives: HashMap<String, AIPrimitiveInfo>,
    nlp_rules: NLPRules,
    usage_stats: HashMap<String, UsageStats>,
}
```

### 17. natural_language_schemas.rs - AI Metadata
**Purpose**: Comprehensive metadata for AI understanding of CAD primitives.

**Contents**:
- Parameter descriptions with units
- Common value ranges
- Usage examples
- Manufacturing context
- Visual descriptions

**Schema Structure**:
```rust
pub struct PrimitiveSchema {
    pub natural_description: String,
    pub parameters: Vec<ParameterInfo>,
    pub examples: Vec<CommandExample>,
    pub common_uses: Vec<String>,
    pub visual_description: String,
}
```

### 18. primitive_examples.rs - AI Training Data
**Purpose**: Provides example commands and responses for AI training.

**Features**:
- Multiple phrasings for same command
- Parameter variations
- Error examples
- Context-aware suggestions

### 19. primitive_system.rs - AI Entry Point
**Purpose**: High-level interface for AI agents to interact with the CAD system.

**Key Methods**:
- `get_ai_catalog()`: Returns all available primitives
- `execute_natural_language()`: Processes commands
- `get_examples_for_primitive()`: Training data

## Primitive Implementations

### 12. box_primitive.rs - Rectangular Boxes
**Purpose**: Creates rectangular boxes.

**Parameters**:
- `width`, `height`, `depth`: Box dimensions

**Topology Created**:
- 8 vertices
- 12 edges
- 6 faces
- 1 shell
- 1 solid

**Applications**:
- Basic building blocks
- Bounding box representations
- Architectural elements
- Machine enclosures

### 13. sphere_primitive.rs - Spherical Solids
**Purpose**: Creates spheres.

**Parameters**:
- `radius`: Sphere radius
- `center`: Center point

**Implementation**:
- UV parameterization
- Tessellation support
- Surface generation

**Applications**:
- Ball bearings
- Dome structures
- Spherical joints
- Optical elements

### 14. cylinder_primitive.rs - Cylindrical Solids
**Purpose**: Creates cylinders.

**Parameters**:
- `radius`: Cylinder radius
- `height`: Cylinder height
- `base_center`: Base center point
- `axis`: Cylinder axis direction

**Structure**:
- Cap faces
- Side surface
- Edge loops

**Applications**:
- Shafts and pins
- Hydraulic cylinders
- Columns and pillars
- Rotational parts

### 15. plane.rs - Bounded Planes
**Purpose**: Creates bounded plane representations.

**Implementation**: Represented as thin boxes for practical use.

**Applications**:
- Cutting planes
- Reference planes
- Work planes for sketching
- Split plane operations

## AI Integration

### 16. ai_primitive_registry.rs - Natural Language Interface
**Purpose**: Central hub for AI-CAD interaction with natural language processing.

**Key Features**:
- Command parsing with fuzzy matching
- Parameter extraction with unit conversion
- Confidence scoring
- Error recovery with suggestions
- Self-documenting API

**Example Commands**:
```
"create a box 10 by 5 by 3"
"make a sphere with radius 25mm"
"build a cylinder 50 high with 10 radius"
```

**Architecture**:
```rust
pub struct PrimitiveRegistry {
    primitives: HashMap<String, AIPrimitiveInfo>,
    nlp_rules: NLPRules,
    usage_stats: HashMap<String, UsageStats>,
}
```

### 17. natural_language_schemas.rs - AI Metadata
**Purpose**: Comprehensive metadata for AI understanding of CAD primitives.

**Contents**:
- Parameter descriptions with units
- Common value ranges
- Usage examples
- Manufacturing context
- Visual descriptions

**Schema Structure**:
```rust
pub struct PrimitiveSchema {
    pub natural_description: String,
    pub parameters: Vec<ParameterInfo>,
    pub examples: Vec<CommandExample>,
    pub common_uses: Vec<String>,
    pub visual_description: String,
}
```

### 18. primitive_examples.rs - AI Training Data
**Purpose**: Provides example commands and responses for AI training.

**Features**:
- Multiple phrasings for same command
- Parameter variations
- Error examples
- Context-aware suggestions

### 19. primitive_system.rs - AI Entry Point
**Purpose**: High-level interface for AI agents to interact with the CAD system.

**Key Methods**:
- `get_ai_catalog()`: Returns all available primitives
- `execute_natural_language()`: Processes commands
- `get_examples_for_primitive()`: Training data

## Utilities and Support

### 20. validation.rs - Model Validation
**Purpose**: B-Rep model validation.

**Validation Levels**:
1. **Quick**: Basic topology connectivity
2. **Standard**: Topology + basic geometry
3. **Deep**: All checks including numerical

**Features**:
- Parallel validation support
- Repair suggestions
- Manufacturing constraint checks
- Tolerance analysis

**Checks Performed**:
- Euler characteristic (V - E + F = 2)
- Manifold properties
- Edge-face consistency
- Orientation consistency
- Gap and overlap detection

### 21. topology.rs - Advanced Queries
**Purpose**: Topology traversal and analysis.

**Functionality**:
- Adjacency graph building
- Path finding
- Topology analysis
- Euler characteristic computation
- Component detection

**Applications**:
- Feature recognition
- Topology optimization
- Model comparison
- Pathfinding for operations

### 22. tests.rs & primitive_tests/
**Purpose**: Test suites for the module.

**Test Categories**:
- Unit tests
- Integration tests
- Benchmarks

## How to Use This Module

### Basic Primitive Creation
```rust
use roshera_cad::primitives::{BRepModel, TopologyBuilder};

// Create a model and builder
let mut model = BRepModel::new();
let mut builder = TopologyBuilder::new(&mut model);

// Create a box
let box_id = builder.create_box_3d(10.0, 5.0, 3.0)?;

// Create a sphere
let sphere_id = builder.create_sphere_3d(
    Point3::new(0.0, 0.0, 10.0),  // center
    5.0                            // radius
)?;
```

### Using the AI Interface
```rust
use roshera_cad::primitives::PrimitiveSystem;

// Execute natural language command
let response = PrimitiveSystem::execute_natural_language(
    "create a box 10 by 5 by 3",
    &mut model
)?;

// Get primitive catalog for AI discovery
let catalog = PrimitiveSystem::get_ai_catalog();
```

### Parametric Updates
```rust
use roshera_cad::primitives::{BoxPrimitive, BoxParameters};

// Update box dimensions
let new_params = BoxParameters::new(15.0, 8.0, 4.0)?;
BoxPrimitive::update_parameters(box_id, new_params, &mut model)?;
```

### Validation
```rust
use roshera_cad::primitives::{validate_model, ValidationLevel};

// Validate the entire model
let report = validate_model(&model, ValidationLevel::Standard)?;
if !report.is_valid() {
    println!("Validation issues: {:?}", report.issues);
}
```

## Implementation Details

- **Data structures**: Uses various stores for vertices, edges, faces, etc.
- **Topology**: Implements B-Rep (Boundary Representation) model
- **AI Integration**: Natural language command processing
- **Validation**: Multi-level validation system

## Design Principles

1. **Data-Oriented Design**: Structure of Arrays (SoA) for potential performance benefits
2. **AI Integration**: Natural language command support
3. **Exact Geometry**: Analytical representations
4. **Topology Validation**: Manifold checking capabilities
5. **Parallel Support**: Multi-threaded operations where applicable
6. **Memory Optimization**: Compact data structures

This primitives module implements a B-Rep CAD kernel with AI integration and web deployment capabilities.