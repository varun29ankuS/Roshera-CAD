# Evil Edge Case Tests

These tests go beyond the happy path. Each one stresses a specific
weakness in typical B-Rep implementations: a known place where naive
code crashes, corrupts the model, or silently produces garbage.

## Categories

### 1. Degenerate geometry

Mathematically valid inputs that are geometrically degenerate.

#### `test_zero_area_face`

- Triangle from three collinear points.
- Breaks normal-vector computation (0/0), boolean classification,
  tessellation, and area-weighted properties.
- Roshera: creates successfully, validation flags it as degenerate.

#### `test_zero_length_edge`

- Edge whose start vertex equals its end vertex.
- Breaks edge traversal, loop orientation, parametric evaluation
  (t=0 and t=1 are the same point).
- Roshera: creation succeeds; length returns 0.

### 2. Pathological topology

Topologically valid but problematic configurations.

#### `test_self_intersecting_loop`

- Figure-8 loop that crosses itself.
- Breaks inside/outside classification, face orientation, boolean
  operations, winding-number calculations.
- Roshera: creation succeeds; the self-intersection validator is the
  next layer up (intentionally not on the constructor).

#### `test_non_manifold_vertex`

- Two triangles touching at a single shared vertex (bowtie).
- Breaks shell-closure algorithms, vertex-normal calculation, and
  some mesh-export formats.
- Roshera: handles non-manifold topology; manifold-only operations
  return a typed error.

### 3. Mutation operations

Tests that modify existing topology, not just create.

#### `test_vertex_deletion_cascading`

- Delete a vertex and verify cascading cleanup.
- Incomplete cleanup leaves dangling edge references, invalid face
  boundaries, and inconsistent topology.
- Roshera: referential integrity through the cascade is enforced.

#### `test_edge_modification_topology_consistency`

- Modify an edge's parameters while keeping topology consistent.
- Parameter changes can break loop closure, invalidate face
  boundaries, corrupt traversal order, or open gaps in topology.
- Roshera: consistency is maintained; cached length/parameter data is
  invalidated.

## Why these tests matter

Every kernel that has shipped to production has accumulated patches
for cases like these. Writing them down explicitly and testing them
from day one is the alternative to discovering each one the hard way.

The actual mitigation surface in the kernel:

1. DashMap-based topology stores: concurrent mutation without a
   global lock.
2. Explicit handling: every operation has a documented behaviour for
   degenerate inputs, including "rejected with a typed error".
3. Clean errors: typed `OperationError` variants, never a silent
   `None` or panic.
4. Documented limits: when an operation can't handle a case, the
   docs say so.

## Implementation patterns

### Creating degenerate geometry

```rust
// Zero-area face — three collinear points
let v1 = Point3::new(0.0, 0.0, 0.0);
let v2 = Point3::new(1.0, 0.0, 0.0);
let v3 = Point3::new(2.0, 0.0, 0.0);  // collinear

// Zero-length edge — same start and end
let edge = Edge::new(0, vertex_id, vertex_id, curve_id,
                     EdgeOrientation::Forward, ParameterRange::unit());
```

### Non-manifold configurations

```rust
// Bowtie — two triangles share exactly one vertex, no shared edge.
let center = model.vertices.add_or_find(0.0, 0.0, 0.0, tolerance);
// Triangle 1 uses `center`.
// Triangle 2 uses `center`.
// No shared edges → non-manifold vertex.
```

### Mutation tests

```rust
// Cascading deletion
model.vertices.remove(vertex_id);
// Assert: every edge that referenced vertex_id is gone.
// Assert: every face that used those edges is invalid or gone.
// Assert: no dangling references remain in the spatial index.
```

## Future tests (not yet in the suite)

### Concurrency stress

- 1000 threads creating and deleting topology in parallel.
- Race conditions in DashMap shard rebalances.
- Deadlock scenarios involving the recorder MPSC channel.

### Numerical degeneracy

- Near-zero-length edges (1e-15).
- Near-parallel faces (angle < 1e-10).
- Near-coincident vertices straddling tolerance.

### Topological complexity

- Genus-1000 surfaces.
- 100k-face shells.
- Deeply nested void shells.

### Boolean stress

- Coplanar face intersections.
- Tangent surface contacts.
- Coincident edge overlaps.

## Validation pattern

Each test follows the same shape:

1. Construct the pathological geometry.
2. Verify the kernel doesn't crash.
3. Run a downstream operation (tessellate, boolean, mass props).
4. Document the behaviour — supported, rejected with typed error, or
   silently degraded (and which).

A kernel that crashes is worse than one that runs slowly. A kernel
that silently produces wrong output is worse than one that returns
an error. The tests enforce that ordering.
