//! System scheduling.

use core::fmt;

use rayon::prelude::*;

use crate::commands::Command;
use crate::query::UnsafeWorldCell;
use crate::system::System;
use crate::{Commands, SystemId, World};

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
}

impl Schedule {
    /// Creates an empty schedule.
    #[must_use]
    pub fn new() -> Self {
        Self {
            systems: Vec::new(),
            batches: Vec::new(),
            built: false,
        }
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

    /// Recomputes the batch ordering.
    pub fn build(&mut self) {
        self.batches = compute_batches(&self.systems);
        self.built = true;
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

        for batch in &self.batches {
            if batch.is_empty() {
                continue;
            }
            if batch.len() == 1 {
                let idx = batch[0];
                debug_assert!(idx < systems_len);
                // SAFETY: idx < systems_len; no other reference to this system.
                let system = unsafe { &mut *sys.get().add(idx) };
                // SAFETY: unique access to world; structural writes allowed.
                let cell = unsafe { UnsafeWorldCell::from_raw(raw.get()) };
                let mut commands =
                    unsafe { Commands::from_raw(raw.get(), true) };
                system.run(cell, &mut commands);
                commands.apply();
            } else {
                // SAFETY: indices in a batch are distinct; systems in a
                // batch have non-conflicting access lists; spawning is
                // disabled, so no thread mutates the entity table.
                let buffers: Vec<(Vec<Command>, Vec<u8>)> = batch
                    .par_iter()
                    .map(|&idx| {
                        debug_assert!(idx < systems_len);
                        // SAFETY: distinct index per task.
                        let system = unsafe { &mut *sys.get().add(idx) };
                        // SAFETY: no conflicting access across threads.
                        let cell =
                            unsafe { UnsafeWorldCell::from_raw(raw.get()) };
                        // SAFETY: spawn disabled.
                        let mut commands =
                            unsafe { Commands::from_raw(raw.get(), false) };
                        system.run(cell, &mut commands);
                        commands.into_parts()
                    })
                    .collect();

                // Sequential apply in batch order.
                for (buffer, arena) in buffers {
                    // SAFETY: buffers were produced by the enqueue path on
                    // this same world; `world` is exclusively borrowed here.
                    unsafe { Commands::apply_parts(world, buffer, &arena) };
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
            .finish()
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
