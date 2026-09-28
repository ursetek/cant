//! Tests for the deferred command buffer.

use crate::{Commands, World};

#[derive(Debug, PartialEq, Clone, Copy)]
struct Pos(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Vel(f32, f32);

/// Helper: build a `Commands` bound to a world and apply it immediately.
fn roundtrip(world: &mut World, f: impl FnOnce(&mut Commands)) {
    let raw: *mut World = core::ptr::from_mut(world);
    // SAFETY: `raw` is a live pointer, no other reference exists.
    let mut commands = unsafe { Commands::from_raw(raw, true) };
    f(&mut commands);
    commands.apply();
}

/// A command-spawned entity is not visible until `apply`.
#[test]
fn spawn_is_deferred() {
    let mut world = World::new();
    let raw: *mut World = core::ptr::from_mut(&mut world);
    // SAFETY: raw pointer to a live world, unique access.
    let mut commands = unsafe { Commands::from_raw(raw, true) };
    let e = commands.spawn();
    assert!(!world.is_alive(e), "not visible before apply");
    commands.apply();
    assert!(world.is_alive(e), "visible after apply");
    assert_eq!(world.live_count(), 1);
}

/// `spawn_with` runs the closure at apply time and writes components.
#[test]
fn spawn_with_writes_components() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");

    let e = {
        let raw: *mut World = core::ptr::from_mut(&mut world);
        // SAFETY: unique access to a live world.
        let mut commands = unsafe { Commands::from_raw(raw, true) };
        let e = commands.spawn_with(|w, e| {
            w.add_component(e, Pos(1.0, 2.0));
        });
        commands.apply();
        e
    };

    assert!(world.is_alive(e));
    let loc = world.location(e).unwrap();
    let arch = world.archetype(loc.archetype);
    assert!(arch.has_component(pos));
}

/// `insert_component` on a reserved entity works because apply runs in order.
#[test]
fn insert_component_on_spawned_entity() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");

    let e = {
        let raw: *mut World = core::ptr::from_mut(&mut world);
        // SAFETY: unique access to a live world.
        let mut commands = unsafe { Commands::from_raw(raw, true) };
        let e = commands.spawn();
        commands.insert_component(e, Pos(3.0, 4.0));
        commands.apply();
        e
    };

    assert!(world.is_alive(e));
    let loc = world.location(e).unwrap();
    let arch = world.archetype(loc.archetype);
    assert!(arch.has_component(pos));
}

/// `despawn` is deferred and frees the slot at apply.
#[test]
fn despawn_is_deferred() {
    let mut world = World::new();
    let e = world.spawn();
    assert!(world.is_alive(e));

    roundtrip(&mut world, |cmd| cmd.despawn(e));

    assert!(!world.is_alive(e));
    assert_eq!(world.live_count(), 0);
}

/// `remove_component` is deferred.
#[test]
fn remove_component_is_deferred() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let e = world.spawn();
    world.add_component(e, Pos(0.0, 0.0));
    assert!(
        world
            .archetype(world.location(e).unwrap().archetype)
            .has_component(pos)
    );

    roundtrip(&mut world, |cmd| cmd.remove_component(e, pos));

    assert!(
        !world
            .archetype(world.location(e).unwrap().archetype)
            .has_component(pos)
    );
}

/// Commands apply in FIFO order: spawn then insert works, spawn then
/// despawn leaves the slot free.
#[test]
fn commands_apply_in_order() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");

    let (a, b) = {
        let raw: *mut World = core::ptr::from_mut(&mut world);
        // SAFETY: unique access to a live world.
        let mut commands = unsafe { Commands::from_raw(raw, true) };
        let a = commands.spawn();
        commands.insert_component(a, Pos(1.0, 0.0));
        let b = commands.spawn();
        commands.despawn(b);
        commands.apply();
        (a, b)
    };

    assert!(world.is_alive(a));
    assert!(!world.is_alive(b));
    assert!(
        world
            .archetype(world.location(a).unwrap().archetype)
            .has_component(pos)
    );
}

