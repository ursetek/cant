//! Scheduler benchmarks.
//!
//! Two orthogonal things are measured here:
//!
//! 1. **Batch construction** — how the greedy batcher groups systems by
//!    `AccessList`. This is a build-time cost; it runs once per schedule.
//! 2. **Pipeline overhead** — the per-batch cost of dispatching systems
//!    through the sequential versus parallel path. Workloads are trivial
//!    (empty closures) so the numbers reflect pipeline cost alone, not
//!    system work.
//!
//! Absolute throughput of real systems is intentionally not measured here:
//! it depends on the workload and drifts with thermal state, making
//! cross-run comparison unreliable. Use `tests/` for correctness and query
//! benchmarks for hot-loop throughput.

#![allow(missing_docs)]

use std::hint::black_box;

use criterion::{
    BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main,
};

use cant::{Access, AccessList, ComponentId, FnSystem, Schedule, World};

// ---------------------------------------------------------------------------
// Sequential batches: N systems, each in its own batch.
// ---------------------------------------------------------------------------

/// N systems, all writing the same component. The batcher cannot place any
/// two of them in a batch together, so the schedule has exactly N batches
/// of one system each. Measures sequential dispatch cost.
fn bench_sequential_batches(c: &mut Criterion) {
    let mut group = c.benchmark_group("schedule_sequential");

    for &n_systems in &[2u32, 4, 8, 16] {
        group.bench_with_input(
            BenchmarkId::from_parameter(n_systems),
            &n_systems,
            |b, &n_systems| {
                b.iter_batched(
                    || {
                        let mut schedule = Schedule::new();
                        for i in 0..n_systems {
                            let mut access = AccessList::new();
                            access.add_component(
                                ComponentId::from_raw(0),
                                Access::Write,
                            );
                            schedule.add_system(FnSystem::new(
                                format!("s{i}"),
                                access,
                                |_, _| {},
                            ));
                        }
                        (schedule, World::new())
                    },
                    |(mut schedule, mut world)| {
                        schedule.run(&mut world);
                        black_box(schedule.batch_count())
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Schedule build cost.
// ---------------------------------------------------------------------------

/// Cost of `Schedule::build` as a function of system count.
///
/// Systems are spread across a small set of distinct components so the
/// batcher has actual work to do (it cannot put all of them in one batch).
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
                            // Four distinct components → up to four
                            // systems per batch.
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

// ---------------------------------------------------------------------------
// Pipeline overhead: parallel vs sequential dispatch on trivial systems.
// ---------------------------------------------------------------------------

/// Force a batch through the parallel path, regardless of its size.
///
/// Systems are trivial (they do nothing), so the measured cost is the
/// pipeline itself: `thread::scope` setup, one `pthread_create` per worker
/// minus one, buffer collection, join, command-buffer application. No
/// component data is touched, so thermal effects do not contaminate the
/// numbers.
///
/// The `seq` column runs the same systems through the sequential path by
/// setting the budget out of reach. Comparing the two at the same `n`
/// isolates the parallel pipeline cost.
fn bench_pipeline_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_overhead");
    group.sample_size(500);

    for &n_systems in &[2usize, 4, 8, 16, 32] {
        for &mode in &["seq", "par"] {
            let id = format!("n{n_systems}_{mode}");

            group.bench_with_input(
                BenchmarkId::from_parameter(&id),
                &n_systems,
                |b, &n_systems| {
                    b.iter_batched(
                        || {
                            let mut schedule = Schedule::new();
                            for i in 0..n_systems {
                                let mut access = AccessList::new();
                                // Distinct component per system: no
                                // conflicts, so all systems land in one
                                // batch.
                                #[allow(clippy::cast_possible_truncation)]
                                access.add_component(
                                    ComponentId::from_raw(i as u32),
                                    Access::Write,
                                );
                                // Non-zero hint so the decision is driven
                                // by `budget_ns`, not by the size
                                // threshold.
                                access.set_cost_hint_ns(1);
                                schedule.add_system(FnSystem::new(
                                    format!("s{i}"),
                                    access,
                                    |_, _| {
                                        // Trivial body. `black_box` keeps
                                        // the closure from being optimized
                                        // out entirely.
                                        black_box(0u64);
                                    },
                                ));
                            }

                            if mode == "par" {
                                // Any non-zero hint ≥ 0 → parallel.
                                schedule.set_parallel_budget_ns(0);
                            } else {
                                // No hint can reach `u64::MAX` → sequential.
                                schedule.set_parallel_budget_ns(u64::MAX);
                            }
                            schedule.build();

                            (schedule, World::new())
                        },
                        |(mut schedule, mut world)| {
                            schedule.run(&mut world);
                            black_box(schedule.batch_count())
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_sequential_batches,
    bench_schedule_build,
    bench_pipeline_overhead,
);
criterion_main!(benches);
