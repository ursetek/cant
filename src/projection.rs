//! Chunk-level projection: moving data between the world and external
//! consumers.
//!
//! [`ChunkSink`] receives chunks from the world: once per chunk, the world
//! passes a [`RawChunk`] with flat, byte-level views of the entities and
//! each column. Consumers copy the bytes out, forward the pointers to
//! another API (GPU, WASM, network), or hash them for serialization.
//!
//! [`ChunkSource`] is the reverse: the world reserves rows in a target
//! archetype and calls [`ChunkSource::produce`] to fill them. Loaders,
//! deserializers, and script bindings implement it.
//!
//! Both traits are byte-level: no per-column generics, no type parameters
//! at the trait boundary. Type information lives in [`Layout`] and in the
//! caller's knowledge of what it registered.

use core::alloc::Layout;
use core::ops::ControlFlow;
use core::ptr::NonNull;

use crate::query::Query;
use crate::world::archetype_index;
use crate::{ArchetypeId, ChunkId, ComponentId, Entity, Location, World};

// ---------------------------------------------------------------------------
// Read side: ChunkSink
// ---------------------------------------------------------------------------

/// Read-only byte-level view of one component column inside a chunk.
///
/// Constructible only by this module; every instance carries a valid pointer
/// to `len` rows of `layout.size()` bytes each. Zero-sized columns report an
/// empty byte slice regardless of `len`.
pub struct RawColumn {
    component: ComponentId,
    ptr: *const u8,
    len: u32,
    layout: Layout,
}

impl RawColumn {
    /// Component identifier of this column.
    #[inline]
    #[must_use]
    pub fn component(&self) -> ComponentId {
        self.component
    }

    /// Number of rows in this column.
    #[inline]
    #[must_use]
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Returns `true` if the column carries no rows.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Layout of the component type.
    #[inline]
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Read-only byte view of every row, tightly packed.
    ///
    /// For non-zero-sized components the returned slice has length
    /// `layout.size() * len`, with row `i` occupying
    /// `[i * size, (i + 1) * size)`. Zero-sized columns yield an empty slice.
    #[inline]
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        let size = self.layout.size();
        if size == 0 {
            return &[];
        }
        // SAFETY: the constructor guarantees `ptr` points to `size * len`
        // initialized bytes, and the returned slice cannot outlive `self`
        // (whose lifetime is bounded by the projection call).
        unsafe {
            core::slice::from_raw_parts(self.ptr, size * self.len as usize)
        }
    }
}

impl core::fmt::Debug for RawColumn {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RawColumn")
            .field("component", &self.component)
            .field("len", &self.len)
            .field("layout", &self.layout)
            .field("ptr", &self.ptr)
            .finish()
    }
}

/// Read-only flat view of one chunk.
///
/// `columns` is ordered exactly as the query's
/// [`access`](crate::QueryMask::access) list.
pub struct RawChunk<'w> {
    archetype: ArchetypeId,
    chunk: ChunkId,
    entities: &'w [Entity],
    columns: &'w [RawColumn],
}

impl<'w> RawChunk<'w> {
    /// Archetype this chunk belongs to.
    #[inline]
    #[must_use]
    pub fn archetype(&self) -> ArchetypeId {
        self.archetype
    }

    /// Chunk identifier inside the archetype.
    #[inline]
    #[must_use]
    pub fn chunk(&self) -> ChunkId {
        self.chunk
    }

    /// Entities in row order.
    #[inline]
    #[must_use]
    pub fn entities(&self) -> &'w [Entity] {
        self.entities
    }

    /// Columns, in query-access order.
    #[inline]
    #[must_use]
    pub fn columns(&self) -> &'w [RawColumn] {
        self.columns
    }

    /// Number of rows in this chunk.
    ///
    /// # Panics
    ///
    /// Panics if the row count exceeds `u32::MAX`. Unreachable in practice:
    /// chunk row counts are bounded by the archetype's chunk capacity.
    #[inline]
    #[must_use]
    pub fn len(&self) -> u32 {
        u32::try_from(self.entities.len()).expect("chunk len overflow")
    }

    /// Returns `true` if the chunk is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }
}

impl core::fmt::Debug for RawChunk<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RawChunk")
            .field("archetype", &self.archetype)
            .field("chunk", &self.chunk)
            .field("entities", &self.entities.len())
            .field("columns", &self.columns.len())
            .finish()
    }
}

/// Consumer of chunk data.
///
/// Implementations receive one [`RawChunk`] per call, in archetype-then-chunk
/// order, and must not retain any pointer or reference beyond the call.
pub trait ChunkSink {
    /// Consumes one chunk.
    ///
    /// Returns [`ControlFlow::Break`] to stop the projection early. The
    /// projected world is not mutated by this call; the sink must not write
    /// through any pointer in `chunk`.
    fn consume(&mut self, chunk: RawChunk<'_>) -> ControlFlow<()>;
}

// ---------------------------------------------------------------------------
// Write side: ChunkSource
// ---------------------------------------------------------------------------

/// Mutable byte-level view of one component column inside a chunk.
///
/// Constructible only by this module. Every instance carries a valid pointer
/// to `len` rows of `layout.size()` bytes each, all uninitialized. The
/// source must fill every row with a valid value of the component type
/// before returning from [`ChunkSource::produce`].
pub struct RawColumnMut {
    component: ComponentId,
    ptr: *mut u8,
    len: u32,
    layout: Layout,
}

impl RawColumnMut {
    /// Component identifier of this column.
    #[inline]
    #[must_use]
    pub fn component(&self) -> ComponentId {
        self.component
    }

