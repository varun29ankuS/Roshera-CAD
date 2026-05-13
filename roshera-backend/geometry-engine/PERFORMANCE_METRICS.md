# Roshera CAD — Internal Performance Metrics

Last measured: May 13, 2026

These numbers are **internal regression targets** obtained on our own bench
machine. Math/quaternion/B-spline figures come from `cargo bench`; the
tessellation table is from `cargo run --release -p geometry-engine
--example tess_quick` (50-iteration averaged wall-clock, warm cache). They
are **not** claims against any third-party CAD kernel. If you want to
compare, run the benches on the same hardware against your own baseline.

## Measured (internal)

### Tessellation per primitive (release, warm cache, May 13 2026)

50-iter average; reports per-call wall-clock plus mesh size at each REST
quality preset. `cargo run --release -p geometry-engine --example tess_quick`.

| Shape              | Coarse                | Default               | Fine                  |
|--------------------|-----------------------|-----------------------|-----------------------|
| Box 10×10×10       | 11.7 µs (12 tri)      | 10.5 µs (12 tri)      | 10.0 µs (12 tri)      |
| Sphere r=5         | 641 µs (760 tri)      | 15.6 ms (19 800 tri)  | 49.2 ms (79 600 tri)  |
| Cylinder r=2,h=5   | 219 µs (436 tri)      | 10.0 ms (10 196 tri)  | 38.9 ms (80 396 tri)  |
| Cone r=2,h=5       | 192 µs (398 tri)      | 14.8 ms (9 998 tri)   | 53.6 ms (79 998 tri)  |
| Torus R=5,r=1      | 314 µs (400 tri)      | 8.3 ms (10 000 tri)   | 27.3 ms (40 000 tri)  |

Per-triangle cost at default quality (the path the REST/WS surface uses
unless the client requests otherwise): ~0.8–1.5 µs/triangle across curved
primitives. Box is curvature-flat and stays at ~10 µs regardless of
preset. Internal regression budget of 250 ms / 1M triangles is met by a
factor of ~3×.

### Vector3
| Operation       | Measured (ns) | Rate (ops/sec)  |
|-----------------|---------------|-----------------|
| Dot product     | ~0.5          | ~2.1 × 10⁹      |
| Cross product   | ~1.2          | ~833M           |
| Normalize       | ~2.1          | ~476M           |
| Magnitude       | ~1.5          | ~667M           |
| Addition        | ~0.3          | ~3.3 × 10⁹      |

### Matrix4
| Operation          | Measured (ns) | Rate (ops/sec) |
|--------------------|---------------|----------------|
| Multiplication     | <1            | >1 × 10⁹       |
| Transform point    | ~6.2          | ~161M          |
| Transform vector   | ~7.8          | ~128M          |
| Determinant        | ~12.3         | ~81M           |

### B-Spline / NURBS
| Operation               | Measured       | Rate          |
|-------------------------|----------------|---------------|
| B-Spline evaluation     | ~16.2 ns       | ~61.8M ops/s  |
| Derivative              | ~185 ns        | ~5.4M ops/s   |
| Batch (10K points)      | ~0.162 ms      | ~61.8M ops/s  |

### Quaternion
| Operation              | Measured (ns) | Rate (ops/sec) |
|------------------------|---------------|----------------|
| Multiply               | ~4.1          | ~244M          |
| Vector rotation        | ~7.8          | ~128M          |
| SLERP                  | ~15.2         | ~66M           |

## Quality

- Test coverage: 43/43 passing at time of measurement
- Default geometric tolerance: 1e-10
- No `unsafe` code in the math module
- Thread-safe operations throughout

## Technical notes

- SIMD-friendly layouts (4-wide where applicable)
- Cache-aligned data structures
- Inline hints on hot paths
- Arena / reusable scratch buffers in loops
- Rayon for multi-threaded tessellation

## How to reproduce

```bash
cargo bench --workspace
```

Numbers vary with CPU, OS, compiler version, and thermal state. Treat the
table above as a target band, not an absolute guarantee.
