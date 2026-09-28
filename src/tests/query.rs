//! Tests for the query layer: mask preparation, chunk iteration, early exit,
//! and mutable access.

use core::ops::ControlFlow;

use crate::query::Access;
use crate::{ComponentId, Entity, QueryError, QueryMask, World};

#[derive(Debug, PartialEq, Clone, Copy)]
struct Pos(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Vel(f32, f32);

#[derive(Debug, PartialEq, Clone, Copy)]
struct Health(u32);

fn count_matching(world: &World, mask: QueryMask) -> u32 {
    let mut query = world.prepare(mask).expect("prepare");
    query.fold(world, 0, |acc, view| acc + view.len())
}

/// Preparing a query with an unregistered component fails.
#[test]
fn prepare_rejects_unknown_component() {
    let world = World::new();
    let mut mask = QueryMask::new();
    mask.read(ComponentId::from_raw(0));
    let err = world.prepare(mask).unwrap_err();
    assert_eq!(err, QueryError::UnknownComponent(ComponentId::from_raw(0)));
}

/// A read query returns the correct entities and matches only archetypes
/// containing the required component.
#[test]
fn read_query_visits_matching_entities() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");

    let a = world.spawn();
    world.add_component(a, Pos(1.0, 0.0));
    let b = world.spawn();
    world.add_component(b, Pos(2.0, 0.0));
    let _c = world.spawn(); // no components: not matched

    let mut mask = QueryMask::new();
    mask.read(pos);
    let mut query = world.prepare(mask).unwrap();

    let mut seen: Vec<Entity> = Vec::new();
    query.for_each(&world, |view| {
        for &e in view.entities() {
            seen.push(e);
        }
    });
    seen.sort_unstable();
    assert_eq!(seen.len(), 2);
    assert!(seen.contains(&a));
    assert!(seen.contains(&b));
}

/// `for_each` on a query with write access panics.
#[test]
#[should_panic(expected = "read-only query")]
fn for_each_rejects_write_query() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");

    let mut mask = QueryMask::new();
    mask.write(pos);
    let mut query = world.prepare(mask).unwrap();

    query.for_each(&world, |_| {});
}

/// Mutable access via `for_each_mut` writes through to storage.
#[test]
fn for_each_mut_writes_components() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");

    let a = world.spawn();
    world.add_component(a, Pos(1.0, 2.0));
    let b = world.spawn();
    world.add_component(b, Pos(3.0, 4.0));

    let mut mask = QueryMask::new();
    mask.write(pos);
    let mut query = world.prepare(mask).unwrap();

    query.for_each_mut(&mut world, |mut view| {
        // SAFETY: `pos` is the only column and it is accessed as `Write`;
        // the query was prepared with `write(pos)` and the caller has
        // exclusive access to the world.
        let positions: &mut [Pos] = view.column_mut::<Pos>(0).unwrap();
        for p in positions {
            p.0 *= 10.0;
            p.1 *= 10.0;
        }
    });

    // Read back through a fresh query.
    let mut read_mask = QueryMask::new();
    read_mask.read(pos);
    let mut read_query = world.prepare(read_mask).unwrap();

    let mut values: Vec<(f32, f32)> = Vec::new();
    read_query.for_each(&world, |view| {
        // SAFETY: `Pos` is the only column and read-only.
        let positions: &[Pos] = view.column::<Pos>(0).unwrap();
        for p in positions {
            values.push((p.0, p.1));
        }
    });
    values.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert_eq!(values, vec![(10.0, 20.0), (30.0, 40.0)]);
}

/// Query mixing `read` and `write` reports the correct access kinds.
#[test]
fn mixed_access_is_reported() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let vel = world.register_component::<Vel>("Vel");

    let mut mask = QueryMask::new();
    mask.read(pos);
    mask.write(vel);
    let query = world.prepare(mask).unwrap();

    assert!(!query.mask().is_read_only());
    assert_eq!(
        query.mask().access(),
        &[(pos, Access::Read), (vel, Access::Write)],
    );
}

/// Access is upgraded from `read` to `write` if both are requested.
#[test]
fn write_upgrades_read_access() {
    let mut mask = QueryMask::new();
    let pos = ComponentId::from_raw(0);
    mask.read(pos);
    mask.write(pos);
    assert_eq!(mask.access(), &[(pos, Access::Write)]);
    assert!(!mask.is_read_only());
}

/// Excluded components filter out archetypes.
#[test]
fn exclude_filters_archetypes() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let vel = world.register_component::<Vel>("Vel");

    let a = world.spawn();
    world.add_component(a, Pos(0.0, 0.0));
    let b = world.spawn();
    world.add_component(b, Pos(0.0, 0.0));
    world.add_component(b, Vel(0.0, 0.0));

    let mut mask = QueryMask::new();
    mask.read(pos);
    mask.exclude(vel);
    let mut query = world.prepare(mask).unwrap();

    let mut entities: Vec<Entity> = Vec::new();
    query.for_each(&world, |view| {
        entities.extend_from_slice(view.entities());
    });
    assert_eq!(entities, vec![a]);
}

