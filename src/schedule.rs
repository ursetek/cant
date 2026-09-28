//! System scheduling.

use core::fmt;

use rayon::prelude::*;

use crate::commands::Command;
use crate::query::UnsafeWorldCell;
use crate::system::System;
use crate::{Commands, SystemId, World};

/// Default fallback minimum batch size for the parallel path, applied only
/// when no system in the batch provides a cost hint.
///
/// Chosen so that batches of trivial systems (no hint, tens of nanoseconds
/// of work each) stay on the sequential path. Applications with heavier
/// systems should use cost hints instead.
pub const DEFAULT_PARALLEL_THRESHOLD: usize = 32;

/// Default cost budget, in nanoseconds.
///
/// Empirically, the parallel pipeline (`rayon::par_iter` plus command-buffer
/// collection) costs ~16 µs on a warm pool, independent of the batch size.
/// A batch should only run in parallel when its summed hint amortizes that
/// overhead with room to spare; a 30× ratio is a reasonable starting point.
/// Systems whose total hint is below this budget run sequentially.
pub const DEFAULT_PARALLEL_BUDGET_NS: u64 = 500_000;

/// Wrapper around a raw pointer that crosses thread boundaries inside rayon.
///
/// # Safety
///
/// The caller of [`SendPtr::get`] is responsible for ensuring no other
/// reference aliases the pointee for the duration of the access. Used only
/// inside [`Schedule::run`], where disjoint system indices and non-conflicting
/// access lists establish the invariant.
struct SendPtr<T>(*mut T);

// SAFETY: see `Schedule::run` for the invariants that justify these impls.
unsafe impl<T> Send for SendPtr<T> {}
// SAFETY: see above.
unsafe impl<T> Sync for SendPtr<T> {}

impl<T> SendPtr<T> {
    /// Returns the wrapped pointer. Used as a method so that closures capture
    /// the `SendPtr` wrapper itself, not the raw-pointer field.
    #[inline]
    fn get(&self) -> *mut T {
        self.0
    }
}

/// A collection of systems with a precomputed execution order.
///
/// Systems are grouped into batches at [`Schedule::build`] time. Systems
/// within a batch have pairwise non-conflicting [`AccessList`](crate::AccessList)s
/// and run in parallel; batches run sequentially. Deferred commands are
/// applied after each batch, in batch order.
pub struct Schedule {
    systems: Vec<Box<dyn System>>,
    batches: Vec<Vec<usize>>,
    built: bool,
    parallel_threshold: usize,
    batch_costs_ns: Vec<u64>,
    parallel_budget_ns: u64,
}

impl Schedule {
    /// Creates an empty schedule.
    #[must_use]
    pub fn new() -> Self {
        Self {
            systems: Vec::new(),
            batches: Vec::new(),
            batch_costs_ns: Vec::new(),
            built: false,
            parallel_threshold: DEFAULT_PARALLEL_THRESHOLD,
            parallel_budget_ns: DEFAULT_PARALLEL_BUDGET_NS,
        }
    }

    /// Sets the fallback minimum batch size for the parallel path.
    ///
    /// Applies only when no system in the batch provided a cost hint.
    pub fn set_parallel_threshold(&mut self, threshold: usize) {
        self.parallel_threshold = threshold;
    }

    /// Sets the cost budget, in nanoseconds.
    ///
    /// A batch whose summed hint is `>= budget` runs in parallel. Defaults
    /// to [`DEFAULT_PARALLEL_BUDGET_NS`], which accounts for the empirical
    /// ~16 µs overhead of the parallel pipeline on a warm rayon pool.
    ///
    /// Lowering the budget lets small but expensive systems parallelize;
    /// raising it keeps everything sequential. Setting `u64::MAX` disables
    /// cost-based parallelization.
    pub fn set_parallel_budget_ns(&mut self, budget_ns: u64) {
        self.parallel_budget_ns = budget_ns;
    }

    /// Returns the current parallel threshold.
    #[must_use]
    pub fn parallel_threshold(&self) -> usize {
        self.parallel_threshold
    }

    /// Returns the current cost budget.
    #[must_use]
    pub fn parallel_budget_ns(&self) -> u64 {
        self.parallel_budget_ns
    }

    /// Recomputes the batch ordering.
    pub fn build(&mut self) {
        self.batches = compute_batches(&self.systems);
        self.batch_costs_ns = self
            .batches
            .iter()
            .map(|b| {
                b.iter()
                    .map(|&i| {
                        u64::from(self.systems[i].access().cost_hint_ns())
                    })
                    .sum()
            })
            .collect();
        self.built = true;
    }

