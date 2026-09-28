//! Tests for entity packing, allocator bookkeeping, and generation bumps.

use crate::entity::EntityMeta;
use crate::{ArchetypeId, Entities, Entity, Location};

/// Bit packing must be lossless for any valid `(index, generation)` pair.
#[test]
fn bit_packing_roundtrip() {
    for (index, generation) in [
        (0u32, 1u32),
        (1, 1),
        (u32::MAX, 1),
        (0, u32::MAX),
        (12345, 67890),
    ] {
        let e = Entity::from_parts(index, generation);
        assert_eq!(e.index(), index, "index survived packing");
        assert_eq!(e.generation(), generation, "generation survived packing");

        let bits = e.to_bits();
        let restored = Entity::from_bits(bits).expect("non-zero bits");
        assert_eq!(restored, e, "round-trip through u64 is lossless");
    }
}

/// Generation `0` is a reserved sentinel; the constructor silently promotes
/// it to `1` so the packed word is never zero.
#[test]
fn generation_zero_is_normalized_to_one() {
    let e = Entity::from_parts(7, 0);
    assert_eq!(e.generation(), 1);
    assert_ne!(e.to_bits(), 0, "packed word must be non-zero");
}

/// `from_bits(0)` is the only way to get a `None` back — this is what makes
/// `Option<Entity>` niche-optimized.
#[test]
fn zero_bits_yield_none() {
    assert!(Entity::from_bits(0).is_none());
}

/// Reusing a slot must invalidate old descriptors. Otherwise stale `Entity`
/// values would silently alias new ones.
#[test]
fn alloc_free_realloc_bumps_generation() {
    let mut entities = Entities::new();

    let first = entities.alloc();
    assert_eq!(first.index(), 0);
    assert_eq!(first.generation(), 1);
    entities.set_location(
        first,
        Location {
            archetype: ArchetypeId::from_raw(0),
            row: 0,
        },
    );
    assert!(entities.is_alive(first));

    let freed = entities.free(first).expect("first free succeeds");
    assert_eq!(freed.archetype, ArchetypeId::from_raw(0));
    assert!(!entities.is_alive(first), "freed descriptor is invalid");

    let second = entities.alloc();
    assert_eq!(second.index(), 0, "slot was reused");
    assert_eq!(second.generation(), 2, "generation was bumped");
    assert_ne!(first, second, "old descriptor does not alias new one");
}

/// `free` must be idempotent: a second call on a dead entity is a no-op and
/// returns `None`. This matters because despawn paths may race with cleanup.
#[test]
fn double_free_is_a_noop() {
    let mut entities = Entities::new();
    let e = entities.alloc();
    entities.set_location(
        e,
        Location {
            archetype: ArchetypeId::from_raw(0),
            row: 0,
        },
    );

    assert!(entities.free(e).is_some());
    assert!(entities.free(e).is_none(), "second free returns None");
    assert_eq!(entities.live_count(), 0, "live count not underflowed");
}

/// `live_count` must track occupancy through alloc/free cycles.
#[test]
fn live_count_tracks_occupancy() {
    let mut entities = Entities::new();
    assert_eq!(entities.live_count(), 0);

    let a = entities.alloc();
    entities.set_location(
        a,
        Location {
            archetype: ArchetypeId::from_raw(0),
            row: 0,
        },
    );
    assert_eq!(entities.live_count(), 1);

    let b = entities.alloc();
    entities.set_location(
        b,
        Location {
            archetype: ArchetypeId::from_raw(0),
            row: 1,
        },
    );
    assert_eq!(entities.live_count(), 2);

    entities.free(a);
    assert_eq!(entities.live_count(), 1);
}

/// `iter_alive` must yield exactly the live slots, in slot order, with the
/// current generation.
#[test]
fn iter_alive_yields_only_live_entities() {
    let mut entities = Entities::new();
    let a = entities.alloc();
    let b = entities.alloc();
    let c = entities.alloc();

    for (i, e) in [a, b, c].iter().enumerate() {
        entities.set_location(
            *e,
            Location {
                archetype: ArchetypeId::from_raw(0),
                #[allow(clippy::cast_possible_truncation)]
                row: i as u32,
            },
        );
    }
    entities.free(b);

    let live: Vec<Entity> = entities.iter_alive().collect();
    assert_eq!(live, vec![a, c], "only a and c remain, in slot order");
}

/// A stale descriptor (same index, older generation) must be rejected even
/// if the slot is currently occupied by a fresh entity.
#[test]
fn stale_descriptor_is_rejected() {
    let mut entities = Entities::new();
    let old = entities.alloc();
    entities.set_location(
        old,
        Location {
            archetype: ArchetypeId::from_raw(0),
            row: 0,
        },
    );
    entities.free(old);

    let new = entities.alloc();
    entities.set_location(
        new,
        Location {
            archetype: ArchetypeId::from_raw(0),
            row: 0,
        },
    );

    assert!(!entities.is_alive(old), "old descriptor rejected");
    assert!(entities.is_alive(new), "new descriptor accepted");
    assert!(entities.location(old).is_none(), "no location for old");
}

/// `Entity` is packed into a `NonZeroU64` so that `Option<Entity>` does not
/// grow. This is a load-bearing assumption: it lets `Option<Entity>` be
/// stored in flat arrays without a separate tag.
#[test]
fn entity_fits_in_eight_bytes() {
    assert_eq!(core::mem::size_of::<Entity>(), 8);
    assert_eq!(core::mem::size_of::<Option<Entity>>(), 8);
}

/// `ArchetypeId` uses `NonZeroU32`, which gives `Location` a niche. In turn,
/// `Option<Location>` needs no separate discriminant, and `EntityMeta` fits
/// in 12 bytes instead of 16. At one million entities, this saves 4 MiB.
#[test]
fn entity_meta_uses_archetype_id_niche() {
    assert_eq!(core::mem::size_of::<ArchetypeId>(), 4);
    assert_eq!(core::mem::size_of::<Option<ArchetypeId>>(), 4);
    assert_eq!(core::mem::size_of::<Location>(), 8);
    assert_eq!(core::mem::size_of::<Option<Location>>(), 8);
    assert_eq!(core::mem::size_of::<EntityMeta>(), 12);
}
