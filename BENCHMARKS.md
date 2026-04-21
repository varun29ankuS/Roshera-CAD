# Roshera Geometry Kernel Benchmarks

Measured on Windows 11, x86_64 (Ryzen), release build, LTO disabled to work
around a rustc-LLVM OOM on this host.

All internal targets below are Roshera's own regression budgets — not
comparisons against any third-party kernel.

## Math microbenchmarks (Criterion, median of 100 samples)

| Operation            | Time      |
|----------------------|-----------|
| Vector3 dot          |   500 ps  |
| Vector3 cross        |   884 ps  |
| Vector3 normalize    |  1.68 ns  |
| Vector3 add          |   984 ps  |
| Matrix4 multiply     |  5.14 ns  |
| Matrix4 inverse      |  29.4 ns  |
| Matrix4 transpose    |  5.50 ns  |
| Point3 distance      |   505 ps  |
| Point3 translate     |   868 ps  |

## Primitive creation (Criterion, full B-Rep topology)

Each iteration builds a fresh `BRepModel` + topology.

| Primitive | Time  |
|-----------|-------|
| Box       | 65 µs |
| Sphere    | 49 µs |
| Cylinder  | 50 µs |

## Boolean + intersection (internal suite, 1k faces)

| Operation               | Measured | Target  | Status |
|-------------------------|----------|---------|--------|
| Boolean union           |  50.5 ms | <100 ms | PASS   |
| Boolean intersection    |  75.4 ms | <150 ms | PASS   |
| Face–face intersection  |  25.3 ms |  <50 ms | PASS   |

## NURBS / B-spline evaluation (1M points)

| Operation           | Measured | Target  | Status     |
|---------------------|----------|---------|------------|
| NURBS surface eval  | 158.6 ms |  <25 ms | REGRESSION |
| B-spline curve eval |  36.8 ms |  <10 ms | REGRESSION |

## Tessellation + memory

| Metric                             | Measured   | Target   | Status     |
|------------------------------------|------------|----------|------------|
| Tessellation (1M triangles, est.)  | 5,350 ms   | <250 ms  | REGRESSION |
| Memory per 1M vertices (SoA)       |    34.3 MB | <192 MB  | PASS       |

## Coverage gaps

- `delete_solid` / `delete_face` — correctness tests only, no timing benchmark.
- Sketch2D creation — 69 passing correctness tests, no Criterion target.
- Cone / Torus primitive creation — missing from the Criterion bench harness.

## Reproduce

```bash
cd roshera-backend

# Math + primitive creation (Criterion)
CARGO_PROFILE_BENCH_LTO=off CARGO_PROFILE_BENCH_CODEGEN_UNITS=16 \
  cargo bench -p geometry-engine --bench geometry_bench

# Boolean / NURBS / tessellation (internal suite)
CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
  cargo test --release -p geometry-engine --lib \
    test_performance_benchmark_suite -- --nocapture
```
