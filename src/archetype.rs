//! Archetype — a set of entities sharing the same component layout.
//!
//! An archetype is the unit of grouping in the ECS. Every entity lives in
//! exactly one archetype; the archetype determines which component types it
//! carries and, transitively, where its component values are stored. Entities
//! migrate between archetypes when components are added or removed.
//!
//! Chunking: each storage inside an archetype is split into chunks of
//! `chunk_capacity` elements. All storages share the same capacity, so chunk
//! boundaries line up across component types. This is what allows a query to
//! iterate chunk by chunk and observe a consistent window of entities.

use core::fmt;

use crate::storage::Storage;
use crate::{ArchetypeId, ChunkId, ComponentId, ComponentMask, Entity};

/// Target byte size for a single chunk.
///
/// The actual capacity is the minimum across all non-ZST components, so
/// every chunk stays at or below this size.
pub const DEFAULT_CHUNK_BYTES: usize = 16 * 1024;

/// Default rows per chunk when no non-ZST component constrains capacity
/// (i.e. the archetype consists only of ZST markers).
pub const DEFAULT_CHUNK_ROWS: u32 = 1024;

/// Result of [`Archetype::swap_remove`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapRemoveResult {
    /// The entity that was removed from the archetype.
    pub removed: Entity,
    /// The entity that was swapped into `removed`'s row, if the removed row
    /// was not the last one. `None` if the removed row was the tail row.
    pub moved: Option<Entity>,
}

/// A group of entities with an identical set of component types.
pub struct Archetype {
    id: ArchetypeId,
    mask: ComponentMask,
    /// All component IDs in the archetype, sorted ascending.
    components: Box<[ComponentId]>,
    /// One storage per component ID, parallel to `components`.
    ///
    /// Zero-sized components still receive a (dummy) storage so the parallel
    /// layout is preserved; such storage is never written to.
    storages: Box<[Box<dyn Storage>]>,
    /// Entities in row order. Invariant: `entities.len() == storages[i].len()`
    /// for every `i`.
    entities: Vec<Entity>,
    /// Rows per chunk, identical for every storage.
    chunk_capacity: u32,
}

impl Archetype {
    /// Creates a new archetype.
    ///
    /// # Panics
    ///
    /// Panics if `components` is not sorted ascending, if
    /// `storages.len() != components.len()`, or if a storage's component ID
    /// does not match the parallel entry in `components`.
    #[must_use]
    pub fn new(
        id: ArchetypeId,
        mask: ComponentMask,
        components: Box<[ComponentId]>,
        mut storages: Vec<Box<dyn Storage>>,
    ) -> Self {
        assert_eq!(
            components.len(),
            storages.len(),
            "components and storages must have the same length",
        );
        for (i, c) in components.iter().enumerate() {
            assert_eq!(
                storages[i].info().id(),
                *c,
                "storage {i} does not match component {c:?}",
            );
            if i > 0 {
                assert!(
                    components[i - 1] < *c,
                    "components must be sorted and unique",
                );
            }
        }

        let chunk_capacity = pick_chunk_capacity(&storages);
        for s in &mut storages {
            s.init_capacity(chunk_capacity);
        }

        Self {
            id,
            mask,
            components,
            storages: storages.into_boxed_slice(),
            entities: Vec::new(),
            chunk_capacity,
        }
    }

    /// Archetype identifier.
    #[inline]
    #[must_use]
    pub fn id(&self) -> ArchetypeId {
        self.id
    }

    /// Component mask, including ZST bits.
    #[inline]
    #[must_use]
    pub fn mask(&self) -> &ComponentMask {
        &self.mask
    }

    /// All component IDs in this archetype, sorted ascending.
    #[inline]
    #[must_use]
    pub fn components(&self) -> &[ComponentId] {
        &self.components
    }

    /// Entities in row order.
    #[inline]
    #[must_use]
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    /// Number of entities.
    ///
    /// # Panics
    ///
    /// Panics if the number of rows exceeds `u32::MAX`. In practice this is
    /// unreachable: chunk identifiers and row indices are `u32`, so an
    /// archetype cannot hold more rows than the entity table can address.
    #[inline]
    #[must_use]
    pub fn len(&self) -> u32 {
        u32::try_from(self.entities.len()).expect("archetype row overflow")
    }

    /// Returns `true` if the archetype holds no entities.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Rows per chunk.
    #[inline]
    #[must_use]
    pub fn chunk_capacity(&self) -> u32 {
        self.chunk_capacity
    }

