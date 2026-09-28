//! Dense SoA storage backed by per-chunk `Box<[MaybeUninit<T>]>`.

use core::any::Any;
use core::fmt;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr;

use crate::component::{ComponentInfo, ComponentKind};
use crate::storage::Storage;
use crate::{ChunkId, ComponentId};

/// Default target byte size for a single chunk.
///
/// Actual chunk capacity is chosen by the archetype so that
/// `capacity * size_of::<T>()` is close to this value. The archetype takes
/// the minimum across all its components.
pub const DEFAULT_CHUNK_BYTES: usize = 16 * 1024;

/// Dense struct-of-arrays storage for a single component type.
///
/// Each chunk holds `chunk_capacity` values of `T` in a contiguous
/// `Box<[MaybeUninit<T>]>`. The first `len` slots of the storage as a whole
/// are initialized; the rest are logically uninitialized.
pub struct DenseStorage<T> {
    info: ComponentInfo,
    chunks: Vec<Box<[MaybeUninit<T>]>>,
    chunk_capacity: u32,
    len: u32,
    _marker: PhantomData<fn() -> T>,
}

impl<T: 'static> DenseStorage<T> {
    /// Convenience constructor: derives [`ComponentInfo`] from `T`.
    #[must_use]
    pub fn new(id: ComponentId, name: &'static str) -> Self {
        let kind = if core::mem::size_of::<T>() == 0 {
            ComponentKind::Zst
        } else {
            ComponentKind::Dense
        };
        Self::new_with_info(ComponentInfo::of::<T>(id, name, kind))
    }

    /// Constructor reusing an existing [`ComponentInfo`].
    ///
    /// Used by [`crate::World`] to avoid recomputing type metadata for every
    /// archetype that includes this component.
    ///
    /// # Panics
    ///
    /// Panics if `info.layout().size() != size_of::<T>()` or if the size
    /// mismatch implies a wrong `Zst`/`Dense` classification.
    #[must_use]
    pub fn new_with_info(info: ComponentInfo) -> Self {
        assert_eq!(
            info.layout().size(),
            core::mem::size_of::<T>(),
            "ComponentInfo layout does not match storage type",
        );
        Self {
            info,
            chunks: Vec::new(),
            chunk_capacity: 0,
            len: 0,
            _marker: PhantomData,
        }
    }

    /// Returns a typed slice of the given chunk's initialized elements.
    ///
    /// This is the fast path used by native queries; it avoids `dyn` entirely.
    ///
    /// # Panics
    ///
    /// Panics if `chunk >= chunk_count()`.
    #[inline]
    #[must_use]
    pub fn chunk_slice(&self, chunk: ChunkId) -> &[T] {
        let chunk_data = &self.chunks[chunk.index()];
        let n = self.initialized_in(chunk.index());
        // SAFETY: the first `n` slots of this chunk are initialized.
        unsafe {
            core::slice::from_raw_parts(chunk_data.as_ptr().cast::<T>(), n)
        }
    }

    /// Returns a mutable typed slice of the given chunk's initialized
    /// elements.
    ///
    /// # Panics
    ///
    /// Panics if `chunk >= chunk_count()`.
    #[inline]
    pub fn chunk_slice_mut(&mut self, chunk: ChunkId) -> &mut [T] {
        let cap = self.chunk_capacity as usize;
        let n = {
            let len = self.len as usize;
            let idx = chunk.index();
            let start = idx * cap;
            len.saturating_sub(start).min(cap)
        };
        let chunk = &mut self.chunks[chunk.index()];
        // SAFETY: the first `n` slots of this chunk are initialized, and we
        // hold an exclusive borrow of the storage.
        unsafe {
            core::slice::from_raw_parts_mut(chunk.as_mut_ptr().cast::<T>(), n)
        }
    }

    /// Number of initialized slots in the given chunk.
    fn initialized_in(&self, chunk_index: usize) -> usize {
        let cap = self.chunk_capacity as usize;
        let len = self.len as usize;
        let start = chunk_index * cap;
        len.saturating_sub(start).min(cap)
    }
}

