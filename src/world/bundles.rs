//! Bundle registration.

use core::any::TypeId;

use crate::archetype::Archetype;
use crate::bundle::Bundle;
use crate::{ArchetypeId, BundleId, ComponentMask, Entity, Location};

use super::{BundleDesc, World, archetype_index};

impl World {
    /// Registers a bundle type and returns its identifier.
    ///
    /// The first call resolves the bundle's component ids, ensures the
    /// archetype exists, and precomputes both the storage-slot mapping and
    /// the arena layout. Later calls are O(1) hash lookups.
    ///
    /// # Panics
    ///
    /// Panics if a component of the bundle is not registered, or if the
    /// bundle count exceeds `u32::MAX`.
    pub fn register_bundle<B: Bundle>(&mut self) -> BundleId {
        let type_id = TypeId::of::<B>();
        if let Some(&bid) = self.bundle_by_type.get(&type_id) {
            return bid;
        }

        let mut ids = Vec::new();
        B::collect_ids(self, &mut ids);

        let mut layout: Vec<(u32, u32)> = Vec::new();
        B::collect_layout(&mut layout);

        let mut mask = ComponentMask::EMPTY;
        for &id in &ids {
            mask.insert(id);
        }
        let archetype = self.find_or_create_archetype(mask);

        let arch = &self.archetypes[archetype_index(archetype)];
        let slots: Box<[usize]> = ids
            .iter()
            .map(|&id| {
                arch.storage_index_of(id).expect(
                    "bundle component absent from archetype: registration bug",
                )
            })
            .collect();

        // Arena layout: per-component byte offsets within the block.
        let mut arena_offsets = Vec::with_capacity(layout.len());
        let mut current: u32 = 0;
        let mut arena_align: u32 = 1;
        for &(size, align) in &layout {
            debug_assert!(align.is_power_of_two());
            current = (current + align - 1) & !(align - 1);
            arena_offsets.push(current);
            current += size;
            arena_align = arena_align.max(align);
        }

        let bid = BundleId::from_raw(
            u32::try_from(self.bundles.len()).expect("bundle id overflow"),
        );
        self.bundles.push(BundleDesc {
            archetype,
            slots,
            arena_offsets: arena_offsets.into_boxed_slice(),
            arena_size: current,
            arena_align,
        });
        self.bundle_by_type.insert(type_id, bid);
        bid
    }

    /// Returns the descriptor for a registered bundle.
    ///
    /// # Panics
    ///
    /// Panics if `bid` was not produced by [`Self::register_bundle`].
    #[must_use]
    pub(crate) fn bundle_desc(&self, bid: BundleId) -> &BundleDesc {
        &self.bundles[bid.index()]
    }

    /// Returns the archetype associated with a registered bundle.
    ///
    /// # Panics
    ///
    /// Panics if `bid` was not produced by [`Self::register_bundle`].
    #[must_use]
    pub fn bundle_archetype(&self, bid: BundleId) -> ArchetypeId {
        self.bundles[bid.index()].archetype
    }

    /// Spawns an entity using a pre-registered bundle.
    ///
    /// # Panics
    ///
    /// Panics if `bid` was not produced by [`Self::register_bundle`].
    pub fn spawn_bundle_id<B: Bundle>(
        &mut self,
        bid: BundleId,
        bundle: B,
    ) -> Entity {
        let entity = self.entities.alloc();
        self.spawn_bundle_reserved_id(bid, entity, bundle);
        entity
    }

    /// Places a reserved entity and writes the bundle's components from a
    /// typed bundle value.
    ///
    /// # Panics
    ///
    /// Panics if `bid` was not produced by [`Self::register_bundle`].
    pub(crate) fn spawn_bundle_reserved_id<B: Bundle>(
        &mut self,
        bid: BundleId,
        entity: Entity,
        bundle: B,
    ) {
        let bundles = &self.bundles;
        let desc = &bundles[bid.index()];
        let archetype = desc.archetype;
        let slots = &desc.slots;

        let archetypes = &mut self.archetypes;
        let arch: &mut Archetype = &mut archetypes[archetype_index(archetype)];

        let row = arch.push_with(entity, |arch, row| {
            // SAFETY: `row` is freshly reserved; `slots` was computed for
            // this bundle against this archetype.
            unsafe { bundle.write_into(arch, row, slots) };
        });
        self.entities
            .set_location(entity, Location { archetype, row });
    }

    /// Places a reserved entity and writes the bundle's components from an
    /// arena block.
    ///
    /// # Safety
    ///
    /// - `entity` must have been reserved via
    ///   [`Self::alloc_reserved_entity`](crate::World) and not yet located.
    /// - `arena_offset` must have been produced by the matching
    ///   `Commands::spawn_bundle` call against `bid`, and `arena` must be
    ///   the arena that call wrote into.
    pub(crate) unsafe fn spawn_bundle_reserved_from_arena(
        &mut self,
        entity: Entity,
        bid: BundleId,
        arena: &[u8],
        arena_offset: usize,
    ) {
        debug_assert!(
            self.entities.location(entity).is_none(),
            "spawn_bundle_reserved_from_arena called on located entity",
        );

        let bundles = &self.bundles;
        let desc = &bundles[bid.index()];
        let archetype = desc.archetype;
        let block_align = desc.arena_align as usize;
        let slots = &desc.slots;
        let offsets = &desc.arena_offsets;
        let base = (arena_offset + block_align - 1) & !(block_align - 1);
        let arena_ptr = arena.as_ptr();

        let arch = &mut self.archetypes[archetype_index(archetype)];
        let row = arch.push_with(entity, |arch, row| {
            for i in 0..slots.len() {
                let off = base + offsets[i] as usize;
                // SAFETY: `off` was computed by the enqueue path; the arena
                // holds a valid value there. `row` is freshly reserved.
                let ptr = unsafe { arena_ptr.add(off) };
                unsafe { arch.write_component_at_slot(slots[i], row, ptr) };
            }
        });
        self.entities
            .set_location(entity, Location { archetype, row });
    }
}
