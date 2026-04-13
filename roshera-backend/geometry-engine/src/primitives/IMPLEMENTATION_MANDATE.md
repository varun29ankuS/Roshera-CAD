# IMPLEMENTATION MANDATE - PRIMITIVES MODULE

**CRITICAL**: This document defines MANDATORY implementation standards for the Roshera CAD primitives module. Any AI or developer working on this module MUST follow these guidelines without exception.

## 🚨 CRITICAL UPDATE - JULY 28, 2025

### BEFORE MAKING ANY CHANGES - MANDATORY PROTOCOL

**THIS IS NOT OPTIONAL - EVERY CHANGE MUST FOLLOW THIS PROTOCOL:**

1. **STOP - READ ALL DOCUMENTATION**
   - Read CLAUDE.md files (root and backend)
   - Read IMPLEMENTATION_MANDATE.md completely
   - Read relevant module documentation
   - Read code comments and understand existing patterns

2. **UNDERSTAND THE CONTEXT**
   - What is the current state of the code?
   - What problem led to this task?
   - What are the system dependencies?
   - What is the architectural design?

3. **UNDERSTAND THE INTENT**
   - Why is this change needed?
   - What business/technical goal does it serve?
   - What should the end result achieve?
   - How will this be used by others?

4. **UNDERSTAND THE IMPACT**
   - What could this change break?
   - What depends on this code?
   - What are the performance implications?
   - What edge cases must be handled?

5. **DOCUMENT YOUR UNDERSTANDING**
   - Before coding, explain your understanding
   - State: "Context: [what you found], Intent: [why needed], Impact: [what changes]"
   - Get confirmation before proceeding

6. **ASK IF UNCLEAR**
   - If ANY aspect is unclear, ASK
   - Don't assume or guess intent
   - It's better to ask than to implement wrong

**EXAMPLE:**
```
"I need to implement surface-surface intersection.

Context: The intersect() method in surface.rs returns empty vectors. This is 
preventing boolean operations from working as they need intersection curves.

Intent: Implement marching algorithm to find actual intersection curves between 
surfaces. This will enable boolean operations to split faces correctly.

Impact: This will make boolean operations functional. Will need ~200 lines of 
code for marching algorithm. No breaking changes to existing API.

Is my understanding correct?"
```

### RECENT ARCHITECTURAL DECISIONS
1. **TIMELINE NOT PARAMETRIC TREE**: We use timeline-based event sourcing, NOT parametric trees
2. **DASHMAP EVERYWHERE**: Use DashMap instead of HashMap for ALL concurrent data structures
3. **PROPER PRIMITIVE USAGE**: primitive_system.rs must use actual primitive implementations (BoxPrimitive, SpherePrimitive, etc.), not bypass them

### CURRENT STATE (as of July 29, 2025)
- **Compilation Errors**: 97 (down from 263 → 193 → 90)
- **Warnings**: ~600 (mostly unused imports)
- **Major Achievements**: 
  - ✅ ALL Priority 1 implementations complete (surface intersections, NURBS, validation)
  - ✅ Surface-surface intersections implemented (cylinder-plane, sphere-sphere, etc.)
  - ✅ NURBS evaluation verified (~2200 lines of production code)
  - ✅ Topology validation system complete
  - ✅ G2 continuous blending surfaces implemented
  - ✅ Ellipse curve type added (360+ lines)
- **Architecture Fixes**:
  - Fixed primitive_system.rs to use actual primitive implementations
  - Added curvature_at method to Surface trait
  - Fixed GeneralNurbsSurface NURBS integration
  - Implemented DashMap globally for thread-safe access

## 🚨 ABSOLUTE RULES - NO EXCEPTIONS

1. **NO PLACEHOLDERS**: Every function must be fully implemented. No TODOs, no "implement later", no stub returns.
2. **NO UNWRAP()**: Zero unwrap() calls. Use proper Result<T, E> error handling everywhere.
3. **PRODUCTION-GRADE ONLY**: Code must be ready for aerospace/automotive CAD applications.
4. **COMPLETE EDGE CASES**: Handle ALL edge cases, not just happy paths.
5. **FULL TESTING**: Every public function needs comprehensive tests including edge cases.

## 📋 IMPLEMENTATION CHECKLIST

### Before Starting ANY Implementation

