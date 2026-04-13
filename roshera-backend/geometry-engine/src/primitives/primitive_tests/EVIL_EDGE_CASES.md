# Evil Edge Case Tests - CAD Robustness at Scale

## Overview

These tests go beyond "happy path" topology to expose the pathological cases that crash most CAD engines. Each test is designed to stress a specific weakness in typical B-Rep implementations.

## Test Categories

### 1. Degenerate Geometry
Tests that create mathematically valid but geometrically degenerate entities.

#### test_zero_area_face
- **What**: Triangle with three collinear points
- **Why It's Evil**: Zero-area faces break:
  - Normal vector calculations (0/0 = undefined)
  - Boolean classification algorithms 
  - Tessellation routines
  - Area-weighted computations
- **Roshera's Handling**: ✅ Creates successfully, handles gracefully

#### test_zero_length_edge
- **What**: Edge where start vertex == end vertex
- **Why It's Evil**: Self-loops break:
  - Edge traversal algorithms
  - Loop orientation calculations
  - Parametric evaluation (t=0 and t=1 are same point)
- **Roshera's Handling**: ✅ Allows creation, maintains topology

### 2. Pathological Topology
Tests that create topologically valid but problematic configurations.

#### test_self_intersecting_loop
- **What**: Figure-8 loop that crosses itself
- **Why It's Evil**: Self-intersections break:
  - Inside/outside classification
  - Face orientation algorithms
  - Boolean operations
  - Winding number calculations
- **Roshera's Handling**: ✅ Creates successfully (validation intentionally missing!)

#### test_non_manifold_vertex
- **What**: Two triangles touching at single vertex (bowtie)
- **Why It's Evil**: Non-manifold vertices break:
  - Shell closure algorithms
  - Vertex normal calculations
  - Mesh generation
  - Solid validity checks
- **Roshera's Handling**: ✅ Handles non-manifold topology

### 3. Mutation Operations
Tests that modify existing topology (not just create).

#### test_vertex_deletion_cascading
- **What**: Delete vertex and validate cascade cleanup
- **Why It's Evil**: Incomplete cleanup causes:
  - Dangling edge references
  - Invalid face boundaries
  - Memory corruption
  - Topology inconsistency
- **Roshera's Handling**: ✅ Proper referential integrity

#### test_edge_modification_topology_consistency
- **What**: Modify edge parameters while maintaining topology
- **Why It's Evil**: Parameter changes can:
  - Break loop closure
  - Invalidate face boundaries
  - Corrupt traversal order
  - Create gaps in topology
- **Roshera's Handling**: ✅ Maintains consistency

## Why These Tests Matter

### Industry Context
Most industry-leading CAD kernels have decades of patches for these edge cases. A single unhandled case can:
- Crash boolean operations
- Corrupt entire models
- Create non-watertight geometry
- Break downstream manufacturing

### Roshera's Advantage
By testing these cases from day one:
1. **Robustness by Design** - Not patches on patches
2. **Clean Architecture** - DashMap handles concurrent mutations
3. **Explicit Handling** - Know what we support vs. reject
4. **AI-Ready** - AI agents will create weird geometry

## Implementation Patterns

### Creating Degenerate Geometry
```rust
// Zero-area face - three collinear points
let v1 = Point3::new(0.0, 0.0, 0.0);
let v2 = Point3::new(1.0, 0.0, 0.0);  
let v3 = Point3::new(2.0, 0.0, 0.0);  // Collinear!

// Zero-length edge - same start/end
let edge = Edge::new(0, vertex_id, vertex_id, curve_id, 
                     EdgeOrientation::Forward, ParameterRange::unit());
```

### Testing Non-Manifold Conditions
```rust
// Bowtie configuration - shared vertex only
let center = model.vertices.add_or_find(0.0, 0.0, 0.0, tolerance);
// Triangle 1 uses center
// Triangle 2 uses center  
// No shared edges - non-manifold!
```

### Mutation Testing
```rust
// Test cascading deletion
model.vertices.remove(vertex_id);
// Verify: All edges using vertex are gone
// Verify: All faces using those edges are invalid
// Verify: No dangling references remain
```

## Future Evil Tests

### Concurrency Stress
- 1000 threads creating/deleting simultaneously
- Race conditions in topology updates
- Deadlock scenarios in operations

### Numerical Degeneracy
- Near-zero length edges (1e-15)
- Near-parallel faces (angle < 1e-10)
- Near-coincident vertices

### Topological Complexity
- Genus-1000 surfaces
- 100k face shells
- Deeply nested void shells

### Boolean Nightmares
- Coplanar face intersections
- Tangent surface contacts
- Coincident edge overlaps

## Validation Strategy

Each evil case should:
1. **Create** the pathological geometry
2. **Validate** it's handled gracefully (no crash)
3. **Operate** on it (tessellate, boolean, etc.)
4. **Document** the behavior (supported vs. rejected)

## Key Insight

The difference between a research CAD kernel and a production one is how it handles these evil cases. Roshera's strategy:
- **Explicit handling** over silent failure
- **Clean errors** over undefined behavior  
- **Document limits** over claiming perfection
- **Test everything** over hoping for the best