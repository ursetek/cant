//! Query iteration: read, write, mixed.

#![allow(missing_docs)]

mod common;

use std::hint::black_box;

use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, criterion_group,
    criterion_main,
};

use common::{
    Position, Velocity, prepare_read_positions, prepare_write_positions,
    world_with_motion,
};

fn bench_read_positions(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_read_positions");
    for &n in &[1_000u32, 10_000, 100_000] {
        let world = world_with_motion(n);
        let mut query = prepare_read_positions(&world);
        group.throughput(Throughput::Elements(u64::from(n)));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let mut sum = 0.0f32;
                query.for_each(&world, |view| {
                    let ps = view.column::<Position>(0).unwrap();
                    for p in ps {
                        sum += p.0;
                    }
                });
                black_box(sum)
            });
        });
    }
    group.finish();
}

fn bench_write_positions(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_write_positions");
    for &n in &[1_000u32, 10_000, 100_000] {
        group.throughput(Throughput::Elements(u64::from(n)));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched_ref(
                || world_with_motion(n),
                |world| {
                    let mut query = prepare_write_positions(world);
                    query.for_each_mut(world, |mut view| {
                        let ps = view.column_mut::<Position>(0).unwrap();
                        for p in ps {
                            p.0 += 1.0;
                        }
                    });
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_read_pair(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_read_position_velocity");
    for &n in &[1_000u32, 10_000, 100_000] {
        let world = world_with_motion(n);
        let pos = world.component_id::<Position>().unwrap();
        let vel = world.component_id::<Velocity>().unwrap();
        let mut mask = cant::QueryMask::new();
        mask.read(pos).read(vel);
        let mut query = world.prepare(mask).unwrap();

        group.throughput(Throughput::Elements(u64::from(n)));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let mut sum = 0.0f32;
                query.for_each(&world, |view| {
                    let ps = view.column::<Position>(0).unwrap();
                    let vs = view.column::<Velocity>(1).unwrap();
                    for (p, v) in ps.iter().zip(vs.iter()) {
                        sum += p.0 + v.1;
                    }
                });
                black_box(sum)
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_read_positions,
    bench_write_positions,
    bench_read_pair
);
criterion_main!(benches);