    /// Number of allocated chunks. Zero for an empty archetype.
    ///
    /// # Panics
    ///
    /// Panics if the computed chunk count exceeds `u32::MAX`. This is
    /// unreachable given that [`Self::len`] is bounded by `u32::MAX` and
    /// `chunk_capacity >= 1`, so the quotient fits in `u32`.
    #[inline]
    #[must_use]
    pub fn chunk_count(&self) -> u32 {
        if self.entities.is_empty() {
            0
        } else {
            let cap = self.chunk_capacity as usize;
            u32::try_from(self.entities.len().div_ceil(cap))
                .expect("chunk count overflow")
        }
    }

    /// Returns `true` if the archetype carries the given component
    /// (including ZST markers).
    #[inline]
    #[must_use]
    pub fn has_component(&self, component: ComponentId) -> bool {
        self.mask.contains(component)
    }

    /// Chunk that holds the given global row.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `row >= len()`.
    #[inline]
    #[must_use]
    pub fn chunk_of(&self, row: u32) -> ChunkId {
        debug_assert!(row < self.len(), "row out of bounds");
        ChunkId::from_raw(row / self.chunk_capacity)
    }

    /// Row inside its chunk for the given global row.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `row >= len()`.
    #[inline]
    #[must_use]
    pub fn row_in_chunk(&self, row: u32) -> u32 {
        debug_assert!(row < self.len(), "row out of bounds");
        row % self.chunk_capacity
    }

    /// Returns the storage for `component`, if present.
    #[must_use]
    pub fn storage(&self, component: ComponentId) -> Option<&dyn Storage> {
        let idx = self.storage_index_of(component)?;
        Some(self.storage_at(idx))
    }

    /// Returns the mutable storage for `component`, if present.
    pub fn storage_mut(
        &mut self,
        component: ComponentId,
    ) -> Option<&mut (dyn Storage + '_)> {
        let idx = self.storage_index_of(component)?;
        Some(self.storage_at_mut(idx))
    }

    /// Appends `entity` and calls `init` to write every component slot of
    /// the new row.
    ///
    /// Returns the row index assigned to the entity.
    ///
    /// # Panics
    ///
    /// Propagates a panic from `init`. If `init` panics, the archetype's
    /// internal invariants are temporarily violated: the entities vector has
    /// grown, but the storages' lengths have not. The caller is expected to
    /// ensure the archetype is either discarded or repaired before further
    /// use.
    pub fn push_with<F>(&mut self, entity: Entity, init: F) -> u32
    where
        F: FnOnce(&mut Archetype, u32),
    {
        let row = self.reserve_row(entity);
        init(self, row);
        // SAFETY: `init` wrote every slot of the reserved row.
        unsafe { self.commit_row() };
        row
    }

    /// Writes one component value into an uninitialized slot.
    ///
    /// # Panics
    ///
    /// Panics if `component` is not present in this archetype. In debug
    /// builds, also panics if `row` is out of bounds; in release builds the
    /// row is the caller's responsibility, as documented under `# Safety`.
    ///
    /// # Safety
    ///
    /// - `component` must be present in this archetype.
    /// - The slot at `(row, component)` must be uninitialized.
    /// - `data` must point to a valid, initialized value of the component's
    ///   type, properly aligned.
    pub unsafe fn write_raw(
        &mut self,
        component: ComponentId,
        row: u32,
        data: *const u8,
    ) {
        let idx = self
            .storage_index_of(component)
            .expect("component not present in archetype");
        let chunk = self.chunk_of(row);
        let row_in_chunk = self.row_in_chunk(row);
        // SAFETY: caller guarantees slot is uninitialized and data valid.
        unsafe { self.storages[idx].write_raw(chunk, row_in_chunk, data) };
    }