impl<T: Send + Sync + 'static> Storage for DenseStorage<T> {
    fn info(&self) -> &ComponentInfo {
        &self.info
    }

    fn len(&self) -> u32 {
        self.len
    }

    fn chunk_count(&self) -> u32 {
        // `chunks.len() <= u32::MAX`: chunks are added one at a time and each
        // holds at least one element, so the total is bounded by the number of
        // entities.
        #[allow(clippy::cast_possible_truncation)]
        {
            self.chunks.len() as u32
        }
    }

    fn chunk_capacity(&self) -> u32 {
        self.chunk_capacity
    }

    fn init_capacity(&mut self, capacity: u32) {
        assert!(capacity > 0, "chunk capacity must be non-zero");
        assert!(
            self.chunk_capacity == 0,
            "chunk capacity already initialized",
        );
        self.chunk_capacity = capacity;
    }

    fn grow_to(&mut self, chunk_count: u32) {
        assert!(
            self.chunk_capacity > 0,
            "init_capacity must be called before grow_to",
        );
        let cap = self.chunk_capacity as usize;
        while self.chunks.len() < chunk_count as usize {
            let chunk: Box<[MaybeUninit<T>]> =
                (0..cap).map(|_| MaybeUninit::uninit()).collect();
            self.chunks.push(chunk);
        }
    }

    unsafe fn chunk_ptr(&self, chunk: ChunkId) -> *const u8 {
        self.chunks[chunk.index()].as_ptr().cast::<u8>()
    }

    unsafe fn chunk_ptr_mut(&mut self, chunk: ChunkId) -> *mut u8 {
        self.chunks[chunk.index()].as_mut_ptr().cast::<u8>()
    }

    unsafe fn write_raw(&mut self, chunk: ChunkId, row: u32, data: *const u8) {
        let cap = self.chunk_capacity;
        assert!(row < cap, "row out of chunk capacity");
        let slot = self.chunks[chunk.index()][row as usize].as_mut_ptr();
        // SAFETY: caller guarantees `data` points to a valid `T` and the slot
        // is uninitialized. `copy_nonoverlapping` performs a bitwise move.
        unsafe {
            ptr::copy_nonoverlapping(data.cast::<T>(), slot, 1);
        }
    }

    unsafe fn drop_raw(&mut self, chunk: ChunkId, row: u32) {
        let cap = self.chunk_capacity;
        assert!(row < cap, "row out of chunk capacity");
        // SAFETY: caller guarantees the slot is initialized.
        unsafe {
            self.chunks[chunk.index()][row as usize].assume_init_drop();
        }
    }

    unsafe fn move_raw(
        &mut self,
        src_chunk: ChunkId,
        src_row: u32,
        dst_chunk: ChunkId,
        dst_row: u32,
    ) {
        let cap = self.chunk_capacity;
        assert!(src_row < cap && dst_row < cap, "row out of chunk capacity");
        let src = self.chunks[src_chunk.index()][src_row as usize].as_ptr();
        let dst = self.chunks[dst_chunk.index()][dst_row as usize].as_mut_ptr();
        // SAFETY: caller guarantees source is initialized and destination is
        // uninitialized, and that they do not overlap.
        unsafe {
            ptr::copy_nonoverlapping(src, dst, 1);
        }
    }

    unsafe fn set_len(&mut self, len: u32) {
        let cap = self.chunk_capacity;
        let max = self.chunk_count().saturating_mul(cap);
        assert!(len <= max, "len exceeds allocated capacity");
        self.len = len;
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl<T> Drop for DenseStorage<T> {
    fn drop(&mut self) {
        let cap = self.chunk_capacity as usize;
        let mut remaining = self.len as usize;
        for chunk in &mut self.chunks {
            let n = remaining.min(cap);
            for slot in &mut chunk[..n] {
                // SAFETY: the first `n` slots of each chunk are initialized.
                unsafe { slot.assume_init_drop() };
            }
            remaining = remaining.saturating_sub(cap);
        }
    }
}

impl<T> fmt::Debug for DenseStorage<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DenseStorage")
            .field("info", &self.info)
            .field("chunks", &self.chunks.len())
            .field("chunk_capacity", &self.chunk_capacity)
            .field("len", &self.len)
            .finish()
    }
}
