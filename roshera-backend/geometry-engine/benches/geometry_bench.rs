use criterion::{black_box, criterion_group, criterion_main, Criterion};
use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::primitives::topology_builder::TopologyBuilder;
use geometry_engine::BRepModel;

fn bench_vector_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_operations");

    let v1 = Vector3::new(1.234, 5.678, 9.012);
    let v2 = Vector3::new(3.456, 7.890, 1.234);

    group.bench_function("dot_product", |b| b.iter(|| black_box(v1.dot(&v2))));

    group.bench_function("cross_product", |b| b.iter(|| black_box(v1.cross(&v2))));

    group.bench_function("normalize", |b| b.iter(|| black_box(v1.normalize())));

    group.bench_function("addition", |b| b.iter(|| black_box(v1 + v2)));

    group.finish();
}

fn bench_matrix_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("matrix_operations");

    let m1 = Matrix4::identity();
    let m2 = Matrix4::from_translation(&Vector3::new(1.0, 2.0, 3.0));

    group.bench_function("matrix_multiply", |b| b.iter(|| black_box(m1 * m2)));

    group.bench_function("matrix_inverse", |b| b.iter(|| black_box(m2.inverse())));

    group.bench_function("matrix_transpose", |b| b.iter(|| black_box(m2.transpose())));

    group.finish();
}

fn bench_primitive_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("primitive_creation");

    group.bench_function("create_box", |b| {
        b.iter(|| {
            let mut model = BRepModel::new();
            let mut builder = TopologyBuilder::new(&mut model);
            builder.create_box_3d(black_box(5.0), black_box(3.0), black_box(2.0))
        })
    });

    group.bench_function("create_sphere", |b| {
        b.iter(|| {
            let mut model = BRepModel::new();
            let mut builder = TopologyBuilder::new(&mut model);
            builder.create_sphere_3d(black_box(Point3::new(0.0, 0.0, 0.0)), black_box(5.0))
        })
    });

    group.bench_function("create_cylinder", |b| {
        b.iter(|| {
            let mut model = BRepModel::new();
            let mut builder = TopologyBuilder::new(&mut model);
            builder.create_cylinder_3d(
                black_box(Point3::new(0.0, 0.0, 0.0)),
                black_box(Vector3::new(0.0, 0.0, 1.0)),
                black_box(2.0),
                black_box(5.0),
            )
        })
    });

    group.finish();
}

fn bench_point_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("point_operations");

    let p1 = Point3::new(1.0, 2.0, 3.0);
    let p2 = Point3::new(4.0, 5.0, 6.0);
    let v = Vector3::new(1.0, 0.0, 0.0);

    group.bench_function("point_distance", |b| b.iter(|| black_box(p1.distance(&p2))));

    group.bench_function("point_translation", |b| b.iter(|| black_box(p1 + v)));

    group.finish();
}

criterion_group!(
    benches,
    bench_vector_operations,
    bench_matrix_operations,
    bench_primitive_creation,
    bench_point_operations
);
criterion_main!(benches);
