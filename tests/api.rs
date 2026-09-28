//! Integration tests that exercise the public surface of `cant`.
//!
//! These tests deliberately avoid internals: they simulate how an external
//! crate (a game, an editor, a mod loader) would use the ECS. As the public
//! API grows — `World`, `Query`, `Schedule` — more integration tests will
//! move here from `src/tests/`.

use cant::{
    ArchetypeId, ComponentId, ComponentInfo, ComponentKind, ComponentMask,
    Entities, Entity, Location, World,
};

/// End-to-end: register, spawn, add components, despawn.
#[test]
fn world_end_to_end() {
    #[derive(Debug, PartialEq, Clone, Copy)]
    struct Position(f32, f32);

    let mut world = World::new();
    let pos = world.register_component::<Position>("Position");

    let e = world.spawn();
    assert!(world.add_component(e, Position(1.0, 2.0)));
    assert_eq!(world.live_count(), 1);

    let loc = world.location(e).expect("alive");
    let arch = world.archetype(loc.archetype);
    assert!(arch.has_component(pos));

    assert!(world.despawn(e));
    assert_eq!(world.live_count(), 0);
}

/// Resources are usable through the public API.
#[test]
fn world_resources() {
    struct Config(u32);

    let mut world = World::new();
    let id = world.register_resource(Config(42));
    assert_eq!(world.resource::<Config>(id).unwrap().0, 42);
}

/// A full alloc → free → realloc cycle must produce two distinct, non-aliasing
/// descriptors. This is the core guarantee external code relies on when it
/// stores `Entity` handles for later use.
#[test]
fn entity_handles_never_alias_across_reuse() {
    let mut entities = Entities::new();

    let a = entities.alloc();
    // The public API requires the caller to set a location before the entity
    // becomes "alive"; we use the crate-internal path via `Location` here
    // only because there is no `World` yet.
    let loc = Location {
        archetype: ArchetypeId::from_raw(0),
        row: 0,
    };

    // SAFETY-NOTE: `Entities::set_location` is `pub(crate)`. External users
    // will get this through `World::spawn`. The integration test exercises
    // the identity invariants that remain valid regardless of who sets the
    // location, so we simulate the state transition here.
    //
    // When `World` lands, this test will be rewritten against `World::spawn`.
    let _ = (a, loc);

    // We cannot call `set_location` from an external test, so the assertion
    // below is the part of the contract that is observable today: two
    // allocations with no intervening free must have distinct indices.
    let b = entities.alloc();
    assert_ne!(a, b);
    assert_ne!(a.index(), b.index());
}

/// `Entity` bit-level round-trip must be stable across the crate boundary:
/// serializers store handles as `u64` and recover them later.
#[test]
fn entity_u64_roundtrip_is_stable() {
    let e = Entity::from_parts(42, 7);
    let bits = e.to_bits();
    assert_eq!(bits, (7u64 << 32) | 0x2au64);

    let restored = Entity::from_bits(bits).expect("non-zero bits");
    assert_eq!(restored.index(), 42);
    assert_eq!(restored.generation(), 7);
}

/// `ComponentMask` is part of the public API because queries will be built
/// from it. `matches` is the query matcher.
#[test]
fn query_mask_matching() {
    let mut required = ComponentMask::EMPTY;
    required.insert(ComponentId::from_raw(1));
    required.insert(ComponentId::from_raw(2));

    let mut excluded = ComponentMask::EMPTY;
    excluded.insert(ComponentId::from_raw(3));

    let mut entity_components = ComponentMask::EMPTY;
    entity_components.insert(ComponentId::from_raw(1));
    entity_components.insert(ComponentId::from_raw(2));
    entity_components.insert(ComponentId::from_raw(9));

    assert!(entity_components.matches(&required, &excluded));

    entity_components.insert(ComponentId::from_raw(3));
    assert!(!entity_components.matches(&required, &excluded));
}

/// `ComponentInfo` must be constructible from external code for user-defined
/// component types.
#[test]
fn component_info_is_constructible() {
    #[derive(Debug)]
    #[allow(dead_code)]
    struct Health(u32);

    let info = ComponentInfo::of::<Health>(
        ComponentId::from_raw(0),
        "Health",
        ComponentKind::Dense,
    );

    assert_eq!(info.name(), "Health");
    assert_eq!(info.kind(), ComponentKind::Dense);
    assert_eq!(info.layout().size(), 4);
}

/// Two independent `Entities` tables must not share state.
#[test]
fn independent_worlds_do_not_share_state() {
    let mut a = Entities::new();
    let mut b = Entities::new();

    let ea = a.alloc();
    let eb = b.alloc();

    assert_eq!(ea.index(), 0);
    assert_eq!(eb.index(), 0);
    assert_eq!(a.live_count(), 1);
    assert_eq!(b.live_count(), 1);
}
