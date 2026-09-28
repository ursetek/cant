//! Entity descriptors, slot allocator, and per-entity metadata.
//!
//! An [`Entity`] is a packed `(index, generation)` pair stored in a
//! `NonZeroU64`, so `Option<Entity>` remains 8 bytes. Slots are reused after
//! [`Entities::free`], and the generation counter invalidates stale
//! descriptors.

use core::fmt;
use core::num::NonZeroU64;

use crate::ArchetypeId;

/// Entity descriptor.
///
/// Packing: `generation` occupies the high 32 bits, `index` the low 32.
/// `generation >= 1` is enforced, so the whole 64-bit word is non-zero and
/// `Option<Entity>` occupies 8 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Entity(NonZeroU64);

impl Entity {
    /// Builds an entity from `index` and `generation`.
    ///
    /// Generation `0` is reserved and silently normalized to `1`.
    #[inline]
    #[must_use]
    pub const fn from_parts(index: u32, generation: u32) -> Self {
        let g = if generation == 0 { 1 } else { generation };
        let raw = ((g as u64) << 32) | (index as u64);
        // SAFETY: `g >= 1`, therefore `raw != 0`.
        Self(unsafe { NonZeroU64::new_unchecked(raw) })
    }

    /// Returns the slot index.
    #[inline]
    #[must_use]
    pub const fn index(self) -> u32 {
        (self.0.get() & 0xFFFF_FFFF) as u32
    }

    /// Returns the generation counter.
    #[inline]
    #[must_use]
    pub const fn generation(self) -> u32 {
        (self.0.get() >> 32) as u32
    }

    /// Packs the entity into a `u64`.
    #[inline]
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        self.0.get()
    }

    /// Unpacks an entity from a `u64` previously obtained via [`Self::to_bits`].
    ///
    /// Returns `None` if `bits == 0`.
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u64) -> Option<Self> {
        match NonZeroU64::new(bits) {
            Some(x) => Some(Self(x)),
            None => None,
        }
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Entity({}v{})", self.index(), self.generation())
    }
}

/// Location of an entity inside an archetype.
///
/// `ArchetypeId` is backed by `NonZeroU32`, which gives this struct a niche:
/// `Option<Location>` occupies the same 8 bytes as `Location`, and
/// [`EntityMeta`] fits in 12 bytes instead of 16. This is load-bearing: at
/// one million entities, the difference is 4 MiB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    /// Owning archetype.
    pub archetype: ArchetypeId,
    /// Global row inside the archetype. Rows are continuous across chunks;
    /// the chunk is derived as `row / chunk_capacity`.
    pub row: u32,
}

/// Metadata for a single entity slot.
#[derive(Clone, Copy, Debug)]
pub struct EntityMeta {
    /// Current generation of the slot.
    pub generation: u32,
    /// `None` if the slot is vacant.
    pub location: Option<Location>,
}

/// Entity allocator plus per-slot metadata table.
///
/// - `meta` is a dense `Vec<EntityMeta>` indexed by [`Entity::index`].
/// - `free` is a LIFO list of vacant slot indices.
/// - `live` tracks the number of occupied slots.
#[derive(Debug, Default)]
pub struct Entities {
    meta: Vec<EntityMeta>,
    free: Vec<u32>,
    live: u32,
}