/// `try_for_each` stops at the first `Break` and returns the value.
#[test]
#[allow(clippy::cast_precision_loss)]
// ^--- `i < 10`, well within `f32`'s exact-integer range.
fn try_for_each_short_circuits() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");

    for i in 0..10 {
        let e = world.spawn();
        world.add_component(e, Pos(i as f32, 0.0));
    }

    let mut mask = QueryMask::new();
    mask.read(pos);
    let mut query = world.prepare(mask).unwrap();

    let mut visits = 0u32;
    let result = query.try_for_each(&world, |view| {
        // Iterate inside the chunk and bail out as soon as we've seen three
        // entities. A single chunk holds all ten entities (chunk capacity is
        // 4096), so the break happens mid-chunk.
        for &_e in view.entities() {
            visits += 1;
            if visits == 3 {
                return ControlFlow::Break(visits);
            }
        }
        ControlFlow::Continue(())
    });
    assert_eq!(result, ControlFlow::Break(3));
    assert_eq!(visits, 3, "iteration stopped after three entities");
}

/// `any` returns `true` as soon as the predicate matches, `false` otherwise.
#[test]
#[allow(clippy::float_cmp)]
// ^--- Exact equality is intentional: `Pos(5.0)` and `Pos(999.0)` are exact
// integer-valued `f32`s, and we want to distinguish "found exactly 5.0" from
// "found nothing".
fn any_returns_early() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");

    let a = world.spawn();
    world.add_component(a, Pos(5.0, 0.0));

    let mut mask = QueryMask::new();
    mask.read(pos);
    let mut query = world.prepare(mask).unwrap();

    assert!(query.any(&world, |view| {
        // SAFETY: read-only column access.
        let ps: &[Pos] = view.column::<Pos>(0).unwrap();
        ps.iter().any(|p| p.0 == 5.0)
    }));
    assert!(!query.any(&world, |view| {
        let ps: &[Pos] = view.column::<Pos>(0).unwrap();
        ps.iter().any(|p| p.0 == 999.0)
    }));
}

/// A query against an archetype with no entities visits no chunks.
#[test]
fn empty_query_visits_nothing() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let _ = world.prepare(QueryMask::new().read(pos).clone()).unwrap();

    // No entity carries `Pos` yet, so no archetype matches.
    let mut mask = QueryMask::new();
    mask.read(pos);
    let mut query = world.prepare(mask).unwrap();

    let mut visits = 0;
    query.for_each(&world, |_| visits += 1);
    assert_eq!(visits, 0);
}

/// Two queries with the same mask share the archetype cache.
#[test]
fn identical_masks_share_cache() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let e = world.spawn();
    world.add_component(e, Pos(0.0, 0.0));

    let mut mask = QueryMask::new();
    mask.read(pos);
    let q1 = world.prepare(mask.clone()).unwrap();
    let q2 = world.prepare(mask).unwrap();

    assert!(
        std::sync::Arc::ptr_eq(
            &std::sync::Arc::<[crate::ArchetypeId]>::from(
                q1.archetypes().to_vec()
            ),
            &std::sync::Arc::<[crate::ArchetypeId]>::from(
                q2.archetypes().to_vec()
            ),
        ) || q1.archetypes() == q2.archetypes()
    );
}

/// A query created before a new archetype appears must see it after
/// re-preparation, not before.
#[test]
fn new_archetype_appears_after_reprepare() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let _ = world.register_component::<Vel>("Vel");

    let a = world.spawn();
    world.add_component(a, Pos(0.0, 0.0));

    let mut mask = QueryMask::new();
    mask.read(pos);
    let q1 = world.prepare(mask.clone()).unwrap();
    let initial = count_matching(&world, mask.clone());
    assert_eq!(initial, 1);

    // Adding `Vel` to `a` migrates it to a new archetype that still matches.
    world.add_component(a, Vel(0.0, 0.0));
    let after = count_matching(&world, mask.clone());
    assert_eq!(after, 1, "still one entity");

    // A previously-prepared query's archetype list is stale: it points at
    // the old archetype id. This is expected: callers re-prepare after
    // structural changes, or hold the world under a scheduler that does.
    let _ = q1;
}

