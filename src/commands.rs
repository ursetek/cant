//! Deferred structural changes.

use crate::bundle::Bundle;
use crate::{BundleId, ComponentId, Entity, World};

/// A command buffered for application at the end of the stage.
pub(crate) enum Command {
    /// Move a reserved entity into the empty archetype.
    Spawn {
        /// The entity whose slot was reserved by `Commands::spawn`.
        entity: Entity,
    },
    /// Free an entity and drop its components.
    Despawn(Entity),
    /// Insert a single component whose value lives in the arena.
    InsertRaw {
        /// Target entity.
        entity: Entity,
        /// Component to write.
        component: ComponentId,
        /// Byte offset into `Commands::arena`.
        arena_offset: u32,
    },
    /// Spawn an entity and write every component of a bundle from the arena.
    SpawnBundle {
        /// Entity whose slot was reserved.
        entity: Entity,
        /// Pre-registered bundle descriptor.
        bundle: BundleId,
        /// Byte offset into `Commands::arena`.
        arena_offset: u32,
    },
    /// Remove a component by identifier.
    Remove {
        /// Target entity.
        entity: Entity,
        /// Component to remove.
        component: ComponentId,
    },
    /// Run an arbitrary closure against the world and entity.
    Apply {
        /// Target entity.
        entity: Entity,
        /// Closure to run at apply time.
        f: Box<ApplyFn>,
    },
}

/// Type-erased structural closure used only by `Commands::spawn_with`.
type ApplyFn = dyn FnOnce(&mut World, Entity) + Send;

/// Rounds `offset` up to the next multiple of `align` (a power of two).
#[inline]
fn align_up(offset: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (offset + align - 1) & !(align - 1)
}

/// Buffer of deferred structural changes.
pub struct Commands {
    world: *mut World,
    buffer: Vec<Command>,
    arena: Vec<u8>,
    allows_spawn: bool,
}

impl Commands {
    /// # Safety
    ///
    /// No other mutable or shared reference to `world` may be used while
    /// this `Commands` exists. `allows_spawn` controls eager entity
    /// allocation.
    pub(crate) unsafe fn from_raw(
        world: *mut World,
        allows_spawn: bool,
    ) -> Self {
        Self {
            world,
            buffer: Vec::new(),
            arena: Vec::new(),
            allows_spawn,
        }
    }

    /// Reserves an entity slot and queues a spawn command.
    ///
    /// # Panics
    ///
    /// Panics if called from a parallel batch.
    #[must_use]
    pub fn spawn(&mut self) -> Entity {
        self.ensure_can_spawn();
        // SAFETY: exclusive access guaranteed by `allows_spawn`.
        let world = unsafe { &mut *self.world };
        let entity = world.alloc_reserved_entity();
        self.buffer.push(Command::Spawn { entity });
        entity
    }