    /// Removes the entity at `row` by swapping the last row into its place.
    ///
    /// Component values at `row` are dropped; component values at the last
    /// row are moved into `row`. The returned [`SwapRemoveResult`] carries
    /// both the removed entity and, if applicable, the entity that was
    /// moved, so the caller can update its location bookkeeping.
    ///
    /// # Panics
    ///
    /// Panics if `row >= len()` or if the archetype is empty.
    pub fn swap_remove(&mut self, row: u32) -> SwapRemoveResult {
        let row_usize = row as usize;
        assert!(row_usize < self.entities.len(), "row out of bounds");

        let last = self.entities.len() - 1;
        let removed = self.entities[row_usize];

        let moved = if row_usize == last {
            // Dropping the tail row: destroy all components in place.
            let chunk = self.chunk_of(row);
            let row_in_chunk = self.row_in_chunk(row);
            for s in &mut self.storages {
                // SAFETY: row < len, so the slot is initialized.
                unsafe { s.drop_raw(chunk, row_in_chunk) };
            }
            None
        } else {
            let src = u32::try_from(last).expect("row overflow");
            let src_chunk = self.chunk_of(src);
            let src_row = self.row_in_chunk(src);
            let dst_chunk = self.chunk_of(row);
            let dst_row = self.row_in_chunk(row);
            for s in &mut self.storages {
                // SAFETY: dst is initialized (row < len) and src is
                // initialized (last < len); we drop dst, then move src into
                // it. After this, src is logically uninitialized.
                unsafe {
                    s.drop_raw(dst_chunk, dst_row);
                    s.move_raw(src_chunk, src_row, dst_chunk, dst_row);
                }
            }
            let moved_entity = self.entities[last];
            self.entities[row_usize] = moved_entity;
            Some(moved_entity)
        };

        self.entities.pop();
        let new_len = u32::try_from(self.entities.len()).expect("row overflow");
        for s in &mut self.storages {
            // SAFETY: the removed slot was dropped and, if applicable, the
            // last slot was moved into its place. The prefix `[0, new_len)`
            // is fully initialized.
            unsafe { s.set_len(new_len) };
        }

        SwapRemoveResult { removed, moved }
    }

    /// Returns the entity at the given row.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `row >= len()`.
    #[inline]
    #[must_use]
    pub(crate) fn entity_at(&self, row: u32) -> Entity {
        debug_assert!(row < self.len(), "row out of bounds");
        self.entities[row as usize]
    }

    /// Reserves a new row for `entity` and returns its index.
    ///
    /// Chunks are grown as needed. Every component slot of the new row is
    /// uninitialized: the caller must write each one and then call
    /// [`Self::commit_row`].
    pub(crate) fn reserve_row(&mut self, entity: Entity) -> u32 {
        let row = self.entities.len();
        let cap = self.chunk_capacity as usize;
        let chunks_needed = (row + 1).div_ceil(cap);
        let chunks_have = if row == 0 { 0 } else { row.div_ceil(cap) };
        if chunks_needed > chunks_have {
            let n = u32::try_from(chunks_needed).expect("chunk count overflow");
            for s in &mut self.storages {
                s.grow_to(n);
            }
        }
        self.entities.push(entity);
        u32::try_from(row).expect("row overflow")
    }

    /// Marks the last reserved row as fully initialized.
    ///
    /// # Safety
    ///
    /// Every component slot in the last row must have been written since the
    /// matching call to [`Self::reserve_row`].
    pub(crate) unsafe fn commit_row(&mut self) {
        let new_len = u32::try_from(self.entities.len()).expect("row overflow");
        for s in &mut self.storages {
            // SAFETY: caller guarantees every slot in the last row is
            // initialized, so the prefix `[0, new_len)` is fully initialized.
            unsafe { s.set_len(new_len) };
        }
    }

    /// Returns a raw pointer to the given slot's bytes.
    ///
    /// # Safety
    ///
    /// - `component` present in this archetype.
    /// - `row < len()`.
    #[inline]
    pub(crate) unsafe fn slot_ptr(
        &self,
        component: ComponentId,
        row: u32,
    ) -> *const u8 {
        let idx = self
            .storage_index_of(component)
            .expect("component not present in archetype");
        let chunk = self.chunk_of(row);
        let rin = self.row_in_chunk(row);
        let stride = self.storages[idx].info().layout().size();
        // SAFETY: `chunk_ptr` returns a valid pointer to the chunk start;
        // `rin * stride` stays within the chunk.
        let base = unsafe { self.storages[idx].chunk_ptr(chunk) };
        unsafe { base.add(rin as usize * stride) }
    }

    /// Returns a mutable raw pointer to the given slot's bytes.
    ///
    /// # Safety
    ///
    /// Same as [`Self::slot_ptr`], plus: no aliasing reference while in use.
    #[inline]
    pub(crate) unsafe fn slot_ptr_mut(
        &mut self,
        component: ComponentId,
        row: u32,
    ) -> *mut u8 {
        let idx = self
            .storage_index_of(component)
            .expect("component not present in archetype");
        let chunk = self.chunk_of(row);
        let rin = self.row_in_chunk(row);
        let stride = self.storages[idx].info().layout().size();
        let base = unsafe { self.storages[idx].chunk_ptr_mut(chunk) };
        // SAFETY: see `slot_ptr`.
        unsafe { base.add(rin as usize * stride) }
    }

