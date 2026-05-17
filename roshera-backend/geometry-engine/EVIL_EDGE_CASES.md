# Evil Edge Cases in CAD Topology

The "happy path" — well-formed, manifold, watertight geometry from a
single thread of execution — is the easy 5%. The other 95% is what
actually trips kernels: degenerate output from booleans, concurrent
mutation from multiple agents, hostile inputs from imported files,
pathological configurations from sloppy CAM workflows.

This file lists the cases we explicitly test (or want to test), why
each one breaks naive implementations, and where the test lives.

## Implemented cases

### 1. Vertex deletion cascading (`test_vertex_deletion_cascading`)

Delete a vertex that is referenced by edges and faces.

- Hits referential integrity through the cascade.
- Stresses topology consistency maintenance and dangling references.
- Stresses spatial index cleanup.

Expected: edges using the vertex are removed, faces that depend on
those edges are removed, no dangling references remain, the topology
stays valid.

Real scenario: user deletes a corner vertex of a complex model.

### 2. Edge modification topology consistency (`test_edge_modification_topology_consistency`)

Modify an edge's parameters while keeping topology coherent.

- Tests whether the change preserves vertex connectivity.
- Stresses face boundary validity.
- Stresses loop traversal after the edit.
- Stresses cache invalidation.

Expected: edge endpoints unchanged, face loops still traversable, the
cached length/parameter data is invalidated.

Real scenario: tweaking a curve during design iteration.

### 3. Zero-area face (`test_zero_area_face`)

Three collinear vertices form a face.

- Normal vector goes undefined (0/0).
- Boolean classification breaks.
- Tessellation produces no output or inverted output.
- Mass properties divide by zero.

Expected: creation succeeds (sliver faces appear as transient state
in real workflows), area returns ≈ 0, validation flags it as
degenerate, operations downstream skip rather than crash.

Real scenario: sliver faces from boolean intersection on near-coplanar
inputs.

### 4. Zero-length edge (`test_zero_length_edge`)

Edge where the start vertex equals the end vertex.

- Self-loops break edge traversal.
- Tangent at t=0 is undefined.
- Arc-length parameterization breaks.

Expected: edge creation succeeds (intermediate state during edits),
length returns 0, traversal terminates instead of looping forever,
validation flags it.

Real scenario: intermediate state during an operation that hasn't
fully resolved.

### 5. Self-intersecting loop (`test_self_intersecting_loop`)

A figure-8 loop crosses itself.

- Inside/outside classification stops being well-defined.
- Winding number is ambiguous.
- Tessellation flips triangles across the crossing.

Expected: creation succeeds, validation detects the self-intersection,
boolean operations either refuse or produce a documented result.

Real scenario: sketch entities crossing each other before the user
cleans them up.

### 6. Non-manifold vertex (`test_non_manifold_vertex`)

Two faces meeting at exactly one shared vertex (bowtie).

- Manifold neighbourhood queries become ambiguous.
- Shell-closure algorithms can't decide which side is interior.
- Some export formats reject the input.

Expected: creation succeeds, non-manifold detection identifies it,
shell building handles it, operations that require manifold input
return a typed error.

Real scenario: assembly contact points or imported geometry from a
mesh source.

## Cases queued but not yet implemented

The following tests are useful but not yet in the suite. Listed here so
nobody re-discovers them as gaps.

### Concurrent topology mutation

Multiple threads simultaneously adding/removing vertices, modifying
edges, creating/deleting faces, running booleans. Validates that the
DashMap-based topology stores stay coherent under contention.

### Topology corruption recovery

Intentionally corrupt the topology (dangling references, broken loops)
and exercise the validator's self-healing path.

### Extreme scale

Boolean operation on inputs with 1M+ faces. Hits memory efficiency and
algorithmic complexity in the broad-phase pruning.

### Numerical degeneracy

Vertices separated by less than tolerance; nearly parallel faces;
almost-tangent surfaces. The classic numerical-robustness sweep.

## How the kernel mitigates these

- DashMap-based topology stores: no global lock during mutation, each
  store operation is internally atomic.
- Tolerance-based comparisons throughout: no hardcoded `1e-12`
  literals at call sites, callers thread `Tolerance` in.
- Event-sourced timeline: intermediate invalid states are transient
  and rollback is structural rather than ad-hoc.
- Multi-level validation: a quick check on every operation, a deep
  check on demand.

## Testing philosophy

Users find every edge case, usually in production, usually on a
deadline. Tests assume malicious or sloppy input, stress concurrency,
and run at realistic scale. A kernel that crashes is worse than one
that runs slowly.