    /// Reserves an entity and queues a bundle spawn.
    ///
    /// # Panics
    ///
    /// Panics if called from a parallel batch.
    pub fn spawn_bundle<B: Bundle + Send + 'static>(
        &mut self,
        bundle: B,
    ) -> Entity {
        self.ensure_can_spawn();
        // SAFETY: exclusive access.
        let world = unsafe { &mut *self.world };
        let bid = world.register_bundle::<B>();
        let entity = world.alloc_reserved_entity();
        let arena_offset = self.write_bundle_to_arena(world, bid, bundle);
        self.buffer.push(Command::SpawnBundle {
            entity,
            bundle: bid,
            arena_offset,
        });
        entity
    }

    /// Like [`Self::spawn_bundle`], but takes a pre-registered bundle id.
    ///
    /// # Panics
    ///
    /// Panics if called from a parallel batch.
    pub fn spawn_bundle_id<B: Bundle + Send + 'static>(
        &mut self,
        bid: BundleId,
        bundle: B,
    ) -> Entity {
        self.ensure_can_spawn();
        // SAFETY: exclusive access.
        let world = unsafe { &mut *self.world };
        let entity = world.alloc_reserved_entity();
        let arena_offset = self.write_bundle_to_arena(world, bid, bundle);
        self.buffer.push(Command::SpawnBundle {
            entity,
            bundle: bid,
            arena_offset,
        });
        entity
    }

    fn write_bundle_to_arena<B: Bundle>(
        &mut self,
        world: &World,
        bid: BundleId,
        bundle: B,
    ) -> u32 {
        let desc = world.bundle_desc(bid);
        let block_align = desc.arena_align as usize;
        let block_size = desc.arena_size as usize;
        let offsets = &desc.arena_offsets;

        let start = align_up(self.arena.len(), block_align);
        self.arena.resize(start + block_size, 0);

        let slice = &mut self.arena[start..start + block_size];
        // SAFETY: the bundle's write_to_arena contract matches this layout.
        unsafe { bundle.write_to_arena(slice, offsets) };

        u32::try_from(start).expect("arena offset overflow")
    }

    /// Reserves an entity and queues an arbitrary structural closure.
    ///
    /// # Panics
    ///
    /// Panics if called from a parallel batch.
    pub fn spawn_with<F>(&mut self, init: F) -> Entity
    where
        F: FnOnce(&mut World, Entity) + Send + 'static,
    {
        let entity = self.spawn();
        self.buffer.push(Command::Apply {
            entity,
            f: Box::new(init),
        });
        entity
    }

    /// Queues an insertion of `value` on `entity`.
    ///
    /// # Panics
    ///
    /// Panics if `T` is not registered.
    pub fn insert_component<T: Send + Sync + 'static>(
        &mut self,
        entity: Entity,
        value: T,
    ) {
        // SAFETY: shared access to `*self.world`.
        let world = unsafe { &*self.world };
        let comp = world
            .component_id::<T>()
            .expect("insert_component: component not registered");
        let layout = world.component_info(comp).layout();
        let size = layout.size();
        let align = layout.align();
        debug_assert_eq!(size, core::mem::size_of::<T>());
        debug_assert_eq!(align, core::mem::align_of::<T>());

        let start = align_up(self.arena.len(), align);
        self.arena.resize(start + size, 0);

        let src = core::ptr::addr_of!(value).cast::<u8>();
        // SAFETY: `start + size == self.arena.len()`, so the destination
        // range is in-bounds. `src` is valid for `size` bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(
                src,
                self.arena.as_mut_ptr().add(start),
                size,
            );
        }
        core::mem::forget(value);

        self.buffer.push(Command::InsertRaw {
            entity,
            component: comp,
            arena_offset: u32::try_from(start).expect("arena offset overflow"),
        });
    }

    /// Queues a despawn.
    pub fn despawn(&mut self, entity: Entity) {
        self.buffer.push(Command::Despawn(entity));
    }

    /// Queues a component removal.
    pub fn remove_component(&mut self, entity: Entity, component: ComponentId) {
        self.buffer.push(Command::Remove { entity, component });
    }

    /// Number of buffered commands.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Returns `true` if no commands are buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Applies every buffered command in FIFO order.
    pub fn apply(self) {
        // SAFETY: `Commands` is the sole mutable accessor for its lifetime.
        let world = unsafe { &mut *self.world };
        let arena = self.arena;
        for cmd in self.buffer {
            // SAFETY: every command's arena offsets index into `arena`.
            unsafe { apply_one(world, cmd, &arena) };
        }
    }

    /// Splits the buffer for the parallel scheduler. Both halves are `Send`.
    pub(crate) fn into_parts(self) -> (Vec<Command>, Vec<u8>) {
        (self.buffer, self.arena)
    }

    /// Applies pre-split buffers sequentially, returning the cleared Vecs
    /// for reuse.
    ///
    /// # Safety
    ///
    /// Every command in `buffer` must have been enqueued against the arena
    /// whose bytes are now held by `arena`, with the same offsets.
    pub(crate) unsafe fn apply_parts(
        world: &mut World,
        mut buffer: Vec<Command>,
        arena: Vec<u8>,
    ) -> (Vec<Command>, Vec<u8>) {
        let arena_ref = arena.as_slice();
        // Drain consumes commands by value while keeping the Vec's backing
        // storage intact for the caller to reuse.
        for cmd in buffer.drain(..) {
            // SAFETY: caller guarantees each command's offsets index into
            // `arena`.
            unsafe { apply_one(world, cmd, arena_ref) };
        }
        (buffer, arena)
    }

    /// Constructs a `Commands` from already-owned buffers.
    ///
    /// Used by the scheduler's parallel path, which hands pooled buffers to
    /// worker threads and reclaims them after applying.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::from_raw`].
    pub(crate) unsafe fn from_parts(
        world: *mut World,
        buffer: Vec<Command>,
        arena: Vec<u8>,
        allows_spawn: bool,
    ) -> Self {
        Self {
            world,
            buffer,
            arena,
            allows_spawn,
        }
    }

    fn ensure_can_spawn(&self) {
        assert!(
            self.allows_spawn,
            "Commands::spawn is unavailable in a parallel batch; \
             mark the system's AccessList with `mark_structural_write`",
        );
    }
}

impl core::fmt::Debug for Commands {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Commands")
            .field("world", &self.world)
            .field("pending", &self.buffer.len())
            .field("arena_len", &self.arena.len())
            .field("allows_spawn", &self.allows_spawn)
            .finish()
    }
}

/// # Safety
///
/// Every command in `buffer` must have been enqueued against the arena whose
/// bytes are now held by `arena`, with the same offsets.
unsafe fn apply_one(world: &mut World, cmd: Command, arena: &[u8]) {
    match cmd {
        Command::Spawn { entity } => world.spawn_reserved(entity),
        Command::Despawn(entity) => {
            world.despawn(entity);
        }
        Command::InsertRaw {
            entity,
            component,
            arena_offset,
        } => {
            // SAFETY: offset was produced by the enqueue path.
            let ptr = unsafe { arena.as_ptr().add(arena_offset as usize) };
            // SAFETY: bytes at `ptr` are a valid value of the component type.
            unsafe { world.add_component_raw(entity, component, ptr) };
        }
        Command::SpawnBundle {
            entity,
            bundle,
            arena_offset,
        } => {
            // SAFETY: offset was produced by the bundle enqueue path.
            unsafe {
                world.spawn_bundle_reserved_from_arena(
                    entity,
                    bundle,
                    arena,
                    arena_offset as usize,
                );
            }
        }
        Command::Remove { entity, component } => {
            world.remove_component(entity, component);
        }
        Command::Apply { entity, f } => {
            f(world, entity);
        }
    }
}
