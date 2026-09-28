//! Archetype migration: adding and removing a single component.

#![allow(missing_docs)]

mod common;

use std::hint::black_box;

use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, criterion_group,
    criterion_main,
};

use cant::World;
use common::{Position, Velocity};

fn bench_add_component(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_component");
    for &n in &[1_000u32, 10_000, 50_000] {
        group.throughput(Throughput::Elements(u64::from(n)));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let mut world = World::new();
                    let _ = world.register_component::<Position>("Position");
                    let _ = world.register_component::<Velocity>("Velocity");
                    let bid = world.register_bundle::<(Position,)>();
                    world.reserve_entities(n);
                    let arch = world.bundle_archetype(bid);
                    world.reserve_archetype(arch, n);
                    let entities: Vec<_> = (0..n)
                        .map(|i| {
                            #[allow(clippy::cast_precision_loss)]
                            world.spawn_bundle_id(
                                bid,
                                (Position(i as f32, 0.0),),
                            )
                        })
                        .collect();
                    (world, entities)
                },
                |(mut world, entities)| {
                    for e in &entities {
                        black_box(world.add_component(*e, Velocity(0.0, 1.0)));
                    }
                    world
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_remove_component(c: &mut Criterion) {
    let mut group = c.benchmark_group("remove_component");
    for &n in &[1_000u32, 10_000, 50_000] {
        group.throughput(Throughput::Elements(u64::from(n)));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let mut world = World::new();
                    let _ = world.register_component::<Position>("Position");
                    let vel = world.register_component::<Velocity>("Velocity");
                    let bid = world.register_bundle::<(Position, Velocity)>();
                    world.reserve_entities(n);
                    let arch = world.bundle_archetype(bid);
                    world.reserve_archetype(arch, n);
                    let entities: Vec<_> = (0..n)
                        .map(|i| {
                            #[allow(clippy::cast_precision_loss)]
                            world.spawn_bundle_id(
                                bid,
                                (Position(i as f32, 0.0), Velocity(0.0, 1.0)),
                            )
                        })
                        .collect();
                    (world, entities, vel)
                },
                |(mut world, entities, vel)| {
                    for e in &entities {
                        black_box(world.remove_component(*e, vel));
                    }
                    world
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_add_component, bench_remove_component);
criterion_main!(benches);
