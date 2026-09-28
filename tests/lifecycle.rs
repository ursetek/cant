//! End-to-end entity lifecycle through the public API.

#![allow(clippy::float_cmp)]

use cant::{QueryMask, World};

#[derive(Debug, PartialEq, Clone, Copy)]
struct Position(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Velocity(f32, f32);

/// A freshly spawned entity lives in the empty archetype until components
/// are added; adding the first component migrates it, removing the last
/// returns it.
#[test]
fn spawn_add_remove_despawn() {
    let mut world = World::new();
    let pos = world.register_component::<Position>("Position");

    let e = world.spawn();
    assert!(world.is_alive(e));
    assert_eq!(world.entity_archetype(e), Some(world.empty_archetype()));

    assert!(world.add_component(e, Position(1.0, 2.0)));
    assert_ne!(world.entity_archetype(e), Some(world.empty_archetype()));
    assert!(
        world
            .archetype(world.location(e).unwrap().archetype)
            .has_component(pos)
    );

    assert!(world.remove_component(e, pos));
    assert_eq!(world.entity_archetype(e), Some(world.empty_archetype()));

    assert!(world.despawn(e));
    assert!(!world.is_alive(e));
    assert_eq!(world.live_count(), 0);
}

/// A read query sees every entity that carries the required component,
/// regardless of what else it carries.
#[test]
fn query_sees_all_matching_archetypes() {
    let mut world = World::new();
    let pos = world.register_component::<Position>("Position");
    let _ = world.register_component::<Velocity>("Velocity");

    let a = world.spawn();
    world.add_component(a, Position(1.0, 0.0));

    let b = world.spawn();
    world.add_component(b, Position(2.0, 0.0));
    world.add_component(b, Velocity(0.0, 1.0));

    let c = world.spawn();
    world.add_component(c, Velocity(0.0, 1.0)); // no Position: filtered out

    let mut mask = QueryMask::new();
    mask.read(pos);
    let mut query = world.prepare(mask).unwrap();

    let mut seen = 0;
    query.for_each(&world, |view| {
        let ps = view.column::<Position>(0).unwrap();
        for p in ps {
            assert!(p.0 == 1.0 || p.0 == 2.0);
        }
        seen += view.len();
    });
    assert_eq!(seen, 2);
}

/// A mutable query writes through; the write is visible to a subsequent
/// read query.
#[test]
fn mutable_query_writes_visible_to_reader() {
    let mut world = World::new();
    let pos = world.register_component::<Position>("Position");

    for i in 0..4 {
        let e = world.spawn();
        #[allow(clippy::cast_precision_loss)]
        world.add_component(e, Position(i as f32, 0.0));
    }

    let mut write_mask = QueryMask::new();
    write_mask.write(pos);
    let mut write_query = world.prepare(write_mask).unwrap();
    write_query.for_each_mut(&mut world, |mut view| {
        let ps = view.column_mut::<Position>(0).unwrap();
        for p in ps {
            p.0 *= 10.0;
        }
    });

    let mut read_mask = QueryMask::new();
    read_mask.read(pos);
    let mut read_query = world.prepare(read_mask).unwrap();
    let mut values = Vec::new();
    read_query.for_each(&world, |view| {
        let ps = view.column::<Position>(0).unwrap();
        for p in ps {
            values.push(p.0);
        }
    });
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(values, vec![0.0, 10.0, 20.0, 30.0]);
}

/// A query obtained before a structural change still finds new archetypes
/// on its next iteration, thanks to version-based refresh.
#[test]
fn query_refreshes_after_new_archetype() {
    let mut world = World::new();
    let pos = world.register_component::<Position>("Position");
    let _ = world.register_component::<Velocity>("Velocity");

    let a = world.spawn();
    world.add_component(a, Position(0.0, 0.0));

    let mut mask = QueryMask::new();
    mask.read(pos);
    let mut query = world.prepare(mask).unwrap();

    let mut before = 0;
    query.for_each(&world, |v| before += v.len());
    assert_eq!(before, 1);

    // A second archetype matching the query appears.
    let b = world.spawn();
    world.add_component(b, Position(0.0, 0.0));
    world.add_component(b, Velocity(0.0, 0.0));

    let mut after = 0;
    query.for_each(&world, |v| after += v.len());
    assert_eq!(after, 2, "same query sees the new archetype");
}

/// Swap-removal on despawn keeps the moved entity's location consistent.
#[test]
fn despawn_in_middle_preserves_survivor_locations() {
    let mut world = World::new();
    let _pos = world.register_component::<Position>("Position");

    let a = world.spawn();
    let b = world.spawn();
    let c = world.spawn();
    world.add_component(a, Position(1.0, 0.0));
    world.add_component(b, Position(2.0, 0.0));
    world.add_component(c, Position(3.0, 0.0));

    world.despawn(a);

    for e in [b, c] {
        let loc = world.location(e).expect("survivor alive");
        assert_eq!(
            world.archetype(loc.archetype).entities()[loc.row as usize],
            e
        );
    }
}

/// Two independent worlds never share state.
#[test]
fn independent_worlds() {
    let mut a = World::new();
    let mut b = World::new();
    let _pa = a.register_component::<Position>("Position");
    let _pb = b.register_component::<Position>("Position");

    let ea = a.spawn();
    let eb = b.spawn();
    assert_eq!(ea.index(), eb.index(), "slots are independent");
    assert_eq!(a.live_count(), 1);
    assert_eq!(b.live_count(), 1);

    assert!(a.despawn(ea));
    assert_eq!(b.live_count(), 1);
}
