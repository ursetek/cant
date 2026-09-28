//! Tests for bundle registration and fast-path spawn.

use crate::{Bundle, BundleId, World};

#[derive(Debug, PartialEq, Clone, Copy)]
struct Pos(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Vel(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Health(u32);

/// Registering the same bundle type twice returns the same id.
#[test]
fn register_is_idempotent() {
    let mut world = World::new();
    let _ = world.register_component::<Pos>("Pos");
    let _ = world.register_component::<Vel>("Vel");

    let a = world.register_bundle::<(Pos, Vel)>();
    let b = world.register_bundle::<(Pos, Vel)>();
    assert_eq!(a, b);
}

/// Different tuple orderings produce different bundle ids (slots differ) but
/// share the same archetype.
#[test]
fn orderings_share_archetype() {
    let mut world = World::new();
    let _ = world.register_component::<Pos>("Pos");
    let _ = world.register_component::<Vel>("Vel");

    let ab = world.register_bundle::<(Pos, Vel)>();
    let ba = world.register_bundle::<(Vel, Pos)>();
    assert_ne!(ab, ba);
    assert_eq!(world.bundle_archetype(ab), world.bundle_archetype(ba));
}

/// `spawn_bundle_id` places the entity in the bundle's archetype and writes
/// all components.
#[test]
fn spawn_bundle_id_writes_all_components() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let vel = world.register_component::<Vel>("Vel");
    let bid = world.register_bundle::<(Pos, Vel)>();

    let e = world.spawn_bundle_id(bid, (Pos(1.0, 2.0), Vel(3.0, 4.0)));
    assert!(world.is_alive(e));

    let loc = world.location(e).unwrap();
    assert_eq!(loc.archetype, world.bundle_archetype(bid));
    let arch = world.archetype(loc.archetype);
    assert!(arch.has_component(pos));
    assert!(arch.has_component(vel));
}

/// `spawn_bundle` picks up an already-registered bundle on the second call.
#[test]
fn spawn_bundle_reuses_registration() {
    let mut world = World::new();
    let _ = world.register_component::<Pos>("Pos");
    let _ = world.register_component::<Vel>("Vel");

    let e1 = world.spawn_bundle((Pos(0.0, 0.0), Vel(0.0, 0.0)));
    let arch1 = world.location(e1).unwrap().archetype;

    let e2 = world.spawn_bundle((Pos(1.0, 1.0), Vel(1.0, 1.0)));
    let arch2 = world.location(e2).unwrap().archetype;

    assert_eq!(arch1, arch2, "second spawn reuses cached archetype");
    assert_eq!(world.archetype(arch1).len(), 2);
}

/// Empty bundle spawns into the empty archetype.
#[test]
fn empty_bundle_spawns_into_empty_archetype() {
    let mut world = World::new();
    let bid: BundleId = world.register_bundle::<()>();
    let e = world.spawn_bundle_id(bid, ());
    assert_eq!(
        world.location(e).unwrap().archetype,
        world.empty_archetype()
    );
}

/// `collect_ids` fills the output in tuple order.
#[test]
fn collect_ids_order() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let vel = world.register_component::<Vel>("Vel");
    let hp = world.register_component::<Health>("Health");

    let mut ids = Vec::new();
    <(Pos, Vel, Health)>::collect_ids(&world, &mut ids);
    assert_eq!(ids, vec![pos, vel, hp]);

    let mut ids_ba = Vec::new();
    <(Vel, Pos)>::collect_ids(&world, &mut ids_ba);
    assert_eq!(ids_ba, vec![vel, pos]);
}
