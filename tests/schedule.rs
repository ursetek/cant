//! Schedule-driven execution through the public API.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cant::{
    Access, AccessList, ComponentId, FnSystem, QueryMask, QuerySystem,
    Schedule, World,
};

#[derive(Debug, Clone, Copy)]
struct Position(f32, #[allow(dead_code)] f32);

/// Independent systems land in one batch and run.
#[test]
fn independent_systems_run_in_one_batch() {
    let counter = Arc::new(AtomicU32::new(0));
    let mut schedule = Schedule::new();

    for _ in 0..4 {
        let c = Arc::clone(&counter);
        schedule.add_system(FnSystem::new(
            "inc",
            AccessList::new(),
            move |_, _| {
                c.fetch_add(1, Ordering::Relaxed);
            },
        ));
    }

    schedule.build();
    assert_eq!(schedule.batch_count(), 1);
    assert_eq!(schedule.batches()[0].len(), 4);

    let mut world = World::new();
    schedule.run(&mut world);
    assert_eq!(counter.load(Ordering::Relaxed), 4);
}

/// Systems writing to the same component go into separate batches.
#[test]
fn conflicting_writes_split_batches() {
    let mut schedule = Schedule::new();

    let mut a = AccessList::new();
    a.add_component(ComponentId::from_raw(0), Access::Write);
    schedule.add_system(FnSystem::new("a", a, |_, _| {}));

    let mut b = AccessList::new();
    b.add_component(ComponentId::from_raw(0), Access::Write);
    schedule.add_system(FnSystem::new("b", b, |_, _| {}));

    schedule.build();
    assert_eq!(schedule.batch_count(), 2);
}

/// A `QuerySystem` derives its access list from the query and iterates.
#[test]
fn query_system_iterates_world() {
    let mut world = World::new();
    let pos = world.register_component::<Position>("Position");

    for i in 0..5 {
        let e = world.spawn();
        #[allow(clippy::cast_precision_loss)]
        world.add_component(e, Position(i as f32, 0.0));
    }

    let mut mask = QueryMask::new();
    mask.write(pos);
    let query = world.prepare(mask).unwrap();

    let total = Arc::new(AtomicU32::new(0));
    let total_for_closure = Arc::clone(&total);
    let mut schedule = Schedule::new();
    schedule.add_system(QuerySystem::new("double", query, move |mut view| {
        let ps = view.column_mut::<Position>(0).unwrap();
        for p in ps {
            p.0 *= 2.0;
        }
        total_for_closure.fetch_add(view.len(), Ordering::Relaxed);
    }));

    schedule.run(&mut world);
    assert_eq!(total.load(Ordering::Relaxed), 5);

    let mut read_mask = QueryMask::new();
    read_mask.read(pos);
    let mut read_query = world.prepare(read_mask).unwrap();
    let mut values = Vec::new();
    read_query.for_each(&world, |view| {
        for p in view.column::<Position>(0).unwrap() {
            values.push(p.0);
        }
    });
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(values, vec![0.0, 2.0, 4.0, 6.0, 8.0]);
}

/// A structural writer is isolated in its own batch and can spawn.
#[test]
fn structural_writer_spawns() {
    let mut schedule = Schedule::new();
    let mut access = AccessList::new();
    access.mark_structural_write();
    schedule.add_system(FnSystem::new("spawner", access, |_, commands| {
        let _ = commands.spawn();
    }));

    // Another system without conflicts still lands in a separate batch
    // because the spawner's structural flag conflicts with everything.
    let mut other = AccessList::new();
    other.add_component(ComponentId::from_raw(0), Access::Write);
    schedule.add_system(FnSystem::new("other", other, |_, _| {}));

    schedule.build();
    assert_eq!(schedule.batch_count(), 2);

    let mut world = World::new();
    schedule.run(&mut world);
    assert_eq!(world.live_count(), 1);
}

/// Adding a system after the first run rebuilds the schedule.
#[test]
fn adding_system_after_run_rebuilds() {
    let mut schedule = Schedule::new();
    schedule.add_system(FnSystem::new("a", AccessList::new(), |_, _| {}));

    let mut world = World::new();
    schedule.run(&mut world);
    assert_eq!(schedule.batch_count(), 1);

    schedule.add_system(FnSystem::new("b", AccessList::new(), |_, _| {}));
    schedule.run(&mut world);
    assert_eq!(schedule.batches()[0].len(), 2);
}
