# Roshera CAD — Internal Performance Metrics

Last measured: July 27, 2025

These numbers are **internal regression targets** obtained on our own bench
machine with `cargo bench`. They are **not** claims against any third-party
CAD kernel. If you want to compare, run the benches on the same hardware
against your own baseline.

## Measured (internal)

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