    /// Number of rows in this column.
    #[inline]
    #[must_use]
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Returns `true` if the column carries no rows.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Layout of the component type.
    #[inline]
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Mutable byte view of every row, tightly packed.
    ///
    /// Row `i` occupies `[i * size, (i + 1) * size)`. Zero-sized columns
    /// yield an empty slice.
    ///
    /// # Note
    ///
    /// The source must write bytes that form a valid value of the component
    /// type. Writing arbitrary bytes into a slot whose type has invariants
    /// (e.g. `NonZeroU32`) is a contract violation, later read as UB. The
    /// `unsafe fn produce` marker covers this.
    #[inline]
    #[must_use]
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        let size = self.layout.size();
        if size == 0 {
            return &mut [];
        }
        // SAFETY: constructor guarantees `ptr` points to `size * len`
        // uninitialized bytes; `&mut self` prevents aliasing across calls.
        unsafe {
            core::slice::from_raw_parts_mut(self.ptr, size * self.len as usize)
        }
    }
}

impl core::fmt::Debug for RawColumnMut {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RawColumnMut")
            .field("component", &self.component)
            .field("len", &self.len)
            .field("layout", &self.layout)
            .field("ptr", &self.ptr)
            .finish()
    }
}

/// Mutable flat view of one chunk, presented to a [`ChunkSource`].
pub struct RawChunkMut<'w> {
    archetype: ArchetypeId,
    chunk: ChunkId,
    columns: &'w mut [RawColumnMut],
}

impl RawChunkMut<'_> {
    /// Archetype receiving the data.
    #[inline]
    #[must_use]
    pub fn archetype(&self) -> ArchetypeId {
        self.archetype
    }

    /// Chunk identifier inside the archetype.
    #[inline]
    #[must_use]
    pub fn chunk(&self) -> ChunkId {
        self.chunk
    }

    /// Columns to fill, one per component of the archetype, in the
    /// archetype's component order.
    #[inline]
    #[must_use]
    pub fn columns(&self) -> &[RawColumnMut] {
        self.columns
    }

    /// Mutable column slice.
    #[inline]
    #[must_use]
    pub fn columns_mut(&mut self) -> &mut [RawColumnMut] {
        self.columns
    }

    /// Number of rows to fill.
    #[inline]
    #[must_use]
    pub fn len(&self) -> u32 {
        self.columns.first().map_or(0, RawColumnMut::len)
    }

    /// Returns `true` if the view has no rows.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl core::fmt::Debug for RawChunkMut<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RawChunkMut")
            .field("archetype", &self.archetype)
            .field("chunk", &self.chunk)
            .field("columns", &self.columns.len())
            .field("len", &self.len())
            .finish()
    }
}

/// Producer of chunk data.
///
/// Sources are consumed in chunk-sized slices. The world calls
/// [`Self::next_chunk_len`] with the remaining capacity of the current
/// chunk; the source returns the number of rows it will fill (possibly
/// zero, meaning end-of-source). It then calls [`Self::produce`] with a
/// mutable view of the reserved rows.
pub trait ChunkSource {
    /// Returns the number of rows the source will produce in the next chunk.
    ///
    /// `max_rows` is the remaining capacity of the current chunk. Returns
    /// `0` to signal that the source is exhausted.
    ///
    /// # Panics
    ///
    /// Must not return a value greater than `max_rows`; doing so causes the
    /// world to panic before calling [`Self::produce`].
    fn next_chunk_len(&mut self, max_rows: u32) -> u32;

