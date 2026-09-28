//! Tests for the schedule: batching, ordering, and execution.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::query::Access;
use crate::schedule::Schedule;
use crate::system::{AccessList, FnSystem, QuerySystem};
use crate::{ComponentId, QueryMask, ResourceId, World};

#[derive(Debug, Clone, Copy)]
struct Pos(f32, f32);

/// Systems with empty access lists share a single batch.
#[test]
fn independent_systems_share_batch() {
    let mut schedule = Schedule::new();
    schedule.add_system(FnSystem::new("a", AccessList::new(), |_, _| {}));
    schedule.add_system(FnSystem::new("b", AccessList::new(), |_, _| {}));
    schedule.build();
    assert_eq!(schedule.batch_count(), 1);
    assert_eq!(schedule.batches()[0], vec![0, 1]);
}

/// Two write accesses to the same component get separate batches.
#[test]
fn conflicting_component_writes_split_batches() {
    let c = ComponentId::from_raw(0);
    let mut schedule = Schedule::new();

    let mut a = AccessList::new();
    a.add_component(c, Access::Write);
    schedule.add_system(FnSystem::new("a", a, |_, _| {}));

    let mut b = AccessList::new();
    b.add_component(c, Access::Write);
    schedule.add_system(FnSystem::new("b", b, |_, _| {}));

    schedule.build();
    assert_eq!(schedule.batch_count(), 2);
}

/// Two write accesses to the same resource get separate batches.
#[test]
fn conflicting_resource_writes_split_batches() {
    let r = ResourceId::from_raw(0);
    let mut schedule = Schedule::new();

    let mut a = AccessList::new();
    a.add_resource(r, Access::Write);
    schedule.add_system(FnSystem::new("a", a, |_, _| {}));

    let mut b = AccessList::new();
    b.add_resource(r, Access::Write);
    schedule.add_system(FnSystem::new("b", b, |_, _| {}));

    schedule.build();
    assert_eq!(schedule.batch_count(), 2);
}

/// Systems touching disjoint components share a single batch.
#[test]
fn disjoint_access_share_batch() {
    let mut schedule = Schedule::new();
    for i in 0..4 {
        let mut access = AccessList::new();
        access.add_component(ComponentId::from_raw(i), Access::Write);
        schedule.add_system(FnSystem::new(format!("s{i}"), access, |_, _| {}));
    }
    schedule.build();
    assert_eq!(schedule.batch_count(), 1);
    assert_eq!(schedule.batches()[0].len(), 4);
}

/// `Schedule::run` executes every system once.
#[test]
fn run_executes_all_systems() {
    let counter = Arc::new(AtomicU32::new(0));
    let mut schedule = Schedule::new();

    for _ in 0..3 {
        let c = Arc::clone(&counter);
        schedule.add_system(FnSystem::new(
            "inc",
            AccessList::new(),
            move |_, _| {
                c.fetch_add(1, Ordering::Relaxed);
            },
        ));
    }

    let mut world = World::new();
    schedule.run(&mut world);
    assert_eq!(counter.load(Ordering::Relaxed), 3);
}

/// A query-based system writes through to storage.
#[test]
fn schedule_with_query_system() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    for i in 0..3 {
        let e = world.spawn();
        // `i < 3`, exact in f32.
        #[allow(clippy::cast_precision_loss)]
        world.add_component(e, Pos(i as f32, 0.0));
    }

    let mut mask = QueryMask::new();
    mask.write(pos);
    let query = world.prepare(mask).unwrap();

    let mut schedule = Schedule::new();
    schedule.add_system(QuerySystem::new("double", query, |mut view| {
        let ps = view.column_mut::<Pos>(0).expect("write column");
        for p in ps {
            p.0 *= 2.0;
        }
    }));

    schedule.run(&mut world);

    let mut read_mask = QueryMask::new();
    read_mask.read(pos);
    let mut read_query = world.prepare(read_mask).unwrap();
    let mut values = Vec::new();
    read_query.for_each(&world, |view| {
        let ps = view.column::<Pos>(0).expect("read column");
        for p in ps {
            values.push(p.0);
        }
    });
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(values, vec![0.0, 2.0, 4.0]);
}

/// Adding a system after the first run triggers a rebuild.
#[test]
fn adding_system_after_run_rebuilds() {
    let mut schedule = Schedule::new();
    schedule.add_system(FnSystem::new("a", AccessList::new(), |_, _| {}));
    let mut world = World::new();
    schedule.run(&mut world);
    assert_eq!(schedule.batch_count(), 1);

    schedule.add_system(FnSystem::new("b", AccessList::new(), |_, _| {}));
    schedule.run(&mut world);
    assert_eq!(schedule.batch_count(), 1);
    assert_eq!(schedule.batches()[0].len(), 2);
}

/// Non-conflicting systems in one batch run in parallel and each gets its
/// own commands buffer; effects are applied in batch order.
#[test]
fn parallel_batch_runs_all_systems() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    let mut schedule = Schedule::new();
    let counter = Arc::new(AtomicU32::new(0));

    for i in 0..8 {
        let c = Arc::clone(&counter);
        let mut access = AccessList::new();
        access.add_component(ComponentId::from_raw(i), Access::Write);
        schedule.add_system(FnSystem::new(
            format!("s{i}"),
            access,
            move |_, _| {
                c.fetch_add(1, Ordering::Relaxed);
            },
        ));
    }

    schedule.build();
    assert_eq!(schedule.batch_count(), 1);
    assert_eq!(schedule.batches()[0].len(), 8);

    let mut world = World::new();
    schedule.run(&mut world);
    assert_eq!(counter.load(Ordering::Relaxed), 8);
}

/// Structural writers get their own batch and run sequentially.
#[test]
fn structural_writer_is_isolated() {
    let mut schedule = Schedule::new();
    let mut a = AccessList::new();
    a.add_component(ComponentId::from_raw(0), Access::Read);
    a.mark_structural_write();
    schedule.add_system(FnSystem::new("spawner", a, |_, _| {}));

    let mut b = AccessList::new();
    b.add_component(ComponentId::from_raw(1), Access::Write);
    schedule.add_system(FnSystem::new("writer", b, |_, _| {}));

    schedule.build();
    assert_eq!(schedule.batch_count(), 2);
    assert_eq!(schedule.batches()[0].len(), 1);
    assert_eq!(schedule.batches()[1].len(), 1);
}

/// A spawning system runs its structural change through the sequential
/// path.
#[test]
fn structural_writer_can_spawn() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    let seen = Arc::new(AtomicU64::new(0));
    let seen_for_closure = Arc::clone(&seen);

    let mut schedule = Schedule::new();
    let mut access = AccessList::new();
    access.mark_structural_write();
    schedule.add_system(FnSystem::new(
        "spawner",
        access,
        move |_, commands| {
            let e = commands.spawn();
            seen_for_closure.store(e.to_bits(), Ordering::Relaxed);
        },
    ));

    let mut world = World::new();
    schedule.run(&mut world);

    assert_eq!(world.live_count(), 1);
    let bits = seen.load(Ordering::Relaxed);
    let e = crate::Entity::from_bits(bits).unwrap();
    assert!(world.is_alive(e));
}
