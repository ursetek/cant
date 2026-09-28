//! The ECS world.
//!
//! [`World`] owns every runtime registry: components, archetypes, entities,
//! resources, and the edge graph connecting archetypes. Responsibilities are
//! split across sibling modules, each contributing `impl World` blocks:
//!
//! - [`components`]: registering and looking up component types.
//! - [`archetypes`]: creating archetypes, edge graph, query cache.
//! - [`entities`]: spawning and despawning entities.
//! - [`migration`]: structural changes (`add_component`/`remove_component`).
//! - [`resources`]: type-erased resource registry.

mod archetypes;
mod bundles;
mod components;
mod entities;
mod migration;
mod resources;

use crate::archetype::Archetype;
use crate::component::ComponentInfo;
use crate::entity::Entities;
use crate::storage::Storage;
use crate::{ArchetypeId, ComponentId, ComponentMask, Query, QueryError};
use crate::{BundleId, QueryMask};
use core::any::TypeId;
pub use resources::ResourceError;
use rustc_hash::FxHashMap;
use std::sync::{Arc, RwLock};

/// Slot index of an [`ArchetypeId`] inside [`World::archetypes`].
///
/// [`ArchetypeId`] is backed by `NonZeroU32`, so raw values start at `1`;
/// slot `0` corresponds to raw `1`.
#[inline]
pub(crate) fn archetype_index(id: ArchetypeId) -> usize {
    id.raw() as usize - 1
}

/// Factory that produces a fresh storage for a registered component.
///
/// The factory receives the [`ComponentInfo`] that the world already holds,
/// so the storage does not need to reconstruct type metadata.
pub(crate) type StorageFactory = fn(ComponentInfo) -> Box<dyn Storage>;

/// Key for the query cache: required mask plus excluded mask.
pub(crate) type QueryKey = (ComponentMask, ComponentMask);

/// Precomputed layout for a registered bundle.
pub(crate) struct BundleDesc {
    /// Archetype that holds the bundle's component set.
    pub(crate) archetype: ArchetypeId,
    /// Storage-slot index of each tuple component inside the archetype.
    pub(crate) slots: Box<[usize]>,
    /// Byte offset of each component value within an arena block.
    pub(crate) arena_offsets: Box<[u32]>,
    /// Total size of an arena block, including trailing padding.
    pub(crate) arena_size: u32,
    /// Alignment of the arena block (max of component alignments).
    pub(crate) arena_align: u32,
}

/// The ECS world: component registry, archetype storage, entity allocator.
pub struct World {
    pub(crate) components: Vec<ComponentInfo>,
    pub(crate) component_factories: Vec<StorageFactory>,
    pub(crate) archetypes: Vec<Archetype>,
    pub(crate) archetype_index: FxHashMap<ComponentMask, ArchetypeId>,
    pub(crate) add_edges: FxHashMap<(ArchetypeId, ComponentId), ArchetypeId>,
    pub(crate) remove_edges: FxHashMap<(ArchetypeId, ComponentId), ArchetypeId>,
    pub(crate) entities: Entities,
    pub(crate) resources: Vec<Box<dyn core::any::Any + Send + Sync>>,
    pub(crate) resource_index: FxHashMap<core::any::TypeId, crate::ResourceId>,
    pub(crate) empty_archetype: ArchetypeId,
    pub(crate) query_cache: RwLock<FxHashMap<QueryKey, Arc<[ArchetypeId]>>>,
    pub(crate) bundles: Vec<BundleDesc>,
    pub(crate) bundle_by_type: FxHashMap<TypeId, BundleId>,
    pub(crate) archetype_version: u64,
}

impl World {
    /// Creates an empty world with a single archetype for entities that carry
    /// no components.
    #[must_use]
    pub fn new() -> Self {
        let mut world = Self {
            components: Vec::new(),
            component_factories: Vec::new(),
            archetypes: Vec::new(),
            archetype_index: FxHashMap::default(),
            add_edges: FxHashMap::default(),
            remove_edges: FxHashMap::default(),
            entities: Entities::new(),
            resources: Vec::new(),
            resource_index: FxHashMap::default(),
            empty_archetype: ArchetypeId::from_raw(1),
            query_cache: RwLock::new(FxHashMap::default()),
            bundles: Vec::new(),
            bundle_by_type: FxHashMap::default(),
            archetype_version: 0,
        };
        world.empty_archetype =
            world.find_or_create_archetype(ComponentMask::EMPTY);
        world
    }

    /// Clears the query cache. Called whenever a new archetype is created.
    pub(crate) fn invalidate_query_cache(&self) {
        self.query_cache
            .write()
            .expect("query cache poisoned")
            .clear();
    }

    /// Prepares a query for the given mask.
    ///
    /// The returned [`Query`] holds a snapshot of the matching archetypes and
    /// the world's archetype version at the time of the call. On the next
    /// iteration call, the query compares its cached version against the
    /// world's current version; if they differ, the archetype list is
    /// refreshed automatically. This means a query obtained before a
    /// structural change remains usable: the first `for_each` after the
    /// change sees the new archetypes.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::UnknownComponent`] if the mask references a
    /// component that was never registered.
    pub fn prepare(&self, mask: QueryMask) -> Result<Query, QueryError> {
        for (c, _) in mask.access() {
            if c.index() >= self.components.len() {
                return Err(QueryError::UnknownComponent(*c));
            }
        }
        let archetypes = self.matching_archetypes(mask.required, mask.excluded);
        Ok(Query {
            mask,
            archetypes,
            version: self.archetype_version,
        })
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for World {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("World")
            .field("components", &self.components.len())
            .field("archetypes", &self.archetypes.len())
            .field("live_entities", &self.entities.live_count())
            .field("resources", &self.resources.len())
            .finish_non_exhaustive()
    }
}