    /// Size in bytes of the given component's stored type. Zero for ZSTs.
    ///
    /// # Panics
    ///
    /// Panics if `component` is not present in this archetype.
    #[inline]
    #[must_use]
    pub(crate) fn component_size(&self, component: ComponentId) -> usize {
        let idx = self
            .storage_index_of(component)
            .expect("component not present in archetype");
        self.storages[idx].info().layout().size()
    }

    /// Relocates the raw bytes of a component from `src_row` to `dst_row`
    /// inside this archetype.
    ///
    /// No destructor runs on either side; both slots are logically
    /// uninitialized after the call (the source is emptied, the destination
    /// is filled). Used for swap-removal bookkeeping during migration.
    ///
    /// # Safety
    ///
    /// - `component` present in this archetype.
    /// - Both rows are live and distinct.
    /// - `copy_nonoverlapping` semantics: source and destination must not
    ///   overlap.
    pub(crate) unsafe fn relocate_raw(
        &mut self,
        component: ComponentId,
        src_row: u32,
        dst_row: u32,
    ) {
        let size = self.component_size(component);
        if size == 0 {
            return;
        }
        // SAFETY: pointers refer to live, distinct slots within the same
        // storage; the caller guarantees no aliasing.
        unsafe {
            let src = self.slot_ptr(component, src_row);
            let dst = self.slot_ptr_mut(component, dst_row);
            core::ptr::copy_nonoverlapping(src, dst, size);
        }
    }

    /// Returns the storage slot index for `component`, if present.
    ///
    /// The index is stable for the lifetime of the archetype and is what
    /// [`Self::storage_at`] expects. Query code hoists this lookup out of
    /// per-chunk loops: compute once per archetype, reuse for every chunk.
    #[inline]
    #[must_use]
    pub(crate) fn storage_index_of(
        &self,
        component: ComponentId,
    ) -> Option<usize> {
        self.components.binary_search(&component).ok()
    }

    /// Returns the storage at `idx` without a lookup.
    ///
    /// # Panics
    ///
    /// Panics if `idx >= self.components().len()`.
    #[inline]
    #[must_use]
    pub(crate) fn storage_at(&self, idx: usize) -> &dyn Storage {
        &*self.storages[idx]
    }

    /// Returns the mutable storage at `idx` without a lookup.
    ///
    /// # Panics
    ///
    /// Panics if `idx >= self.components().len()`.
    #[inline]
    pub(crate) fn storage_at_mut(
        &mut self,
        idx: usize,
    ) -> &mut (dyn Storage + '_) {
        &mut *self.storages[idx]
    }

    /// Drops the component value at `(component, row)`.
    ///
    /// # Safety
    ///
    /// - `component` must be present in this archetype.
    /// - `row < len()`.
    /// - The slot must be initialized and not dropped again.
    pub(crate) unsafe fn drop_component(
        &mut self,
        component: ComponentId,
        row: u32,
    ) {
        let idx = self
            .storage_index_of(component)
            .expect("component not present in archetype");
        let chunk = self.chunk_of(row);
        let rin = self.row_in_chunk(row);
        unsafe { self.storages[idx].drop_raw(chunk, rin) };
    }

    /// Writes a component value into an uninitialized slot.
    ///
    /// # Safety
    ///
    /// - `component` must be present in this archetype.
    /// - The slot at `(component, row)` must be uninitialized.
    /// - `data` must point to a valid, initialized value of the component's
    ///   type, properly aligned.
    pub(crate) unsafe fn write_component(
        &mut self,
        component: ComponentId,
        row: u32,
        data: *const u8,
    ) {
        let idx = self
            .storage_index_of(component)
            .expect("component not present in archetype");
        let chunk = self.chunk_of(row);
        let rin = self.row_in_chunk(row);
        // SAFETY: caller guarantees slot is uninitialized and data valid.
        unsafe { self.storages[idx].write_raw(chunk, rin, data) };
    }

    /// Writes a component value into an uninitialized slot, addressed by
    /// storage-slot index instead of `ComponentId`.
    ///
    /// Used by the bundle fast path, which precomputes slot indices at
    /// registration time and skips the binary search per spawn.
    ///
    /// # Safety
    ///
    /// - `slot < components().len()`.
    /// - The slot at `(slot, row)` must be uninitialized.
    /// - `data` must point to a valid, initialized value of the component
    ///   type stored at `slot`.
    pub(crate) unsafe fn write_component_at_slot(
        &mut self,
        slot: usize,
        row: u32,
        data: *const u8,
    ) {
        let chunk = self.chunk_of(row);
        let rin = self.row_in_chunk(row);
        // SAFETY: caller guarantees slot bounds and slot uninitialized.
        unsafe { self.storages[slot].write_raw(chunk, rin, data) };
    }

