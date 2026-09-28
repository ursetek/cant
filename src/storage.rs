//! Type-erased component storages.
//!
//! One storage per (archetype, component-type) pair. All storages inside an
//! archetype share the same per-chunk element capacity, so chunk boundaries
//! align across components.

use core::any::Any;

use crate::component::ComponentInfo;
use crate::{ChunkId, ComponentId};

pub mod dense;

pub use dense::DenseStorage;

/// Type-erased component storage.
///
/// The storage is a sequence of chunks. Each chunk holds exactly
/// [`Self::chunk_capacity`] slots of `T`. The slots `[0, len)` are
/// initialized; the slots `[len, chunk_capacity)` are logically uninitialized
/// (they may contain stale bits from previously moved-out values, but must
/// not be dropped).
///
/// # Invariants
///
/// - Every chunk has the same element count, fixed by [`Self::init_capacity`].
/// - `len <= chunk_count * chunk_capacity`.
/// - If `len > 0`, then `chunk_count >= 1` and `chunk_capacity > 0`.
pub trait Storage: Send + Sync + 'static {
    /// Returns the metadata of the underlying component type.
    fn info(&self) -> &ComponentInfo;

    /// Returns the number of initialized elements.
    fn len(&self) -> u32;

    /// Returns `true` if no elements are stored.
    #[inline]
    #[must_use]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of allocated chunks.
    fn chunk_count(&self) -> u32;

    /// Returns the per-chunk element capacity. `0` if not yet initialized.
    fn chunk_capacity(&self) -> u32;

    /// Sets the per-chunk capacity. Called exactly once by the owning
    /// archetype, right after the storage is created.
    ///
    /// # Panics
    ///
    /// Panics if `capacity == 0` or if the capacity has already been set.
    fn init_capacity(&mut self, capacity: u32);

    /// Ensures that at least `chunk_count` chunks are allocated.
    ///
    /// # Panics
    ///
    /// Panics if [`Self::init_capacity`] has not been called yet.
    fn grow_to(&mut self, chunk_count: u32);

    /// Returns a raw pointer to the first element of the given chunk.
    ///
    /// # Safety
    ///
    /// `chunk < self.chunk_count()`.
    unsafe fn chunk_ptr(&self, chunk: ChunkId) -> *const u8;

    /// Returns a mutable raw pointer to the first element of the given chunk.
    ///
    /// # Safety
    ///
    /// - `chunk < self.chunk_count()`.
    /// - No other reference may alias this chunk while the returned pointer
    ///   is in use.
    unsafe fn chunk_ptr_mut(&mut self, chunk: ChunkId) -> *mut u8;

    /// Moves `*data` into the slot at `(chunk, row)`.
    ///
    /// # Safety
    ///
    /// - `chunk < self.chunk_count()`.
    /// - `row < self.chunk_capacity()`.
    /// - The slot is uninitialized.
    /// - `data` points to a valid, initialized value of the component type.
    unsafe fn write_raw(&mut self, chunk: ChunkId, row: u32, data: *const u8);

    /// Drops the value at `(chunk, row)`.
    ///
    /// # Safety
    ///
    /// - `chunk < self.chunk_count()`.
    /// - `row < self.chunk_capacity()`.
    /// - The slot is initialized.
    unsafe fn drop_raw(&mut self, chunk: ChunkId, row: u32);

    /// Moves a value from `(src_chunk, src_row)` to `(dst_chunk, dst_row)`.
    ///
    /// The source slot is left uninitialized; the destination slot must be
    /// uninitialized before the call.
    ///
    /// # Safety
    ///
    /// - Both chunks are valid, both rows are within capacity.
    /// - Source is initialized, destination is uninitialized.
    unsafe fn move_raw(
        &mut self,
        src_chunk: ChunkId,
        src_row: u32,
        dst_chunk: ChunkId,
        dst_row: u32,
    );

    /// Overrides the tracked length.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that all slots in `[0, len)` are actually
    /// initialized and that no initialized slot lies outside this range.
    unsafe fn set_len(&mut self, len: u32);

    /// Upcasts to [`Any`], for downcasting to a concrete `DenseStorage<T>`.
    fn as_any(&self) -> &dyn Any;

    /// Upcasts to [`Any`] mutably.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Returns the identifier of the component stored in `storage`.
#[inline]
#[must_use]
pub fn storage_component(storage: &dyn Storage) -> ComponentId {
    storage.info().id()
}