    /// Fills `chunk` with fresh component values.
    ///
    /// # Safety
    ///
    /// - Every column of `chunk` must be fully written before returning:
    ///   all `chunk.len()` rows of every column must contain valid values of
    ///   the corresponding component type.
    /// - No pointer from `chunk` may be retained after return.
    unsafe fn produce(&mut self, chunk: RawChunkMut<'_>);
}

// ---------------------------------------------------------------------------
// Query::project
// ---------------------------------------------------------------------------

impl Query {
    /// Projects every matching chunk into `sink`.
    ///
    /// Chunks are visited in archetype order, then in chunk order within each
    /// archetype. The sink is called once per chunk; returning
    /// [`ControlFlow::Break`] stops the projection.
    ///
    /// # Panics
    ///
    /// Panics if the query was prepared against a different world, or if the
    /// query's archetype cache is stale relative to `world`. Preparing the
    /// query against the same world and not mutating the world's archetype
    /// set since preparation is the caller's responsibility.
    pub fn project<S: ChunkSink>(
        &self,
        world: &World,
        sink: &mut S,
    ) -> ControlFlow<()> {
        let access = self.mask().access();
        let mut scratch: Vec<RawColumn> = Vec::with_capacity(access.len());

        for &arch_id in self.archetypes() {
            let arch = world.archetype(arch_id);
            let cap = arch.chunk_capacity() as usize;
            let total = arch.entities().len();

            // Precompute slot indices once per archetype; the per-chunk loop
            // then skips the binary search entirely.
            let slots: Vec<usize> = access
                .iter()
                .map(|(c, _)| {
                    arch.storage_index_of(*c).expect(
                        "query matched archetype missing a required component",
                    )
                })
                .collect();

            let chunk_count = arch.chunk_count();
            for chunk_index in 0..chunk_count {
                let chunk = ChunkId::from_raw(chunk_index);
                let start = chunk_index as usize * cap;
                let len = (total - start).min(cap);
                if len == 0 {
                    break;
                }
                let entities = &arch.entities()[start..start + len];
                #[allow(clippy::cast_possible_truncation)]
                let len_u32 = len as u32;

                scratch.clear();
                for ((comp, _), slot) in access.iter().zip(slots.iter()) {
                    let storage = arch.storage_at(*slot);
                    let layout = storage.info().layout();
                    let ptr = if layout.size() == 0 {
                        NonNull::<u8>::dangling().as_ptr()
                    } else {
                        // SAFETY: `chunk < chunk_count` and `slot` was
                        // computed for this archetype.
                        unsafe { storage.chunk_ptr(chunk) }
                    };
                    scratch.push(RawColumn {
                        component: *comp,
                        ptr,
                        len: len_u32,
                        layout,
                    });
                }

                let view = RawChunk {
                    archetype: arch_id,
                    chunk,
                    entities,
                    columns: &scratch,
                };

                if let ControlFlow::Break(b) = sink.consume(view) {
                    return ControlFlow::Break(b);
                }
            }
        }
        ControlFlow::Continue(())
    }
}

// ---------------------------------------------------------------------------
// World::ingest
// ---------------------------------------------------------------------------

impl World {
    /// Spawns entities from `source`, placing them in `archetype`.
    ///
    /// The archetype determines the column set. The source fills every
    /// column of every spawned row; its [`ChunkSource::next_chunk_len`]
    /// method controls how many rows are written per chunk (bounded by the
    /// chunk's remaining capacity).
    ///
    /// Returns the total number of entities created.
    ///
    /// # Panics
    ///
    /// Panics if `archetype` was not produced by this world, or if the source
    /// returns `next_chunk_len > max_rows`.
    pub fn ingest<S: ChunkSource>(
        &mut self,
        archetype: ArchetypeId,
        source: &mut S,
    ) -> u32 {
        // Split disjoint field borrows: `archetypes` and `entities` are
        // touched independently in the loop body.
        let archetypes = &mut self.archetypes;
        let entities = &mut self.entities;
        let arch = &mut archetypes[archetype_index(archetype)];
        let cap = arch.chunk_capacity();
        let component_count = arch.components().len();
        let mut total = 0u32;

        loop {
            let current_row = arch.len();
            let chunk_index = current_row / cap;
            let start_in_chunk = current_row % cap;
            let max_rows = cap - start_in_chunk;

            let want = source.next_chunk_len(max_rows);
            if want == 0 {
                break;
            }
            assert!(
                want <= max_rows,
                "ChunkSource::next_chunk_len returned {want} rows but only \
                 {max_rows} fit in the current chunk",
            );

            // Reserve `want` fresh entities in the target archetype.
            for i in 0..want {
                let e = entities.alloc();
                let row = arch.reserve_row(e);
                debug_assert_eq!(row, current_row + i);
                entities.set_location(e, Location { archetype, row });
            }

            // Build a mutable view of rows
            // `[start_in_chunk, start_in_chunk + want)` of `chunk_index`.
            let chunk = ChunkId::from_raw(chunk_index);
            let mut columns: Vec<RawColumnMut> =
                Vec::with_capacity(component_count);
            for slot in 0..component_count {
                let comp = arch.components()[slot];
                let layout = arch.storage_at(slot).info().layout();
                let ptr = if layout.size() == 0 {
                    NonNull::<u8>::dangling().as_ptr()
                } else {
                    #[allow(clippy::cast_possible_truncation)]
                    let offset = start_in_chunk as usize * layout.size();
                    // SAFETY: `slot < component_count`, and `chunk` was
                    // derived from `arch.len()`, so the chunk exists. The
                    // borrow of `arch` ends at this call.
                    unsafe { arch.chunk_base_mut(slot, chunk) }
                        .wrapping_add(offset)
                };
                columns.push(RawColumnMut {
                    component: comp,
                    ptr,
                    len: want,
                    layout,
                });
            }

            let view = RawChunkMut {
                archetype,
                chunk,
                columns: &mut columns,
            };

            // SAFETY: every reserved slot in every column is uninitialized;
            // the source contract requires it to fill them all.
            unsafe { source.produce(view) };

            // SAFETY: the source contract guarantees every slot in the newly
            // reserved rows is now initialized.
            unsafe { arch.commit_row() };

            total += want;
        }

        total
    }
}
