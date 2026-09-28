//! Spawn and despawn throughput.

#![allow(missing_docs)]

mod common;

use std::hint::black_box;

use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, criterion_group,
    criterion_main,
};

use cant::World;
use common::{Position, Velocity};

fn bench_spawn_empty(c: &mut Criterion) {
    let mut group = c.benchmark_group("spawn_empty");
    for &n in &[1_000u32, 10_000, 100_000] {
        group.throughput(Throughput::Elements(u64::from(n)));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let mut world = World::new();
                    world.reserve_entities(n);
                    world
                },
                |mut world| {
                    for _ in 0..n {
                        black_box(world.spawn());
                    }
                    world
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_spawn_bundle(c: &mut Criterion) {
    let mut group = c.benchmark_group("spawn_bundle");
    for &n in &[1_000u32, 10_000, 100_000] {
        group.throughput(Throughput::Elements(u64::from(n)));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let mut world = World::new();
                    let _ = world.register_component::<Position>("Position");
                    let _ = world.register_component::<Velocity>("Velocity");
                    let bid = world.register_bundle::<(Position, Velocity)>();
                    world.reserve_entities(n);
                    let archetype = world.bundle_archetype(bid);
                    world.reserve_archetype(archetype, n);
                    (world, bid)
                },
                |(mut world, bid)| {
                    for i in 0..n {
                        #[allow(clippy::cast_precision_loss)]
                        black_box(world.spawn_bundle_id(
                            bid,
                            (Position(i as f32, 0.0), Velocity(0.0, 1.0)),
                        ));
                    }
                    world
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_despawn_half(c: &mut Criterion) {
    let mut group = c.benchmark_group("despawn_half");
    for &n in &[1_000u32, 10_000, 100_000] {
        group.throughput(Throughput::Elements(u64::from(n / 2)));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let world = common::world_with_motion(n);
                    let entities: Vec<_> = world.entities().collect();
                    (world, entities)
                },
                |(mut world, entities)| {
                    for e in entities.iter().step_by(2) {
                        world.despawn(*e);
                    }
                    black_box(world.live_count())
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_spawn_empty,
    bench_spawn_bundle,
    bench_despawn_half,
);
criterion_main!(benches);
