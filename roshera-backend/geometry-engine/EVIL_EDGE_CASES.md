# Evil Edge Cases in CAD Topology

## Overview

This document describes the "evil edge cases" that trip up most CAD engines at scale. These are the pathological cases that go beyond the happy path and test the robustness of topology handling, concurrent modifications, and degenerate geometry.

## Why These Tests Matter

Most CAD systems work well with:
- Well-formed geometry (manifold, watertight)
- Sequential operations (single-threaded)
- Valid inputs (non-degenerate)
- Clean topology (no self-intersections)

But real-world usage includes:
- Degenerate geometry from boolean operations
- Concurrent modifications from multiple users/AI agents
- Invalid inputs from external files
- Pathological cases from complex operations

## Evil Edge Cases Implemented

### 1. Vertex Deletion Cascading (`test_vertex_deletion_cascading`)

**What It Tests**: Deleting a vertex that's referenced by edges and faces.

**Why It's Evil**: 
- Tests referential integrity through cascading deletes
- Challenges topology consistency maintenance
- Exposes dangling reference bugs
- Tests cleanup of spatial indices

**Expected Behavior**: 
- Edges referencing the vertex should be removed
- Faces using those edges should be removed
- No dangling references should remain
- Topology should remain valid

**Real-World Scenario**: User deletes a corner vertex of a complex model.

### 2. Edge Modification Topology Consistency (`test_edge_modification_topology_consistency`)

**What It Tests**: Modifying edge parameters while maintaining topology.

**Why It's Evil**:
- Tests whether modifications preserve connectivity
- Challenges face boundary validity
- Tests loop traversal after changes
- Exposes cache invalidation issues

**Expected Behavior**:
- Edge remains connected to same vertices
- Face loops remain traversable
- Parameter changes don't break topology
- Cached data is properly invalidated

**Real-World Scenario**: Adjusting edge curves during design iteration.

### 3. Zero-Area Face (`test_zero_area_face`)

**What It Tests**: Creating a face with three collinear vertices.

**Why It's Evil**:
- Zero area breaks many geometric algorithms
- Normal vector computation becomes undefined
- Area-based calculations (mass properties) fail
- Boolean operations produce degenerate results

**Expected Behavior**:
- System shouldn't crash
- Area computation returns 0 or near-0
- Validation should flag as degenerate
- Operations should handle gracefully

**Real-World Scenario**: Boolean operations producing sliver faces.

### 4. Zero-Length Edge (`test_zero_length_edge`)

**What It Tests**: Creating an edge where start vertex equals end vertex.

**Why It's Evil**:
- Self-loops break traversal algorithms
- Tangent vector computation fails
- Length-based parameterization undefined
- Tessellation algorithms crash

**Expected Behavior**:
- Edge creation succeeds (may be needed temporarily)
- Length computation returns 0
- Traversal algorithms handle self-loops
- Validation flags as degenerate

**Real-World Scenario**: Intermediate state during complex operations.

### 5. Self-Intersecting Loop (`test_self_intersecting_loop`)

**What It Tests**: Creating a figure-8 loop that intersects itself.

**Why It's Evil**:
- Breaks inside/outside classification
- Winding number computation fails
- Area calculation becomes ambiguous
- Tessellation produces inverted triangles

**Expected Behavior**:
- Loop creation succeeds (for flexibility)
- Validation detects self-intersection
- Boolean operations handle gracefully
- Rendering shows the intersection

**Real-World Scenario**: Sketch with crossing paths before cleanup.

### 6. Non-Manifold Vertex (`test_non_manifold_vertex`)

**What It Tests**: Two faces touching at a single vertex (bowtie configuration).

**Why It's Evil**:
- Breaks manifold assumptions
- Neighborhood queries become ambiguous
- Shell algorithms fail
- Export to some formats impossible

**Expected Behavior**:
- Topology creation succeeds
- Non-manifold detection works
- Shell building handles correctly
- Appropriate error on manifold-only operations

**Real-World Scenario**: Complex assemblies with touching parts.

## Additional Evil Cases to Implement

### Concurrent Mutation Stress

```rust
#[test]
fn test_concurrent_topology_mutations() {
    // Multiple threads simultaneously:
    // - Adding/removing vertices
    // - Modifying edges
    // - Creating/deleting faces
    // - Running boolean operations
}
```

### Topology Corruption Recovery

```rust
#[test]
fn test_topology_self_healing() {
    // Intentionally corrupt topology
    // Test automatic repair mechanisms
    // Verify healed topology is valid
}
```

### Extreme Scale

```rust
#[test]
fn test_million_face_boolean() {
    // Boolean operation on models with 1M+ faces
    // Tests memory efficiency
    // Tests algorithmic complexity
}
```

### Numerical Degeneracy

```rust
#[test]
fn test_near_coincident_geometry() {
    // Vertices separated by < tolerance
    // Nearly parallel faces
    // Almost tangent surfaces
}
```

## Why Roshera Handles These Better

### 1. DashMap Architecture
- Thread-safe concurrent modifications
- No global locks during topology changes
- Each operation is atomic

### 2. Tolerance-Based System
- Robust handling of near-degenerate cases
- Configurable tolerance for different scenarios
- Numerical stability built-in

### 3. Event-Based Timeline
- Operations can be rolled back
- Intermediate invalid states are temporary
- History tracking for debugging

### 4. Comprehensive Validation
- Multi-level validation (quick/standard/deep)
- Self-healing capabilities
- Clear error reporting

## Testing Philosophy

**"If it can go wrong, test it"**

1. **Assume Malicious Input**: Test as if users are trying to break the system
2. **Stress Concurrency**: Real systems have multiple users/AI agents
3. **Embrace Degeneracy**: Real geometry is messy
4. **Test at Scale**: Performance cliffs hide at large scales
5. **Validate Everything**: Never trust, always verify

## Metrics for Success

A robust CAD kernel should:
- Handle all evil cases without crashing
- Degrade gracefully (slow > crash)
- Report issues clearly
- Recover when possible
- Maintain data integrity

## Integration with CI/CD

These evil edge case tests should:
- Run on every PR
- Have performance benchmarks
- Track regression over time
- Generate coverage reports
- Alert on new failures

## Conclusion

Evil edge cases separate toy CAD systems from production-ready kernels. By explicitly testing these pathological cases, Roshera ensures robustness that matches or exceeds commercial CAD systems.

Remember: **Users will find every edge case, usually in production, usually on deadline.**