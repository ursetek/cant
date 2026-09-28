//! Resources: registration, typed access, mutation through systems.

#![allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cant::{Access, AccessList, FnSystem, ResourceError, Schedule, World};

#[derive(Debug, PartialEq)]
struct Time {
    dt: f32,
}

#[derive(Debug, PartialEq)]
struct Score(u32);

/// Register, read, mutate.
#[test]
fn resource_roundtrip() {
    let mut world = World::new();
    let id = world.register_resource(Time { dt: 0.016 });
    assert_eq!(world.resource::<Time>(id), Ok(&Time { dt: 0.016 }));
    world.resource_mut::<Time>(id).unwrap().dt = 0.033;
    assert_eq!(world.resource::<Time>(id), Ok(&Time { dt: 0.033 }));
}

/// Re-registering the same type replaces the value but keeps the id.
#[test]
fn reregister_replaces_in_place() {
    let mut world = World::new();
    let a = world.register_resource(Time { dt: 1.0 });
    let b = world.register_resource(Time { dt: 2.0 });
    assert_eq!(a, b);
    assert_eq!(world.resource::<Time>(a), Ok(&Time { dt: 2.0 }));
}

/// Type mismatch is reported, not silently ignored.
#[test]
fn type_mismatch_reports_error() {
    let mut world = World::new();
    let id = world.register_resource(Time { dt: 0.0 });
    assert_eq!(
        world.resource::<Score>(id),
        Err(ResourceError::TypeMismatch)
    );
}

/// Unknown id is reported.
#[test]
fn unknown_resource_reports_error() {
    let world = World::new();
    assert_eq!(
        world.resource::<Time>(cant::ResourceId::from_raw(0)),
        Err(ResourceError::NotRegistered),
    );
}

/// A system reads and writes a resource through the shared cell.
#[test]
fn system_mutates_resource() {
    let score = Arc::new(AtomicU32::new(0));

    let mut schedule = Schedule::new();
    let mut access = AccessList::new();

    let mut world = World::new();
    let time_id = world.register_resource(Time { dt: 1.0 });
    let score_id = world.register_resource(Score(0));

    access.add_resource(time_id, Access::Read);
    access.add_resource(score_id, Access::Write);

    let score_for_closure = Arc::clone(&score);
    schedule.add_system(FnSystem::new("tick", access, move |cell, _| {
        // SAFETY: the schedule guarantees no other system with conflicting
        // access runs concurrently; both resource accesses are unique here.
        let time: &Time = unsafe { cell.resource::<Time>(time_id) }.unwrap();
        let s: &mut Score =
            unsafe { cell.resource_mut::<Score>(score_id) }.unwrap();
        s.0 += time.dt as u32;
        score_for_closure.store(s.0, Ordering::Relaxed);
    }));

    schedule.run(&mut world);

    assert_eq!(world.resource::<Score>(score_id), Ok(&Score(1)));
    assert_eq!(score.load(Ordering::Relaxed), 1);
}

/// Two systems reading the same resource can share a batch.
#[test]
fn readers_share_batch() {
    let mut world = World::new();
    let id = world.register_resource(Time { dt: 0.5 });

    let mut access = AccessList::new();
    access.add_resource(id, Access::Read);

    let mut schedule = Schedule::new();
    schedule.add_system(FnSystem::new("a", access.clone(), |_, _| {}));
    schedule.add_system(FnSystem::new("b", access, |_, _| {}));
    schedule.build();
    assert_eq!(schedule.batch_count(), 1);
    schedule.run(&mut world);
}

/// A writer forces its own batch.
#[test]
fn writer_splits_batch() {
    let mut world = World::new();
    let id = world.register_resource(Time { dt: 0.5 });

    let mut reader = AccessList::new();
    reader.add_resource(id, Access::Read);
    let mut writer = AccessList::new();
    writer.add_resource(id, Access::Write);

    let mut schedule = Schedule::new();
    schedule.add_system(FnSystem::new("reader", reader, |_, _| {}));
    schedule.add_system(FnSystem::new("writer", writer, |_, _| {}));
    schedule.build();
    assert_eq!(schedule.batch_count(), 2);
    schedule.run(&mut world);
}