impl Entities {
    /// Creates an empty allocator.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            meta: Vec::new(),
            free: Vec::new(),
            live: 0,
        }
    }

    /// Allocates a new entity.
    ///
    /// The `location` field is left as `None`; the caller is expected to
    /// assign it via [`Self::set_location`].
    ///
    /// # Panics
    ///
    /// Panics if the total number of slots exceeds `u32::MAX`.
    #[inline]
    #[must_use]
    pub fn alloc(&mut self) -> Entity {
        let entity = if let Some(index) = self.free.pop() {
            let meta = &mut self.meta[index as usize];
            debug_assert!(
                meta.location.is_none(),
                "free-list slot points to a live entity",
            );
            Entity::from_parts(index, meta.generation)
        } else {
            let index =
                u32::try_from(self.meta.len()).expect("entity index overflow");
            self.meta.push(EntityMeta {
                generation: 1,
                location: None,
            });
            Entity::from_parts(index, 1)
        };
        self.live += 1;
        entity
    }

    /// Frees the entity, returning its previous location.
    ///
    /// Returns `None` if the entity was already dead or if the generation
    /// does not match.
    #[inline]
    pub fn free(&mut self, entity: Entity) -> Option<Location> {
        let meta = self.meta.get_mut(entity.index() as usize)?;
        if meta.generation != entity.generation() {
            return None;
        }
        let loc = meta.location.take()?;

        let mut next = meta.generation.wrapping_add(1);
        if next == 0 {
            next = 1;
        }
        meta.generation = next;

        self.free.push(entity.index());
        self.live -= 1;
        Some(loc)
    }

    /// Returns `true` if the descriptor matches a live slot.
    #[inline]
    #[must_use]
    pub fn is_alive(&self, entity: Entity) -> bool {
        match self.meta.get(entity.index() as usize) {
            Some(meta) => {
                meta.generation == entity.generation()
                    && meta.location.is_some()
            }
            None => false,
        }
    }

    /// Returns the current location of the entity, if alive.
    #[inline]
    #[must_use]
    pub fn location(&self, entity: Entity) -> Option<Location> {
        let meta = self.meta.get(entity.index() as usize)?;
        if meta.generation != entity.generation() {
            return None;
        }
        meta.location
    }

    /// Assigns a new location to a live entity.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the entity generation does not match the
    /// slot generation.
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn set_location(&mut self, entity: Entity, loc: Location) {
        let meta = &mut self.meta[entity.index() as usize];
        debug_assert_eq!(meta.generation, entity.generation());
        meta.location = Some(loc);
    }

    /// Clears the location of the entity without releasing the slot.
    ///
    /// Returns the previous location, or `None` if the slot is already vacant
    /// or the generation does not match.
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn clear_location(
        &mut self,
        entity: Entity,
    ) -> Option<Location> {
        let meta = self.meta.get_mut(entity.index() as usize)?;
        if meta.generation != entity.generation() {
            return None;
        }
        meta.location.take()
    }

    /// Number of currently live entities.
    #[allow(clippy::inline_always)]
    #[inline(always)]
    #[must_use]
    pub fn live_count(&self) -> u32 {
        self.live
    }

    /// Iterates over all live entities in slot order.
    pub fn iter_alive(&self) -> impl Iterator<Item = Entity> + '_ {
        self.meta.iter().enumerate().filter_map(|(i, meta)| {
            meta.location?;
            // `i < meta.len() <= u32::MAX`, so the cast is lossless.
            #[allow(clippy::cast_possible_truncation)]
            Some(Entity::from_parts(i as u32, meta.generation))
        })
    }

    /// Allocates a new entity and records its initial location in one write.
    ///
    /// Equivalent to [`Self::alloc`] followed by [`Self::set_location`], but
    /// writes the metadata slot exactly once. Used by `World::spawn` and
    /// `World::spawn_bundle_id`, where the target archetype and row are
    /// known before the entity is allocated.
    #[inline]
    #[must_use]
    pub(crate) fn alloc_with_location(&mut self, loc: Location) -> Entity {
        let entity = if let Some(index) = self.free.pop() {
            let meta = &mut self.meta[index as usize];
            debug_assert!(
                meta.location.is_none(),
                "free-list slot points to a live entity",
            );
            meta.location = Some(loc);
            Entity::from_parts(index, meta.generation)
        } else {
            let index =
                u32::try_from(self.meta.len()).expect("entity index overflow");
            self.meta.push(EntityMeta {
                generation: 1,
                location: Some(loc),
            });
            Entity::from_parts(index, 1)
        };
        self.live += 1;
        entity
    }

    /// Reserves capacity for at least `additional` more entity slots.
    ///
    /// Does not touch `free`; the free list only shrinks with `free` calls
    /// and its capacity is amortized separately.
    #[inline]
    pub fn reserve(&mut self, additional: u32) {
        self.meta.reserve(additional as usize);
    }
}