/// Two spawn calls return distinct entities.
#[test]
fn spawn_returns_distinct_entities() {
    let mut world = World::new();
    let (a, b) = {
        let raw: *mut World = core::ptr::from_mut(&mut world);
        // SAFETY: unique access to a live world.
        let mut commands = unsafe { Commands::from_raw(raw, true) };
        let a = commands.spawn();
        let b = commands.spawn();
        commands.apply();
        (a, b)
    };
    assert_ne!(a, b);
    assert!(world.is_alive(a));
    assert!(world.is_alive(b));
    assert_eq!(world.live_count(), 2);
}

/// `len` and `is_empty` track buffer occupancy.
#[test]
fn buffer_length_tracking() {
    let mut world = World::new();
    let raw: *mut World = core::ptr::from_mut(&mut world);
    // SAFETY: unique access to a live world.
    let mut commands = unsafe { Commands::from_raw(raw, true) };
    assert!(commands.is_empty());
    let e = commands.spawn();
    assert_eq!(commands.len(), 1);
    commands.despawn(e);
    assert_eq!(commands.len(), 2);
    commands.apply();
}
/// `spawn_bundle` writes all components in one migration.
#[test]
fn spawn_bundle_writes_components() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let vel = world.register_component::<Vel>("Vel");

    let e = {
        let raw: *mut World = core::ptr::from_mut(&mut world);
        // SAFETY: unique access.
        let mut commands = unsafe { Commands::from_raw(raw, true) };
        let e = commands.spawn_bundle((Pos(1.0, 2.0), Vel(3.0, 4.0)));
        commands.apply();
        e
    };

    let loc = world.location(e).unwrap();
    let arch = world.archetype(loc.archetype);
    assert!(arch.has_component(pos));
    assert!(arch.has_component(vel));
    assert_eq!(arch.len(), 1);
}

/// `Commands::spawn` from a parallel batch panics.
#[test]
#[should_panic(expected = "parallel batch")]
fn spawn_in_parallel_batch_panics() {
    let mut world = World::new();
    let raw: *mut World = core::ptr::from_mut(&mut world);
    // SAFETY: unique access, but spawning disabled.
    let mut commands = unsafe { Commands::from_raw(raw, false) };
    let _ = commands.spawn();
}
/// Multiple `spawn_bundle` calls in one buffer: zero per-command allocation.
#[test]
fn many_bundle_spawns_share_arena() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let vel = world.register_component::<Vel>("Vel");

    let entities = {
        let raw: *mut World = core::ptr::from_mut(&mut world);
        // SAFETY: unique access.
        let mut commands = unsafe { Commands::from_raw(raw, true) };
        let mut out = Vec::new();
        for i in 0..100u32 {
            #[allow(clippy::cast_precision_loss)]
            let e = commands.spawn_bundle((Pos(i as f32, 0.0), Vel(0.0, 0.0)));
            out.push(e);
        }
        commands.apply();
        out
    };

    assert_eq!(world.live_count(), 100);
    for e in &entities {
        let loc = world.location(*e).unwrap();
        let arch = world.archetype(loc.archetype);
        assert!(arch.has_component(pos));
        assert!(arch.has_component(vel));
    }
}

/// `insert_component` from arena works on an existing entity.
#[test]
fn insert_from_arena_migrates() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let e = world.spawn();

    {
        let raw: *mut World = core::ptr::from_mut(&mut world);
        // SAFETY: unique access.
        let mut commands = unsafe { Commands::from_raw(raw, true) };
        commands.insert_component(e, Pos(1.0, 2.0));
        commands.apply();
    }

    let loc = world.location(e).unwrap();
    let arch = world.archetype(loc.archetype);
    assert!(arch.has_component(pos));
}