- [ ] Read `REQUIREMENTS.md` completely
- [ ] Read `TEST_PLAN.md` completely  
- [ ] Review existing math module (DO NOT MODIFY - it's frozen)
- [ ] Understand B-Rep topology hierarchy: Vertex → Edge → Loop → Face → Shell → Solid
- [ ] Check if similar functionality exists before creating new

### For EVERY Function You Implement

```rust
/// Brief description of what this function does
/// 
/// # Arguments
/// * `param1` - What this parameter is and valid ranges
/// * `param2` - What this parameter is and constraints
/// 
/// # Returns
/// What this returns and when
/// 
/// # Errors
/// * `ErrorType::Variant1` - When this error occurs
/// * `ErrorType::Variant2` - When this error occurs
/// 
/// # Edge Cases
/// - Handles zero-length edges by returning ErrorType::DegenerateGeometry
/// - Handles coincident points within tolerance by merging
/// - Handles self-intersections by splitting at intersection points
/// 
/// # Example
/// ```
/// let result = function_name(param1, param2)?;
/// assert_eq!(result.some_property(), expected_value);
/// ```
/// 
/// # Performance
/// O(n) complexity where n is number of edges. Typically 0.1ms for 1000 edges.
/// 
/// # References
/// - Mantyla, M. (1988). "An Introduction to Solid Modeling", pp. 120-125
/// - Patent US5,619,625 - "Method for interpolating smooth surfaces"
pub fn function_name(param1: Type1, param2: Type2) -> Result<ReturnType, PrimitiveError> {
    // COMPLETE IMPLEMENTATION - NO PLACEHOLDERS
}
```

## 🎯 CURRENT IMPLEMENTATION STATUS & PRIORITIES

### ✅ COMPLETED IMPLEMENTATIONS (July 2025)

#### 1. Surface-Surface Intersections ✅ DONE
- ✅ Cylinder-Plane: Returns ellipse/line/point curves
- ✅ Cylinder-Cylinder: Handle parallel, skew, intersecting cases  
- ✅ Sphere-Plane: Returns circle/point
- ✅ Sphere-Sphere: Returns circle/point
- ✅ Surface marching algorithm: Grid sampling, adaptive stepping
- ⚠️ Cone-Plane: Partial (circle case done, need parabola/hyperbola)
- ❌ Torus-Plane: Still TODO (Villarceau circles)

#### 2. Curve Operations ✅ DONE
- ✅ Arc-Arc intersection
- ✅ NURBS-Curve intersection  
- ✅ NURBS-Plane intersection
- ✅ Ellipse curve type (360+ lines)

#### 3. Core Systems ✅ DONE
- ✅ NURBS evaluation (~2200 lines)
- ✅ Topology validation (comprehensive)
- ✅ G2 continuous blending surfaces

### ❌ CRITICAL MISSING IMPLEMENTATIONS (Do These First!)

#### 1. Validation Functions (in `validation.rs`) - PHASE 1 PRIORITY
**Current State**: Many functions return default "valid" results without checking
**Required**: Actual implementation of:
- **Euler characteristic checking**: V - E + F = 2 for simple solids
- **Manifold detection**: Find non-manifold edges/vertices
- **Gap detection**: Find holes in topology
- **Orientation validation**: Check face normal consistency
- **Topology healing**: Fix common issues

#### 2. Remove ALL unwrap() calls - PHASE 1 PRIORITY
**Current State**: Hundreds of unwrap() calls that will panic
**Required**: Replace with proper Result<T, E> error handling
**Priority Files**:
- primitives module: curve.rs, surface.rs, face.rs, edge.rs
- operations module: boolean.rs, extrude.rs, revolve.rs

#### 3. Fix Compilation Errors - PHASE 1 PRIORITY  
**Current State**: 97 compilation errors preventing testing
**Focus Areas**:
- Missing trait implementations
- Type mismatches
- Incorrect method signatures
- Missing imports

#### 4. Missing Primitives (Phase 2)
**Required Files**:
- ✅ `cone_primitive.rs` - Basic done, need full intersections
- ✅ `torus_primitive.rs` - Basic done, need intersections
- ❌ `point_primitive.rs` - 2D/3D points for sketching
- ❌ `line_primitive.rs` - Infinite/ray/segment variants
- ❌ `polyline_primitive.rs` - Open/closed polylines
- ❌ `polygon_primitive.rs` - With hole support

#### 3. 2D Sketching System (Create new module)
**Required Structure**:
```
primitives/
  sketch/
    mod.rs
    sketch_container.rs    // Sketch with plane and constraints
    sketch_point.rs        // 2D constrained points
    sketch_line.rs         // 2D lines with construction support
    sketch_arc.rs          // 2D arcs (3-point, center-radius, tangent)
    sketch_circle.rs       // 2D circles
    sketch_ellipse.rs      // 2D ellipses
    sketch_spline.rs       // 2D splines (interpolated, control point)
    sketch_constraints.rs  // Geometric constraints
    sketch_solver.rs       // Constraint solver
```

### ⚠️ INCOMPLETE IMPLEMENTATIONS (Fix These!)

#### 1. Curve Intersections (in `curve.rs`)
- Arc-Arc intersection: Currently returns empty vector
- NURBS-Curve intersection: Currently returns empty vector
- NURBS-Plane intersection: Currently returns empty vector

#### 2. Surface Conversions (in `surface.rs`)
- `to_bspline()` methods: Currently create placeholders
- Need proper control point calculation
- Need proper knot vector generation

#### 3. Validation Functions (in `validation.rs`)
- Many validation functions return default "valid" results
- Need actual implementation of:
  - Euler characteristic checking
  - Manifold detection
  - Gap detection
  - Orientation validation

## 🔧 IMPLEMENTATION PATTERNS TO FOLLOW

### 1. Error Handling Pattern
```rust
// NEVER DO THIS
let vertex = vertices.get(id).unwrap();

// ALWAYS DO THIS
let vertex = vertices.get(id)
    .ok_or_else(|| PrimitiveError::InvalidVertex { 
        id, 
        reason: "Vertex not found in store".to_string() 
    })?;
```

### 2. Edge Case Handling Pattern
```rust
pub fn create_arc(start: Point3, end: Point3, center: Point3) -> Result<Arc, PrimitiveError> {
    // Check for degenerate cases FIRST
    if start.distance_to(&end) < tolerance.distance() {
        return Err(PrimitiveError::DegenerateGeometry {
            entity: "Arc",
            reason: "Start and end points are coincident"
        });
    }
    
    if start.distance_to(&center) < tolerance.distance() {
        return Err(PrimitiveError::DegenerateGeometry {
            entity: "Arc",
            reason: "Start point coincident with center"
        });
    }
    
    // Check for collinear points (straight line, not arc)
    let v1 = (start - center).normalize();
    let v2 = (end - center).normalize();
    if v1.cross(&v2).magnitude() < tolerance.angle() {
        return Err(PrimitiveError::InvalidGeometry {
            entity: "Arc",
            reason: "Points are collinear - cannot form arc"
        });
    }
    
    // NOW implement the actual arc creation
    // ...
}
```

### 3. Testing Pattern
For EVERY public function, create these tests:
```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_function_normal_case() { /* Happy path */ }
    
    #[test]
    fn test_function_edge_case_zero() { /* Zero/empty input */ }
    
    #[test]
    fn test_function_edge_case_coincident() { /* Coincident geometry */ }
    
    #[test]
    fn test_function_edge_case_degenerate() { /* Degenerate geometry */ }
    
    #[test]
    fn test_function_numerical_limits() { /* Very large/small values */ }
    
    #[test]
    fn test_function_error_cases() { /* All error conditions */ }
}
```

### 4. Performance Considerations
```rust
// Use math module's optimized operations
use crate::math::{Vector3, Matrix4}; // SIMD-optimized

