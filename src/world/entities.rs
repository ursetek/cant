//! Entity lifecycle.

use crate::archetype::Archetype;
use crate::{ArchetypeId, ComponentMask, Entity, Location};

use super::{World, archetype_index};

impl World {
    /// Spawns an entity with no components.
    ///
    /// The entity lives in the empty archetype until components are added.
    pub fn spawn(&mut self) -> Entity {
        let entity = self.entities.alloc();
        self.spawn_reserved(entity);
        entity
    }

    /// Spawns an entity with the given component mask, invoking `init` to
    /// write each component slot.
    ///
    /// `init` receives the destination archetype and the row index assigned
    /// to the entity. It must initialize every component slot of that row,
    /// using [`Archetype::write_raw`].
    pub fn spawn_with<F>(&mut self, mask: ComponentMask, init: F) -> Entity
    where
        F: FnOnce(&mut Archetype, u32),
    {
        let entity = self.entities.alloc();
        let arch_id = self.find_or_create_archetype(mask);
        let arch = &mut self.archetypes[archetype_index(arch_id)];
        let row = arch.push_with(entity, init);
        self.entities.set_location(
            entity,
            Location {
                archetype: arch_id,
                row,
            },
        );
        entity
    }

    /// Despawns an entity, dropping all its components.
    ///
    /// Returns `false` if the descriptor is stale or the entity was already
    /// dead.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        let Some(loc) = self.entities.location(entity) else {
            return false;
        };

        let arch = &mut self.archetypes[archetype_index(loc.archetype)];
        let result = arch.swap_remove(loc.row);

        if let Some(moved) = result.moved {
            self.entities.set_location(
                moved,
                Location {
                    archetype: loc.archetype,
                    row: loc.row,
                },
            );
        }
        self.entities.free(entity);
        true
    }

    /// Returns `true` if the descriptor matches a live entity.
    #[must_use]
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.is_alive(entity)
    }

    /// Returns the location of the entity inside its archetype.
    #[must_use]
    pub fn location(&self, entity: Entity) -> Option<Location> {
        self.entities.location(entity)
    }

    /// Number of live entities.
    #[must_use]
    pub fn live_count(&self) -> u32 {
        self.entities.live_count()
    }

    /// Iterates over all live entities in slot order.
    pub fn entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.entities.iter_alive()
    }

    /// Returns the archetype currently owning the entity, if it is alive.
    #[must_use]
    pub fn entity_archetype(&self, entity: Entity) -> Option<ArchetypeId> {
        self.entities.location(entity).map(|l| l.archetype)
    }

    /// Reserves an entity slot without placing it in any archetype.
    ///
    /// Used by [`Commands::spawn`](crate::Commands::spawn). The slot is
    /// allocated and the generation is bumped as usual, but the entity's
    /// `Location` stays `None` until [`Self::spawn_reserved`] runs. In this
    /// state the entity is not visible to queries and `is_alive` returns
    /// `false`.
    ///
    /// # Panics
    ///
    /// Panics if the entity table is exhausted.
    pub(crate) fn alloc_reserved_entity(&mut self) -> Entity {
        self.entities.alloc()
    }

    /// Places a previously reserved entity into the empty archetype.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the entity was already located.
    pub(crate) fn spawn_reserved(&mut self, entity: Entity) {
        debug_assert!(
            self.entities.location(entity).is_none(),
            "spawn_reserved called on an already-located entity",
        );
        let arch_id = self.empty_archetype;
        let arch = &mut self.archetypes[archetype_index(arch_id)];
        let row = arch.push_with(entity, |_, _| {});
        self.entities.set_location(
            entity,
            Location {
                archetype: arch_id,
                row,
            },
        );
    }

    /// Spawns an entity with the given bundle.
    ///
    /// Equivalent to `register_bundle::<B>()` followed by `spawn_bundle_id`,
    /// except the bundle id is looked up (or created) on the first call.
    ///
    /// # Panics
    ///
    /// Panics if any component of the bundle was not registered.
    pub fn spawn_bundle<B: crate::bundle::Bundle>(
        &mut self,
        bundle: B,
    ) -> Entity {
        let bid = self.register_bundle::<B>();
        self.spawn_bundle_id(bid, bundle)
    }
}
