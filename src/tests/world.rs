//! Tests for `World`: registration, spawn/despawn, migration, resources.

use crate::{ArchetypeId, ComponentKind, ComponentMask, Entity, World};

#[derive(Debug, PartialEq, Clone, Copy)]
struct Pos(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Vel(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Health(u32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Marker;

fn read_pos(world: &World, entity: Entity, comp: crate::ComponentId) -> Pos {
    let loc = world.location(entity).expect("alive");
    let arch = world.archetype(loc.archetype);
    let storage = arch.storage(comp).expect("has Pos");
    let dense = storage
        .as_any()
        .downcast_ref::<crate::storage::DenseStorage<Pos>>()
        .expect("correct storage");
    let chunk = arch.chunk_of(loc.row);
    let rin = arch.row_in_chunk(loc.row);
    dense.chunk_slice(chunk)[rin as usize]
}

/// Registration assigns dense IDs and records kind based on size.
#[test]
fn register_components_dense_and_zst() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let marker = world.register_component::<Marker>("Marker");

    assert_eq!(pos.raw(), 0);
    assert_eq!(marker.raw(), 1);
    assert_eq!(world.component_info(pos).kind(), ComponentKind::Dense);
    assert_eq!(world.component_info(marker).kind(), ComponentKind::Zst);
    assert_eq!(world.component_count(), 2);
}

/// A fresh world has one archetype (the empty one) and no entities.
#[test]
fn new_world_has_empty_archetype_only() {
    let world = World::new();
    assert_eq!(world.archetype_count(), 1);
    assert_eq!(world.live_count(), 0);
    let empty = world.archetype(world.empty_archetype());
    assert!(empty.mask().is_empty());
}

/// Spawning creates an entity in the empty archetype; despawning kills it.
#[test]
fn spawn_despawn_roundtrip() {
    let mut world = World::new();
    let e = world.spawn();
    assert!(world.is_alive(e));
    assert_eq!(world.live_count(), 1);
    assert_eq!(world.entity_archetype(e), Some(world.empty_archetype()));

    assert!(world.despawn(e));
    assert!(!world.is_alive(e));
    assert_eq!(world.live_count(), 0);
    assert!(!world.despawn(e), "double despawn returns false");
}

/// Adding a component migrates the entity and stores the value.
#[test]
fn add_component_migrates_entity() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let e = world.spawn();

    assert!(world.add_component(e, Pos(1.0, 2.0)));
    assert_ne!(world.entity_archetype(e), Some(world.empty_archetype()));
    assert_eq!(read_pos(&world, e, pos), Pos(1.0, 2.0));

    // Adding again replaces in place, archetype does not change.
    let arch_after_first = world.entity_archetype(e);
    assert!(world.add_component(e, Pos(3.0, 4.0)));
    assert_eq!(world.entity_archetype(e), arch_after_first);
    assert_eq!(read_pos(&world, e, pos), Pos(3.0, 4.0));
}

/// Adding two components to two entities produces the same archetype.
#[test]
fn add_reuses_archetype() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let vel = world.register_component::<Vel>("Vel");

    let a = world.spawn();
    let b = world.spawn();
    world.add_component(a, Pos(1.0, 0.0));
    world.add_component(a, Vel(0.0, 1.0));
    world.add_component(b, Pos(2.0, 0.0));
    world.add_component(b, Vel(0.0, 2.0));

    assert_eq!(world.entity_archetype(a), world.entity_archetype(b));
    let arch = world.archetype(world.entity_archetype(a).unwrap());
    assert_eq!(arch.len(), 2);
    assert!(arch.has_component(pos));
    assert!(arch.has_component(vel));
}

/// Removing a component migrates back, preserving the other components.
#[test]
fn remove_component_migrates_back() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let vel = world.register_component::<Vel>("Vel");

    let e = world.spawn();
    world.add_component(e, Pos(1.0, 2.0));
    world.add_component(e, Vel(3.0, 4.0));

    assert!(world.remove_component(e, vel));
    assert_eq!(read_pos(&world, e, pos), Pos(1.0, 2.0));

    assert!(world.remove_component(e, pos));
    assert_eq!(world.entity_archetype(e), Some(world.empty_archetype()));
}

