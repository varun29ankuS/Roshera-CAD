# Primitive Module Requirements - World-Class CAD Kernel

**Last Updated**: July 27, 2025  
**Target**: Production-grade parametric CAD primitives  
**Compliance**: STEP AP203/214, ISO 10303, industry B-Rep standards

## 🎯 CRITICAL HARD REQUIREMENTS

### 1. Robust Parametric Construction
- **R1.1**: Every primitive uses analytical parameters (length, radius, angle) - NEVER hard-coded geometry
- **R1.2**: Support live parametric updates: change parameter → instant topology rebuild
- **R1.3**: Parameter validation with meaningful error messages
- **R1.4**: Parameter constraints and dependencies (e.g., cone top_radius ≤ base_radius)
- **R1.5**: Parameter units and conversions (mm, inch, meters)

### 2. Complete Topological Structure (B-Rep)
- **R2.1**: Generate ALL B-Rep entities for every primitive:
  - **Vertices**: Unique IDs, exact 3D coordinates
  - **Edges**: Parametric curves (Line, Circle, Ellipse, Spline)
  - **Faces**: Analytical surfaces (Plane, Cylinder, Sphere, Cone, Torus)
  - **Loops**: Closed edge cycles with orientation
  - **Shells**: Connected face groups
  - **Solids**: Manifold or non-manifold volumes
- **R2.2**: Stable, queryable IDs that persist through parameter changes
- **R2.3**: Complete adjacency information (edge-face, vertex-edge relationships)
- **R2.4**: Euler characteristic validation: V - E + F = 2 for simple solids

### 3. Exact Analytical Geometry
- **R3.1**: NO mesh approximations - only exact surfaces and curves
- **R3.2**: Analytical representations:
  - **Planes**: Point + Normal vector
  - **Cylinders**: Axis + Radius + Height
  - **Spheres**: Center + Radius
  - **Cones**: Axis + Base Radius + Top Radius + Height
  - **Torus**: Major/Minor radius + Axis
- **R3.3**: Parametric curve definitions (t-parameter domain)
- **R3.4**: Surface parameter domains (u,v coordinates)
- **R3.5**: Precision: IEEE 754 double precision minimum

### 4. Manifold and Non-Manifold Support
- **R4.1**: Generate watertight manifold solids by default
- **R4.2**: Support non-manifold entities for advanced modeling
- **R4.3**: Detect and report topology issues (open edges, non-manifold vertices)
- **R4.4**: Healing utilities for fixing topology problems
- **R4.5**: Validation functions for manifold checking

### 5. Orientation and Mathematical Consistency
- **R5.1**: Explicit edge orientation (forward/backward relative to curve)
- **R5.2**: Face orientation (outward/inward normal direction)
- **R5.3**: Consistent winding order (right-hand rule)
- **R5.4**: Surface normal calculations (analytical, not approximated)
- **R5.5**: UV parameter space orientation consistency

### 6. Boolean Operation Compatibility
- **R6.1**: Primitives must support clean Boolean operations
- **R6.2**: Consistent face sharing for identical surfaces
- **R6.3**: Robust intersection handling
- **R6.4**: Tolerance management for numerical stability
- **R6.5**: Self-intersection detection and resolution

### 7. Timeline-Based Modeling and History
- **R7.1**: Every primitive stores creation parameters
- **R7.2**: Timeline-based history tracking (not parametric tree)
- **R7.3**: Sequential operation history for undo/redo
- **R7.4**: Timeline integration with chronological operation order
- **R7.5**: Serialization/deserialization of parameter state and timeline
- **R7.6**: Timeline navigation and branching support

### 8. API Design Standards
- **R8.1**: Deterministic functions (same input → same output)
- **R8.2**: Thread-safe operations (Send + Sync)
- **R8.3**: Comprehensive error handling (Result<T, E> pattern)
- **R8.4**: Zero unsafe code in public APIs
- **R8.5**: Full documentation with examples

### 9. Modularity and Extensibility
- **R9.1**: Plugin architecture for custom primitives
- **R9.2**: Trait-based design for primitive operations
- **R9.3**: Clear extension points with documentation
- **R9.4**: Minimal core dependencies
- **R9.5**: Version compatibility guarantees

### 10. Integration Hooks
- **R10.1**: Boolean engine integration
- **R10.2**: Tessellation system compatibility
- **R10.3**: Export system support (STEP, IGES, STL)
- **R10.4**: Constraint solver interfaces
- **R10.5**: Rendering system integration

## 📐 MANDATORY PRIMITIVE TYPES

### Basic Primitives (MVP)
1. **Box/Cuboid**
   - Parameters: width, height, depth, corner_radius (optional)
   - Variants: Rounded corners, chamfered edges
   - Topology: 8 vertices, 12 edges, 6 faces

2. **Sphere**
   - Parameters: radius, center
   - Variants: Partial sphere (theta/phi ranges)
   - Topology: Exact mathematical sphere surface

3. **Cylinder**
   - Parameters: radius, height, axis direction
   - Variants: Open/closed ends, partial angle
   - Topology: Cylindrical surface + end caps

4. **Cone**
   - Parameters: base_radius, top_radius, height, axis
   - Variants: Truncated, full cone, open/closed
   - Special: Cylinder when base_radius == top_radius

5. **Torus**
   - Parameters: major_radius, minor_radius, axis
   - Variants: Partial torus (angle sweep)
   - Topology: Toroidal surface

### Advanced Primitives (Phase 2)
6. **Ellipsoid**
   - Parameters: a, b, c axis lengths, center, orientation
   - Variants: Sphere when a=b=c