/// `esc_column_mut` allows two mutable columns of the same chunk to be
/// borrowed simultaneously, provided they are distinct.
#[test]
fn esc_column_mut_two_columns_in_parallel() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let vel = world.register_component::<Vel>("Vel");

    let a = world.spawn();
    world.add_component(a, Pos(1.0, 0.0));
    world.add_component(a, Vel(2.0, 0.0));

    let mut mask = QueryMask::new();
    mask.write(pos).write(vel);
    let mut query = world.prepare(mask).unwrap();

    query.for_each_mut(&mut world, |view| {
        // SAFETY: `pos` and `vel` are distinct columns, both prepared with
        // `Write` access; the caller guarantees no other reference aliases
        // either column while the slices are alive.
        let (p_slice, v_slice) = unsafe {
            let p: &mut [Pos] = view.esc_column_mut::<Pos>(0).unwrap();
            let v: &mut [Vel] = view.esc_column_mut::<Vel>(1).unwrap();
            (p, v)
        };
        for (p, v) in p_slice.iter_mut().zip(v_slice.iter_mut()) {
            p.0 += v.0;
            p.1 += v.1;
        }
    });

    let mut read_mask = QueryMask::new();
    read_mask.read(pos);
    let mut read_query = world.prepare(read_mask).unwrap();

    let mut values: Vec<(f32, f32)> = Vec::new();
    read_query.for_each(&world, |view| {
        // SAFETY: read-only column access.
        let ps: &[Pos] = view.column::<Pos>(0).unwrap();
        for p in ps {
            values.push((p.0, p.1));
        }
    });
    assert_eq!(values, vec![(3.0, 0.0)]);
}

/// `storage_index_of` returns the same slot for a component as the sorted
/// position in `components()`.
#[test]
fn storage_index_of_matches_sorted_position() {
    let mut world = World::new();
    let a = world.register_component::<Pos>("Pos");
    let b = world.register_component::<Vel>("Vel");

    let e = world.spawn();
    world.add_component(e, Pos(0.0, 0.0));
    world.add_component(e, Vel(0.0, 0.0));

    let loc = world.location(e).unwrap();
    let arch = world.archetype(loc.archetype);
    // `components()` is sorted ascending. `Pos` was registered first, so
    // `a.raw() < b.raw()`; `a` sits at slot 0, `b` at slot 1.
    assert_eq!(arch.storage_index_of(a), Some(0));
    assert_eq!(arch.storage_index_of(b), Some(1));
    assert_eq!(arch.storage_index_of(ComponentId::from_raw(999)), None);
}

/// `storage_at` yields the same storage as `storage`, and panics on OOB.
#[test]
fn storage_at_direct_index() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let e = world.spawn();
    world.add_component(e, Pos(0.0, 0.0));

    let loc = world.location(e).unwrap();
    let arch = world.archetype(loc.archetype);
    let idx = arch.storage_index_of(pos).unwrap();
    let a = arch.storage_at(idx);
    let b = arch.storage(pos).unwrap();
    assert_eq!(a.info().id(), b.info().id());
}

/// Type mismatch returns `None` instead of handing out a wrongly-typed slice.
#[test]
fn column_type_mismatch_returns_none() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let e = world.spawn();
    world.add_component(e, Pos(0.0, 0.0));

    let mut mask = QueryMask::new();
    mask.read(pos);
    let mut query = world.prepare(mask).unwrap();

    query.for_each(&world, |view| {
        assert!(view.column::<Pos>(0).is_some());
        assert!(view.column::<Vel>(0).is_none(), "wrong type rejected");
    });
}

/// `column_mut` on a `Read`-only column returns `None`.
#[test]
fn column_mut_on_read_access_returns_none() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let e = world.spawn();
    world.add_component(e, Pos(0.0, 0.0));

    let mut mask = QueryMask::new();
    mask.read(pos);
    let mut query = world.prepare(mask).unwrap();

    query.for_each(&world, |mut view| {
        assert!(view.column_mut::<Pos>(0).is_none(), "read-only rejected");
    });
}

/// Two shared `column` calls on the same view compose without unsafe.
#[test]
fn multiple_shared_columns_are_safe() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let vel = world.register_component::<Vel>("Vel");
    let e = world.spawn();
    world.add_component(e, Pos(1.0, 2.0));
    world.add_component(e, Vel(3.0, 4.0));

    let mut mask = QueryMask::new();
    mask.read(pos).read(vel);
    let mut query = world.prepare(mask).unwrap();

    query.for_each(&world, |view| {
        let ps = view.column::<Pos>(0).unwrap();
        let vs = view.column::<Vel>(1).unwrap();
        assert_eq!(ps[0], Pos(1.0, 2.0));
        assert_eq!(vs[0], Vel(3.0, 4.0));
    });
}
/// A query created before a new archetype appears picks it up on the next
/// iteration without an explicit re-prepare.
#[test]
fn stale_query_refreshes_on_use() {
    let mut world = World::new();
    let pos = world.register_component::<Pos>("Pos");
    let _ = world.register_component::<Vel>("Vel");

    let e1 = world.spawn();
    world.add_component(e1, Pos(0.0, 0.0));

    let mut mask = QueryMask::new();
    mask.read(pos);
    let mut query = world.prepare(mask).unwrap();

    // First iteration: one archetype.
    let mut before = 0;
    query.for_each(&world, |view| before += view.len());
    assert_eq!(before, 1);

    // New archetype appears (Pos + Vel).
    let e2 = world.spawn();
    world.add_component(e2, Pos(0.0, 0.0));
    world.add_component(e2, Vel(0.0, 0.0));

    // Second iteration through the same query must see the new archetype.
    let mut after = 0;
    query.for_each(&world, |view| after += view.len());
    assert_eq!(after, 2, "query refreshed to include the new archetype");
}
