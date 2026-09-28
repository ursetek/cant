//! Shared fixtures for benchmarks.

#![allow(dead_code)]

use cant::{Query, QueryMask, World};

#[derive(Debug, Clone, Copy)]
pub struct Position(pub f32, pub f32);

#[derive(Debug, Clone, Copy)]
pub struct Velocity(pub f32, pub f32);

/// Builds a world with `n` entities, each carrying `(Position, Velocity)`.
///
/// Reserves capacity up front so the hot loop is not interleaved with `Vec`
/// growth.
pub fn world_with_motion(n: u32) -> World {
    let mut world = World::new();
    let _ = world.register_component::<Position>("Position");
    let _ = world.register_component::<Velocity>("Velocity");
    let bid = world.register_bundle::<(Position, Velocity)>();
    world.reserve_entities(n);
    let archetype = world.bundle_archetype(bid);
    world.reserve_archetype(archetype, n);
    for i in 0..n {
        #[allow(clippy::cast_precision_loss)]
        world.spawn_bundle_id(
            bid,
            (Position(i as f32, 0.0), Velocity(0.0, 1.0)),
        );
    }
    world
}

/// Prepares a read-only `Position` query against `world`.
pub fn prepare_read_positions(world: &World) -> Query {
    let pos = world
        .component_id::<Position>()
        .expect("Position registered");
    let mut mask = QueryMask::new();
    mask.read(pos);
    world.prepare(mask).expect("prepare")
}

/// Prepares a write `Position` query against `world`.
pub fn prepare_write_positions(world: &World) -> Query {
    let pos = world
        .component_id::<Position>()
        .expect("Position registered");
    let mut mask = QueryMask::new();
    mask.write(pos);
    world.prepare(mask).expect("prepare")
}