// Pre-allocate collections when size is known
let mut vertices = Vec::with_capacity(expected_count);

// Use iterators instead of collecting when possible
edges.iter()
    .filter(|e| e.is_boundary())
    .map(|e| e.length())
    .sum::<f64>()  // Don't collect unnecessarily
```

## 📐 SPECIFIC IMPLEMENTATION REQUIREMENTS

### Surface-Surface Intersections
Use the marching algorithm pattern from `boolean.rs`:
1. Find initial intersection points via grid sampling
2. March along intersection curves with adaptive step size
3. Handle branches and loops
4. Return proper parametric curves

### Curve-Curve Intersections
1. Try analytical solutions first (line-line, line-circle)
2. Fall back to Newton-Raphson for complex cases
3. Always check parameter bounds [0,1]
4. Handle multiple intersections

### Primitive Creation
Every primitive MUST generate:
1. Complete topology (vertices, edges, faces, etc.)
2. Proper orientation (outward normals)
3. Manifold structure (watertight)
4. Valid parameter ranges

### 2D Sketching
1. All sketch entities exist in 2D parameter space
2. Must support constraint definitions
3. Must integrate with 3D operations (extrude, revolve)
4. Handle over/under-constrained systems

## 🧪 TESTING REQUIREMENTS

### Test Coverage Targets
- Unit Tests: 100% of public functions
- Edge Cases: All identified in this document
- Performance: Benchmarks for critical paths
- Integration: Primitive creation → Operation → Validation

### Required Test Categories (from TEST_PLAN.md)
1. **Unit Tests** (80+ tests)
2. **Integration Tests** (40+ tests)
3. **Stress Tests** (30+ tests)
4. **Edge Case Tests** (30+ tests)
5. **Performance Benchmarks** (20+ tests)

### Performance Benchmarks to Beat
From project requirements - we MUST beat these:
- Boolean Union (1k faces): < 100ms (Industry standard: 200ms)
- Surface Evaluation (1M pts): < 25ms (Industry standard: 50ms)
- Primitive Creation: < 1ms for basic, < 10ms for complex

## 🚀 IMPLEMENTATION PRIORITY ORDER

### Phase 1: Fix Critical Broken Functionality
1. Complete ALL surface-surface intersections in `surface.rs`
2. Complete ALL curve-curve intersections in `curve.rs`
3. Fix ALL validation functions in `validation.rs`
4. Remove ALL unwrap() calls

### Phase 2: Implement Missing Primitives
1. Cone primitive (with apex singularity handling)
2. Torus primitive (with self-intersection support)
3. Point, Line, Polyline, Polygon primitives

### Phase 3: Add 2D Sketching
1. Sketch container and plane management
2. 2D geometric entities
3. Constraint system
4. Sketch-to-3D operations

### Phase 4: Complete Edge Case Handling
1. Degenerate geometry detection
2. Self-intersection handling
3. Numerical stability improvements
4. Topology healing

### Phase 5: Comprehensive Testing
1. Implement all tests from TEST_PLAN.md
2. Add property-based tests
3. Add fuzzing tests
4. Performance optimization

## 📚 REQUIRED REFERENCES

When implementing, cite these sources:
- **Curves/Surfaces**: Piegl & Tiller (1997) "The NURBS Book"
- **Boolean Ops**: Mantyla (1988) "Introduction to Solid Modeling"
- **Intersections**: Patrikalakis & Maekawa (2002) "Shape Interrogation"
- **Topology**: Hoffmann (1989) "Geometric and Solid Modeling"

## ⚡ QUICK REFERENCE - WHAT TO USE FROM MATH MODULE

The math module is FROZEN - use it, don't modify it:

### Available Types
- `Vector2`, `Vector3`, `Vector4` - SIMD-optimized vectors
- `Matrix3`, `Matrix4` - Transformation matrices
- `Quaternion` - Rotations
- `Point3` - 3D points (alias for Vector3)
- `Tolerance` - Precision handling
- `BSplineCurve`, `BSplineSurface` - Spline evaluation
- `NurbsCurve`, `NurbsSurface` - NURBS evaluation

### Available Operations
- Vector operations: dot, cross, normalize, angle
- Matrix operations: multiply, inverse, decompose
- Curve evaluation: evaluate_at(t), derivative_at(t)
- Surface evaluation: evaluate_at(u, v), normal_at(u, v)
- Intersection helpers in `surface_surface_intersection.rs`

### Example Usage
```rust
use crate::math::{Vector3, Point3, Tolerance};

let v1 = Vector3::new(1.0, 0.0, 0.0);
let v2 = Vector3::new(0.0, 1.0, 0.0);
let angle = v1.angle(&v2); // π/2

let tolerance = Tolerance::default(); // 1e-10
if (p1 - p2).magnitude() < tolerance.distance() {
    // Points are coincident
}
```

## 🎯 SUCCESS CRITERIA

Your implementation is complete when:
1. ✅ ZERO unwrap() calls remain
2. ✅ ZERO TODO/FIXME comments remain
3. ✅ ALL functions have complete implementations
4. ✅ ALL edge cases are handled
5. ✅ ALL tests pass (including edge cases)
6. ✅ Performance beats industry-leading targets
7. ✅ Code is documented with examples
8. ✅ Mathematical references are cited

## 🆘 WHEN IN DOUBT

1. Check existing implementations in math module for patterns
2. Look at `boolean.rs` for complex algorithm examples
3. Read the research papers cited in references
4. Handle edge cases explicitly - never ignore them
5. Test with extreme values (very large, very small, zero)
6. Consider numerical stability in all calculations

---

**Remember**: We're building a WORLD-CLASS CAD kernel. No shortcuts, no excuses. Every line of code should be production-ready from day one.