    /// Returns a mutable raw pointer to the start of the given storage's
    /// chunk.
    ///
    /// Unlike [`Self::storage_at_mut`], this returns a raw pointer with no
    /// lifetime, so the mutable borrow of `self` ends at the call site. Used
    /// by the projection layer, which builds raw views across many storages
    /// in one loop and cannot afford to hold a `&mut dyn Storage` for each.
    ///
    /// # Safety
    ///
    /// - `slot < components().len()`.
    /// - `chunk < chunk_count()`.
    /// - No other reference may alias the chunk while the returned pointer
    ///   is used.
    pub(crate) unsafe fn chunk_base_mut(
        &mut self,
        slot: usize,
        chunk: ChunkId,
    ) -> *mut u8 {
        // SAFETY: caller guarantees slot and chunk bounds.
        unsafe { self.storages[slot].chunk_ptr_mut(chunk) }
    }

    /// Replaces the component value at `(component, row)`.
    ///
    /// The old value is dropped, then `data` is moved in.
    ///
    /// # Safety
    ///
    /// - `component` must be present in this archetype.
    /// - `row < len()`.
    /// - `data` must point to a valid, initialized value; ownership of the
    ///   value transfers to this call.
    pub(crate) unsafe fn replace_component(
        &mut self,
        component: ComponentId,
        row: u32,
        data: *const u8,
    ) {
        let idx = self
            .storage_index_of(component)
            .expect("component not present in archetype");
        let chunk = self.chunk_of(row);
        let rin = self.row_in_chunk(row);
        // SAFETY: the slot is initialized (row < len) and `data` is valid.
        unsafe {
            self.storages[idx].drop_raw(chunk, rin);
            self.storages[idx].write_raw(chunk, rin, data);
        }
    }

    /// Pops the last entity from `entities`. If the last row differs from
    /// `src_row`, its entity is written into `src_row` first.
    ///
    /// This is the bookkeeping half of a swap-removal that does not drop or
    /// move component values. The caller must have handled the storages.
    pub(crate) fn pop_last_entity_and_swap(
        &mut self,
        src_row: u32,
    ) -> Option<Entity> {
        let src_idx = src_row as usize;
        let last_idx = self.entities.len() - 1;
        if src_idx == last_idx {
            self.entities.pop();
            None
        } else {
            let moved = self.entities[last_idx];
            self.entities[src_idx] = moved;
            self.entities.pop();
            Some(moved)
        }
    }

    /// Shrinks the tracked length to the current entity count, without
    /// dropping any value.
    ///
    /// Handles the empty-archetype case: when the last entity was migrated
    /// out, `entities.len() == 0` and every storage's length becomes `0`.
    ///
    /// # Safety
    ///
    /// The caller must have moved out or dropped every initialized slot in
    /// the rows at indices `[entities.len(), old_len)`. Those slots are
    /// logically uninitialized and will not be touched again.
    pub(crate) unsafe fn shrink_len_no_drop(&mut self) {
        let new_len = u32::try_from(self.entities.len()).expect("row overflow");
        for s in &mut self.storages {
            // SAFETY: caller guarantees the prefix `[0, new_len)` is
            // initialized; the removed tail rows are already logically
            // uninitialized.
            unsafe { s.set_len(new_len) };
        }
    }
}

impl fmt::Debug for Archetype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Archetype")
            .field("id", &self.id)
            .field("components", &self.components)
            .field("entities", &self.entities.len())
            .field("chunk_capacity", &self.chunk_capacity)
            .field("chunk_count", &self.chunk_count())
            .finish_non_exhaustive()
    }
}

/// Picks the per-chunk capacity.
///
/// For archetypes containing at least one non-ZST component, the capacity is
/// the smallest `DEFAULT_CHUNK_BYTES / size` across all non-ZST components,
/// so every chunk stays at or below the byte target. Archetypes consisting
/// only of ZST markers fall back to `DEFAULT_CHUNK_ROWS`, since byte size
/// alone does not constrain them.
fn pick_chunk_capacity(storages: &[Box<dyn Storage>]) -> u32 {
    let mut cap: Option<u32> = None;
    for s in storages {
        let sz = s.info().layout().size();
        if sz == 0 {
            continue;
        }
        let per_chunk = DEFAULT_CHUNK_BYTES / sz;
        let c = u32::try_from(per_chunk).unwrap_or(u32::MAX).max(1);
        cap = Some(match cap {
            Some(prev) => prev.min(c),
            None => c,
        });
    }
    cap.unwrap_or(DEFAULT_CHUNK_ROWS).max(1)
}