    /// Adds a system, returning its dense identifier.
    ///
    /// # Panics
    ///
    /// Panics if the system count exceeds `u32::MAX`. Unreachable in
    /// practice.
    pub fn add_system<S: System + 'static>(&mut self, system: S) -> SystemId {
        let id = SystemId::from_raw(
            u32::try_from(self.systems.len()).expect("system id overflow"),
        );
        self.systems.push(Box::new(system));
        self.built = false;
        id
    }

    /// Runs every system in the schedule, once, in batch order.
    ///
    /// Systems within a batch run in parallel; batches themselves run
    /// sequentially. Deferred commands are applied between batches, in batch
    /// order, for deterministic effects.
    pub fn run(&mut self, world: &mut World) {
        if !self.built {
            self.build();
        }

        let systems_ptr: *mut Box<dyn System> = self.systems.as_mut_ptr();
        let systems_len = self.systems.len();
        let sys = SendPtr(systems_ptr);
        let raw = SendPtr(core::ptr::from_mut(world));

        for (batch_idx, batch) in self.batches.iter().enumerate() {
            if batch.is_empty() {
                continue;
            }

            if batch.len() == 1 {
                // Single-system batch: spawning permitted. Always
                // sequential.
                let idx = batch[0];
                debug_assert!(idx < systems_len);
                // SAFETY: idx < systems_len.
                let system = unsafe { &mut *sys.get().add(idx) };
                // SAFETY: unique access to world; structural writes allowed.
                let cell = unsafe { UnsafeWorldCell::from_raw(raw.get()) };
                // SAFETY: unique access to world; spawning permitted.
                let mut commands =
                    unsafe { Commands::from_raw(raw.get(), true) };
                system.run(cell, &mut commands);
                commands.apply();
                continue;
            }

            // Multi-system batch: systems are pairwise non-conflicting by
            // construction, so no system here is a structural writer and
            // spawning is disabled for all of them.
            let batch_cost = self.batch_costs_ns[batch_idx];
            let use_parallel = should_parallelize(
                batch.len(),
                batch_cost,
                self.parallel_threshold,
                self.parallel_budget_ns,
            );

            if !use_parallel {
                let mut buffers: Vec<(Vec<Command>, Vec<u8>)> =
                    Vec::with_capacity(batch.len());
                for &idx in batch {
                    debug_assert!(idx < systems_len);
                    // SAFETY: idx < systems_len; sequential iteration.
                    let system = unsafe { &mut *sys.get().add(idx) };
                    // SAFETY: no conflicting access across sequential calls.
                    let cell = unsafe { UnsafeWorldCell::from_raw(raw.get()) };
                    // SAFETY: spawning disabled for multi-system batches.
                    let mut local =
                        unsafe { Commands::from_raw(raw.get(), false) };
                    system.run(cell, &mut local);
                    buffers.push(local.into_parts());
                }
                for (buffer, arena) in buffers {
                    // SAFETY: buffers were produced against this world.
                    unsafe {
                        Commands::apply_parts(&mut *raw.get(), buffer, &arena);
                    }
                }
                continue;
            }

            // Parallel path.
            // SAFETY: indices in a batch are distinct; systems in a batch
            // have non-conflicting access lists; spawning is disabled.
            let buffers: Vec<(Vec<Command>, Vec<u8>)> = batch
                .par_iter()
                .map(|&idx| {
                    debug_assert!(idx < systems_len);
                    // SAFETY: distinct index per task.
                    let system = unsafe { &mut *sys.get().add(idx) };
                    // SAFETY: no conflicting access across threads.
                    let cell = unsafe { UnsafeWorldCell::from_raw(raw.get()) };
                    // SAFETY: spawning disabled.
                    let mut commands =
                        unsafe { Commands::from_raw(raw.get(), false) };
                    system.run(cell, &mut commands);
                    commands.into_parts()
                })
                .collect();

            for (buffer, arena) in buffers {
                // SAFETY: buffers were produced against this world.
                unsafe {
                    Commands::apply_parts(&mut *raw.get(), buffer, &arena);
                }
            }
        }
    }

    /// Number of systems in the schedule.
    #[must_use]
    pub fn system_count(&self) -> usize {
        self.systems.len()
    }

    /// Number of batches after the last build.
    #[must_use]
    pub fn batch_count(&self) -> usize {
        self.batches.len()
    }

    /// Batch layout: each entry is a list of system indices.
    #[must_use]
    pub fn batches(&self) -> &[Vec<usize>] {
        &self.batches
    }
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Schedule")
            .field("systems", &self.systems.len())
            .field("batches", &self.batches.len())
            .field("built", &self.built)
            .field("parallel_threshold", &self.parallel_threshold)
            .field("parallel_budget_ns", &self.parallel_budget_ns)
            .finish_non_exhaustive()
    }
}

/// Decides whether a multi-system batch runs in parallel.
///
/// - If the batch's summed cost hint is non-zero, the batch runs in
///   parallel only when the hint reaches `budget_ns`.
/// - Otherwise (no hints), the batch runs in parallel only when its size
///   reaches `threshold`.
#[inline]
fn should_parallelize(
    len: usize,
    cost_ns: u64,
    threshold: usize,
    budget_ns: u64,
) -> bool {
    debug_assert!(len >= 2, "single-system batches bypass this decision");
    if cost_ns > 0 {
        cost_ns >= budget_ns
    } else {
        len >= threshold
    }
}

/// Greedy batch assignment.
fn compute_batches(systems: &[Box<dyn System>]) -> Vec<Vec<usize>> {
    let mut batches: Vec<Vec<usize>> = Vec::new();
    'outer: for (i, sys) in systems.iter().enumerate() {
        let acc = sys.access();
        for batch in &mut batches {
            let compatible = batch
                .iter()
                .all(|&j| !systems[j].access().conflicts_with(acc));
            if compatible {
                batch.push(i);
                continue 'outer;
            }
        }
        batches.push(vec![i]);
    }
    batches
}
