//! Tests for the system layer: access lists, closure systems, query systems.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::query::UnsafeWorldCell;
use crate::system::{AccessList, FnSystem, QuerySystem, System};
use crate::{ComponentId, QueryMask, ResourceId, World};

use crate::query::Access;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pos(f32, f32);

/// Two write accesses to the same component conflict.
#[test]
fn write_write_conflicts() {
    let c = ComponentId::from_raw(0);
    let mut a = AccessList::new();
    a.add_component(c, Access::Write);
    let mut b = AccessList::new();
    b.add_component(c, Access::Write);
    assert!(a.conflicts_with(&b));
}

/// Two read accesses to the same component do not conflict.
#[test]
fn read_read_no_conflict() {
    let c = ComponentId::from_raw(0);
    let mut a = AccessList::new();
    a.add_component(c, Access::Read);
    let mut b = AccessList::new();
    b.add_component(c, Access::Read);
    assert!(!a.conflicts_with(&b));
}

/// Read + write to the same component conflicts in either direction.
#[test]
fn read_write_conflicts() {
    let c = ComponentId::from_raw(0);
    let mut a = AccessList::new();
    a.add_component(c, Access::Read);
    let mut b = AccessList::new();
    b.add_component(c, Access::Write);
    assert!(a.conflicts_with(&b));
    assert!(b.conflicts_with(&a));
}

/// Access to different components never conflicts.
#[test]
fn different_components_no_conflict() {
    let mut a = AccessList::new();
    a.add_component(ComponentId::from_raw(0), Access::Write);
    let mut b = AccessList::new();
    b.add_component(ComponentId::from_raw(1), Access::Write);
    assert!(!a.conflicts_with(&b));
}

/// Resource accesses follow the same rules as component accesses.
#[test]
fn resource_conflicts() {
    let r = ResourceId::from_raw(0);
    let mut a = AccessList::new();
    a.add_resource(r, Access::Write);
    let mut b = AccessList::new();
    b.add_resource(r, Access::Read);
    assert!(a.conflicts_with(&b));

    let mut c = AccessList::new();
    c.add_resource(r, Access::Read);
    assert!(!b.conflicts_with(&c));
}

/// `add_component` upgrades a prior `Read` to `Write`.
#[test]
fn add_component_upgrades_to_write() {
    let c = ComponentId::from_raw(0);
    let mut a = AccessList::new();
    a.add_component(c, Access::Read);
    a.add_component(c, Access::Write);
    assert_eq!(a.components(), &[(c, Access::Write)]);
}

/// `FnSystem` runs its closure and reports name and access.
#[test]
fn fn_system_runs() {
    let mut world = World::new();
    let raw: *mut World = core::ptr::from_mut(&mut world);

    // SAFETY: unique access.
    let mut commands = unsafe { crate::Commands::from_raw(raw, true) };
    let cell = UnsafeWorldCell::new(&world);

    let counter = Arc::new(AtomicU32::new(0));
    let counter_for_closure = Arc::clone(&counter);

    let mut access = AccessList::new();
    access.add_resource(ResourceId::from_raw(0), Access::Read);

    let mut sys = FnSystem::new("counter", access, move |_, _| {
        counter_for_closure.fetch_add(1, Ordering::SeqCst);
    });

    assert_eq!(sys.name(), "counter");
    assert_eq!(sys.access().resources().len(), 1);

    sys.run(cell, &mut commands);

    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

/// `QuerySystem` derives its access list from the query and iterates chunks.
#[test]
fn query_system_iterates() {
    let mut world = World::new();
    let raw: *mut World = core::ptr::from_mut(&mut world);

    // SAFETY: unique access.
    let mut commands = unsafe { crate::Commands::from_raw(raw, true) };

    let pos = world.register_component::<Pos>("Pos");
    for i in 0..5 {
        let e = world.spawn();
        // `i < 5`, exact in f32.
        #[allow(clippy::cast_precision_loss)]
        world.add_component(e, Pos(i as f32, 0.0));
    }
    let cell = UnsafeWorldCell::new(&world);

    let mut mask = QueryMask::new();
    mask.write(pos);
    let query = world.prepare(mask).unwrap();

    let total = Arc::new(AtomicU32::new(0));
    let total_for_closure = Arc::clone(&total);

    let mut sys = QuerySystem::new("collect", query, move |view| {
        total_for_closure.fetch_add(view.len(), Ordering::Relaxed);
    });

    assert_eq!(sys.name(), "collect");
    assert_eq!(sys.access().components(), &[(pos, Access::Write)]);

    sys.run(cell, &mut commands);

    assert_eq!(total.load(Ordering::Relaxed), 5);
}