7. **Capsule/Pill**
   - Parameters: radius, height (cylinder + hemisphere ends)
   - Common in mechanical design

8. **Wedge/Prism**
   - Parameters: base polygon, height, taper angle
   - Variants: N-sided regular prisms

9. **Pyramid**
   - Parameters: base shape, height, apex offset
   - Variants: Truncated pyramid (frustum)

10. **Superellipsoid**
    - Parameters: a, b, c, n1, n2 (shape exponents)
    - Generalizes sphere, cylinder, box

### Swept Primitives (Phase 3)
11. **Extrusion**
    - Parameters: profile (2D sketch), direction, distance
    - Variants: Tapered, twisted extrusion

12. **Revolution**
    - Parameters: profile, axis, angle
    - Creates bottles, bowls, turned parts

13. **Loft**
    - Parameters: multiple profiles, guides
    - Creates smooth transitions

14. **Pipe/Sweep**
    - Parameters: path curve, cross-section
    - Variants: Variable cross-section

## 🧪 COMPREHENSIVE TESTING REQUIREMENTS

### Parameter Validation Tests
- **T1.1**: Valid parameter ranges
- **T1.2**: Invalid parameters (negative, zero, infinite)
- **T1.3**: Boundary conditions (very small/large values)
- **T1.4**: Parameter relationships and constraints

### Topology Tests
- **T2.1**: Euler characteristic validation
- **T2.2**: Manifold/non-manifold detection
- **T2.3**: Orientation consistency
- **T2.4**: Adjacency correctness

### Geometry Tests
- **T3.1**: Surface/curve evaluation accuracy
- **T3.2**: Normal vector calculations
- **T3.3**: Parameter space consistency
- **T3.4**: Boundary condition handling

### Boolean Operation Tests
- **T4.1**: Primitive-primitive intersections
- **T4.2**: Self-intersection detection
- **T4.3**: Degenerate case handling
- **T4.4**: Tolerance sensitivity

### Performance Tests
- **T5.1**: Creation time benchmarks
- **T5.2**: Memory usage profiling
- **T5.3**: Parameter update speed
- **T5.4**: Scalability testing

## 📊 PERFORMANCE TARGETS

### Creation Speed
- **Basic primitives**: < 1ms creation time
- **Complex primitives**: < 10ms creation time
- **Parameter updates**: < 100μs for simple changes

### Memory Efficiency
- **Vertex storage**: < 32 bytes per vertex
- **Edge storage**: < 64 bytes per edge
- **Face storage**: < 128 bytes per face
- **Total overhead**: < 2KB per primitive

### Accuracy
- **Geometric tolerance**: 1e-10 absolute
- **Angular tolerance**: 1e-12 radians
- **Parametric tolerance**: 1e-12 in parameter space

## 🔧 IMPLEMENTATION ARCHITECTURE

### Core Traits
```rust
pub trait Primitive: Send + Sync {
    type Parameters: Clone + Serialize + Deserialize;
    
    fn create(params: Self::Parameters) -> Result<Self, PrimitiveError>;
    fn update_parameters(&mut self, params: Self::Parameters) -> Result<(), PrimitiveError>;
    fn get_parameters(&self) -> &Self::Parameters;
    fn get_topology(&self) -> &BRepTopology;
    fn validate(&self) -> Result<(), TopologyError>;
}

pub trait ParametricSurface {
    fn evaluate(&self, u: f64, v: f64) -> Point3;
    fn normal(&self, u: f64, v: f64) -> Vector3;
    fn bounds(&self) -> (f64, f64, f64, f64); // u_min, u_max, v_min, v_max
}

pub trait ParametricCurve {
    fn evaluate(&self, t: f64) -> Point3;
    fn derivative(&self, t: f64) -> Vector3;
    fn bounds(&self) -> (f64, f64); // t_min, t_max
}
```

### Extension Points
- **Custom Primitives**: Implement `Primitive` trait
- **Custom Surfaces**: Implement `ParametricSurface` trait
- **Custom Curves**: Implement `ParametricCurve` trait
- **Validation Rules**: Extend `Validator` trait
- **Export Formats**: Implement `Exporter` trait

## 🎯 SUCCESS CRITERIA

### Functional Requirements ✅
- All 14 primitive types implemented and tested
- Complete B-Rep topology generation
- Parametric update capability
- Boolean operation compatibility

### Quality Requirements ✅
- Zero unsafe code in public API
- 95%+ test coverage
- Performance targets met
- Full documentation

### Integration Requirements ✅
- Boolean engine compatibility
- Export system integration
- Rendering system hooks
- Constraint solver interfaces

## 📋 DEVELOPMENT PHASES

### Phase 1: Foundation (Current)
- ✅ Basic primitive framework
- ✅ Box, Sphere, Cylinder working
- 🚧 Parameter validation system
- 🚧 Complete topology generation

### Phase 2: Core Primitives
- ⏳ Cone and Torus implementation
- ⏳ Advanced parameter handling
- ⏳ Boolean operation support
- ⏳ Comprehensive testing

### Phase 3: Advanced Features
- ⏳ Swept primitives (extrude, revolve)
- ⏳ Complex primitives (ellipsoid, superellipsoid)
- ⏳ Performance optimization
- ⏳ Export integration

### Phase 4: Production Ready
- ⏳ Industrial validation
- ⏳ Performance certification
- ⏳ Documentation completion
- ⏳ API stabilization

---

**This document defines the complete requirements for a world-class parametric CAD primitive module that can compete with industry-leading CAD kernels.**