/// Removing a component the entity does not carry is a no-op.
#[test]
fn remove_missing_component_is_false() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let e = world.spawn();
    assert!(!world.remove_component(e, pos));
}

/// Despawning in the middle of a swap-removal updates the moved entity's
/// location.
#[test]
fn despawn_middle_row_fixes_moved_entity_location() {
    let mut world = World::new();
    let _pos = world.register_component::<Pos>("Pos");

    let a = world.spawn();
    let b = world.spawn();
    let c = world.spawn();
    world.add_component(a, Pos(1.0, 0.0));
    world.add_component(b, Pos(2.0, 0.0));
    world.add_component(c, Pos(3.0, 0.0));

    // All three share the same archetype.
    assert_eq!(world.entity_archetype(a), world.entity_archetype(c));

    world.despawn(a);

    assert!(!world.is_alive(a));
    assert!(world.is_alive(b));
    assert!(world.is_alive(c));
    // Location of `c` was updated by the swap.
    let loc_c = world.location(c).unwrap();
    let arch = world.archetype(loc_c.archetype);
    assert_eq!(arch.entities()[loc_c.row as usize], c);
}

/// Resources: register, look up, replace.
#[test]
fn resources_roundtrip() {
    #[derive(Debug, PartialEq)]
    struct Time(f32);
    let mut world = World::new();

    let id = world.register_resource(Time(0.5));
    assert_eq!(world.resource::<Time>(id), Ok(&Time(0.5)));
    *world.resource_mut::<Time>(id).unwrap() = Time(1.0);
    assert_eq!(world.resource::<Time>(id), Ok(&Time(1.0)));

    // Re-registering replaces in place.
    let id2 = world.register_resource(Time(2.0));
    assert_eq!(id, id2);
    assert_eq!(world.resource::<Time>(id), Ok(&Time(2.0)));
}

/// Query cache grows with archetypes.
#[test]
fn matching_archetypes_finds_new_archetypes() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let vel = world.register_component::<Vel>("Vel");

    let mut required = ComponentMask::EMPTY;
    required.insert(pos);

    let before = world
        .matching_archetypes(required, ComponentMask::EMPTY)
        .len();

    let e = world.spawn();
    world.add_component(e, Pos(0.0, 0.0));

    let after = world
        .matching_archetypes(required, ComponentMask::EMPTY)
        .len();
    assert!(after > before, "new archetype is matched");

    // The Vel-only archetype is not created yet.
    let mut req_vel = ComponentMask::EMPTY;
    req_vel.insert(vel);
    let e2 = world.spawn();
    world.add_component(e2, Vel(0.0, 0.0));
    let vel_matches = world
        .matching_archetypes(req_vel, ComponentMask::EMPTY)
        .len();
    assert_eq!(vel_matches, 1);
}

/// `ArchetypeId::from_raw(0)` normalizes to `1`, which is the empty
/// archetype in a freshly created world.
#[test]
fn empty_archetype_id_is_one() {
    let world = World::new();
    assert_eq!(world.empty_archetype(), ArchetypeId::from_raw(1));
    assert_eq!(world.empty_archetype(), ArchetypeId::from_raw(0));
}

/// `with_matching_archetypes` hands out a slice for the duration of the
/// callback without holding any borrow on the world afterwards.
#[test]
fn with_matching_archetypes_callback() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let e = world.spawn();
    world.add_component(e, Pos(0.0, 0.0));

    let mut required = ComponentMask::EMPTY;
    required.insert(pos);

    let count = world.with_matching_archetypes(
        required,
        ComponentMask::EMPTY,
        <[ArchetypeId]>::len,
    );
    assert!(count >= 1);

    // World is fully usable again — no outstanding borrow.
    let _ = world.spawn();
}
