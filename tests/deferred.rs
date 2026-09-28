//! Deferred structural changes via `Commands`.

use cant::{AccessList, Entity, FnSystem, QueryMask, Schedule, World};

#[derive(Debug, PartialEq, Clone, Copy)]
struct Position(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Velocity(f32, f32);

/// A system that spawns entities through `Commands`; the effect is visible
/// after `Schedule::run`.
#[test]
fn spawn_through_commands() {
    let mut schedule = Schedule::new();
    let mut access = AccessList::new();
    access.mark_structural_write();
    schedule.add_system(FnSystem::new("spawner", access, |_, commands| {
        for _ in 0..5 {
            let _ = commands.spawn();
        }
    }));

    let mut world = World::new();
    schedule.run(&mut world);
    assert_eq!(world.live_count(), 5);
}

/// `spawn_bundle` defers both slot reservation and component writes; the
/// entity is fully materialized after the stage.
#[test]
fn spawn_bundle_through_commands() {
    let mut schedule = Schedule::new();
    let mut access = AccessList::new();
    access.mark_structural_write();
    schedule.add_system(FnSystem::new("spawner", access, |_, commands| {
        for i in 0..3u32 {
            #[allow(clippy::cast_precision_loss)]
            let _ = commands
                .spawn_bundle((Position(i as f32, 0.0), Velocity(0.0, 1.0)));
        }
    }));

    let mut world = World::new();
    let pos = world.register_component::<Position>("Position");
    let vel = world.register_component::<Velocity>("Velocity");
    schedule.run(&mut world);

    assert_eq!(world.live_count(), 3);

    let mut mask = QueryMask::new();
    mask.read(pos).read(vel);
    let mut query = world.prepare(mask).unwrap();

    let mut count = 0;
    query.for_each(&world, |view| {
        count += view.len();
        let ps = view.column::<Position>(0).unwrap();
        let vs = view.column::<Velocity>(1).unwrap();
        for (p, v) in ps.iter().zip(vs.iter()) {
            assert_eq!(*v, Velocity(0.0, 1.0));
            assert!((0.0..3.0).contains(&p.0));
        }
    });
    assert_eq!(count, 3);
}

/// A command-spawned entity can be captured by a later command through the
/// entity handle.
#[test]
fn spawn_then_insert_in_order() {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<Entity>));
    let captured_in = std::sync::Arc::clone(&captured);

    let mut schedule = Schedule::new();
    let mut access = AccessList::new();
    access.mark_structural_write();
    schedule.add_system(FnSystem::new(
        "spawner",
        access,
        move |_, commands| {
            let e = commands.spawn();
            commands.insert_component(e, Position(7.0, 8.0));
            *captured_in.lock().unwrap() = Some(e);
        },
    ));

    let mut world = World::new();
    let pos = world.register_component::<Position>("Position");
    schedule.run(&mut world);

    let e = captured.lock().unwrap().unwrap();
    assert!(world.is_alive(e));
    let loc = world.location(e).unwrap();
    assert!(world.archetype(loc.archetype).has_component(pos));
}

/// A command-despawned entity does not exist after the stage.
#[test]
fn despawn_through_commands() {
    let mut world = World::new();
    let e = world.spawn();

    let target = std::sync::Arc::new(std::sync::Mutex::new(Some(e)));
    let target_in = std::sync::Arc::clone(&target);
    let mut schedule = Schedule::new();
    let mut access = AccessList::new();
    access.mark_structural_write();
    schedule.add_system(FnSystem::new(
        "despawner",
        access,
        move |_, commands| {
            if let Some(e) = *target_in.lock().unwrap() {
                commands.despawn(e);
            }
        },
    ));

    schedule.run(&mut world);
    assert!(!world.is_alive(e));
    assert_eq!(world.live_count(), 0);
}

/// `Commands::spawn` from a parallel batch panics with a clear message.
#[test]
#[should_panic(expected = "parallel batch")]
fn spawn_in_parallel_batch_panics() {
    let mut schedule = Schedule::new();

    // First: a non-conflicting sibling, so we land in a parallel batch.
    let mut reader = AccessList::new();
    reader.add_component(cant::ComponentId::from_raw(0), cant::Access::Read);
    schedule.add_system(FnSystem::new("reader", reader, |_, _| {}));

    // Second: an attacker that spawns without marking structural write.
    // Its access list is disjoint from the reader's, so the batcher puts it
    // in the same batch; spawn panics at runtime.
    let mut attacker = AccessList::new();
    attacker.add_component(cant::ComponentId::from_raw(1), cant::Access::Read);
    schedule.add_system(FnSystem::new("attacker", attacker, |_, commands| {
        let _: Entity = commands.spawn();
    }));

    let mut world = World::new();
    schedule.run(&mut world);
}

/// `Commands` is consumed by `apply`; the schedule owns this cycle.
#[test]
fn commands_apply_is_idempotent_inside_schedule() {
    let mut schedule = Schedule::new();
    let mut access = AccessList::new();
    access.mark_structural_write();
    schedule.add_system(FnSystem::new("once", access, |_, commands| {
        let _ = commands.spawn();
        assert_eq!(commands.len(), 1);
        assert!(!commands.is_empty());
    }));

    let mut world = World::new();
    schedule.run(&mut world);
    assert_eq!(world.live_count(), 1);
}
