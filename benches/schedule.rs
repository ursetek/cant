//! Scheduler throughput: parallel batches of independent systems.

#![allow(missing_docs)]

mod common;

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{
    BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main,
};

use cant::{Access, AccessList, ComponentId, FnSystem, Schedule, World};
use cant::{QueryMask, QuerySystem};

use crate::common::Position;

/// N independent systems, each touching a distinct synthetic component.
/// They all land in one batch and run in parallel.
fn bench_parallel_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("schedule_parallel");
    for &n_systems in &[2u32, 4, 8, 16] {
        group.bench_with_input(
            BenchmarkId::from_parameter(n_systems),
            &n_systems,
            |b, &n_systems| {
                b.iter_batched(
                    || {
                        let counter = Arc::new(AtomicU64::new(0));
                        let mut schedule = Schedule::new();
                        for i in 0..n_systems {
                            let c = Arc::clone(&counter);
                            let mut access = AccessList::new();
                            access.add_component(
                                ComponentId::from_raw(i),
                                Access::Write,
                            );
                            schedule.add_system(FnSystem::new(
                                format!("s{i}"),
                                access,
                                move |_, _| {
                                    c.fetch_add(1, Ordering::Relaxed);
                                },
                            ));
                        }
                        (schedule, World::new(), counter)
                    },
                    |(mut schedule, mut world, counter)| {
                        schedule.run(&mut world);
                        black_box(counter.load(Ordering::Relaxed))
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

/// N systems all writing the same component; they serialize into N batches.
fn bench_sequential_batches(c: &mut Criterion) {
    let mut group = c.benchmark_group("schedule_sequential");
    for &n_systems in &[2u32, 4, 8, 16] {
        group.bench_with_input(
            BenchmarkId::from_parameter(n_systems),
            &n_systems,
            |b, &n_systems| {
                b.iter_batched(
                    || {
                        let counter = Arc::new(AtomicU64::new(0));
                        let mut schedule = Schedule::new();
                        for i in 0..n_systems {
                            let c = Arc::clone(&counter);
                            let mut access = AccessList::new();
                            access.add_component(
                                ComponentId::from_raw(0),
                                Access::Write,
                            );
                            schedule.add_system(FnSystem::new(
                                format!("s{i}"),
                                access,
                                move |_, _| {
                                    c.fetch_add(1, Ordering::Relaxed);
                                },
                            ));
                        }
                        (schedule, World::new(), counter)
                    },
                    |(mut schedule, mut world, counter)| {
                        schedule.run(&mut world);
                        black_box(counter.load(Ordering::Relaxed))
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

/// Schedule rebuild cost as a function of system count.
fn bench_schedule_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("schedule_build");
    for &n_systems in &[8u32, 32, 128] {
        group.bench_with_input(
            BenchmarkId::from_parameter(n_systems),
            &n_systems,
            |b, &n_systems| {
                b.iter_batched(
                    || {
                        let mut schedule = Schedule::new();
                        for i in 0..n_systems {
                            let mut access = AccessList::new();
                            // Spread across 4 distinct components so batching
                            // has actual work to do.
                            access.add_component(
                                ComponentId::from_raw(i % 4),
                                Access::Write,
                            );
                            schedule.add_system(FnSystem::new(
                                format!("s{i}"),
                                access,
                                |_, _| {},
                            ));
                        }
                        schedule
                    },
                    |mut schedule| {
                        schedule.build();
                        black_box(schedule.batch_count())
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

/// N read-only query systems over a shared world.
///
/// Two workloads: `light` (10k entities per system, ~7 µs of work) and
/// `heavy` (500k entities per system, ~350 µs of work). Each workload is
/// run with the hint set to the system's real cost (calibrated below) and
/// with hint zero (falls back to the size threshold).
fn bench_parallel_calibrated(c: &mut Criterion) {
    let mut group = c.benchmark_group("schedule_parallel_calibrated");
    group.sample_size(100);

    // (workload_label, entities, hint_ns)
    let configs: &[(&str, u32, u32)] =
        &[("light", 10_000, 7_000), ("heavy", 500_000, 350_000)];

    for &(label, entities, hint_ns) in configs {
        for &n_systems in &[2u32, 4, 8] {
            for hint in [0u32, hint_ns] {
                let h_label = if hint == 0 { "nohint" } else { "hint" };
                let id = format!("{label}_n{n_systems}_{h_label}");

                group.bench_with_input(
                    BenchmarkId::from_parameter(&id),
                    &n_systems,
                    |b, &n_systems| {
                        b.iter_batched(
                            || {
                                let world = common::world_with_motion(entities);
                                let pos =
                                    world.component_id::<Position>().unwrap();
                                let mut schedule = Schedule::new();
                                for _ in 0..n_systems {
                                    let mut mask = QueryMask::new();
                                    mask.read(pos);
                                    let query = world.prepare(mask).unwrap();
                                    let sys = QuerySystem::new(
                                        "read",
                                        query,
                                        |view| {
                                            let ps = view
                                                .column::<Position>(0)
                                                .unwrap();
                                            let mut s = 0.0f32;
                                            for p in ps {
                                                s += p.0;
                                            }
                                            black_box(s);
                                        },
                                    );
                                    let sys = if hint == 0 {
                                        sys
                                    } else {
                                        sys.with_cost_hint_ns(hint)
                                    };
                                    schedule.add_system(sys);
                                }
                                schedule.build();
                                (schedule, world)
                            },
                            |(mut schedule, mut world)| {
                                schedule.run(&mut world);
                                black_box(schedule.batch_count())
                            },
                            BatchSize::LargeInput,
                        );
                    },
                );
            }
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_parallel_batch,
    bench_sequential_batches,
    bench_schedule_build,
    bench_parallel_calibrated,
);
criterion_main!(benches);
