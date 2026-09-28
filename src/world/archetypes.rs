//! Archetype registry, edge graph, query cache.

use std::sync::Arc;

use crate::archetype::Archetype;
use crate::{ArchetypeId, ComponentId, ComponentMask, MAX_COMPONENTS};

use super::{QueryKey, World, archetype_index};

impl World {
    /// Returns the identifier of the archetype for `mask`, creating it if
    /// necessary.
    ///
    /// Creating an archetype invalidates the query cache.
    pub(crate) fn find_or_create_archetype(
        &mut self,
        mask: ComponentMask,
    ) -> ArchetypeId {
        if let Some(&id) = self.archetype_index.get(&mask) {
            return id;
        }

        let id = ArchetypeId::from_raw(
            u32::try_from(self.archetypes.len() + 1)
                .expect("archetype id overflow"),
        );
        let components: Box<[ComponentId]> = mask.iter().collect();
        let storages = components
            .iter()
            .map(|&c| {
                let info = self.components[c.index()].clone();
                (self.component_factories[c.index()])(info)
            })
            .collect::<Vec<_>>();

        let arch = Archetype::new(id, mask, components, storages);
        self.archetypes.push(arch);
        self.archetype_index.insert(mask, id);
        // Extend both adjacency tables by one row (MAX_COMPONENTS entries).
        self.add_edges
            .resize(self.add_edges.len() + MAX_COMPONENTS, 0);
        self.remove_edges
            .resize(self.remove_edges.len() + MAX_COMPONENTS, 0);
        self.archetype_version = self.archetype_version.wrapping_add(1);
        self.invalidate_query_cache();

        id
    }

    /// Edge in the archetype graph: adding `comp` to `from`.
    pub(crate) fn add_edge(
        &mut self,
        from: ArchetypeId,
        comp: ComponentId,
    ) -> ArchetypeId {
        let from_idx = archetype_index(from);
        let comp_idx = comp.index();
        debug_assert!(
            comp_idx < MAX_COMPONENTS,
            "ComponentId out of mask bounds"
        );
        let slot = from_idx * MAX_COMPONENTS + comp_idx;

        let cached = self.add_edges[slot];
        if cached != 0 {
            return ArchetypeId::from_raw(cached);
        }

        let mut target_mask = *self.archetypes[from_idx].mask();
        target_mask.insert(comp);
        let to = self.find_or_create_archetype(target_mask);
        // `find_or_create_archetype` may have resized the edge tables; the
        // slot index is still valid because `from_idx` did not change.
        self.add_edges[slot] = to.raw();
        to
    }

    /// Edge in the archetype graph: removing `comp` from `from`.
    pub(crate) fn remove_edge(
        &mut self,
        from: ArchetypeId,
        comp: ComponentId,
    ) -> ArchetypeId {
        let from_idx = archetype_index(from);
        let comp_idx = comp.index();
        debug_assert!(
            comp_idx < MAX_COMPONENTS,
            "ComponentId out of mask bounds"
        );
        let slot = from_idx * MAX_COMPONENTS + comp_idx;

        let cached = self.remove_edges[slot];
        if cached != 0 {
            return ArchetypeId::from_raw(cached);
        }

        let mut target_mask = *self.archetypes[from_idx].mask();
        target_mask.remove(comp);
        let to = self.find_or_create_archetype(target_mask);
        self.remove_edges[slot] = to.raw();
        to
    }

    /// Returns the archetype with the given identifier.
    ///
    /// # Panics
    ///
    /// Panics if `id` was not produced by this world.
    #[must_use]
    pub fn archetype(&self, id: ArchetypeId) -> &Archetype {
        &self.archetypes[archetype_index(id)]
    }

    /// Returns the empty archetype, where freshly spawned entities live until
    /// components are added.
    #[must_use]
    pub fn empty_archetype(&self) -> ArchetypeId {
        self.empty_archetype
    }

    /// Number of archetypes created so far.
    ///
    /// # Panics
    ///
    /// Panics if the archetype count exceeds `u32::MAX`. Unreachable: each
    /// archetype holds at least one entity, and entities are `u32`-indexed.
    #[must_use]
    pub fn archetype_count(&self) -> u32 {
        u32::try_from(self.archetypes.len()).expect("archetype count overflow")
    }

    /// Returns the archetypes matching the given query mask as a shared,
    /// reference-counted slice.
    ///
    /// Results are cached; the cache is invalidated whenever a new archetype
    /// is created. The returned `Arc` can be cloned cheaply (one atomic
    /// increment) and iterated without holding any borrow on the world, which
    /// is what the query layer needs.
    #[allow(dead_code, reason = "consumed by the upcoming `Query` layer")]
    pub(crate) fn matching_archetypes(
        &self,
        required: ComponentMask,
        excluded: ComponentMask,
    ) -> Arc<[ArchetypeId]> {
        let key: QueryKey = (required, excluded);

        if let Some(ids) = self
            .query_cache
            .read()
            .expect("query cache poisoned")
            .get(&key)
        {
            return Arc::clone(ids);
        }

        // Slow path: compute and insert. Two writers may race here; the
        // second overwrites with an identical value, which is harmless.
        let ids: Arc<[ArchetypeId]> = self
            .archetypes
            .iter()
            .filter(|a| a.mask().matches(&required, &excluded))
            .map(Archetype::id)
            .collect::<Vec<_>>()
            .into();

        self.query_cache
            .write()
            .expect("query cache poisoned")
            .insert(key, Arc::clone(&ids));

        ids
    }

    /// Invokes `f` with the archetypes matching the query mask.
    ///
    /// Convenience wrapper over [`Self::matching_archetypes`]; the callback
    /// receives a slice borrowed from the shared [`Arc`].
    #[allow(dead_code, reason = "consumed by the upcoming `Query` layer")]
    pub(crate) fn with_matching_archetypes<F, R>(
        &self,
        required: ComponentMask,
        excluded: ComponentMask,
        f: F,
    ) -> R
    where
        F: FnOnce(&[ArchetypeId]) -> R,
    {
        let ids = self.matching_archetypes(required, excluded);
        f(&ids)
    }
}
