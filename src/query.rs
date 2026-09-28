//! Querying the world: masks, prepared queries, chunked iteration.
//!
//! # Iteration model
//!
//! Queries iterate **chunks**, not entities. Each chunk is presented to the
//! caller as a [`ChunkView`]: a flat bundle of typed slices
//! (`entities`, `col_a`, `col_b`, ...) that share the same row range. This is
//! the shape external consumers (WASM modules, editors, GPU bridges) also
//! consume.
//!
//! Iteration is driven by a closure, not by [`Iterator`]. A lending iterator
//! over `(&mut A, &B)` is not expressible on stable Rust without GAT tricks;
//! a closure sidesteps the problem entirely, keeps the borrow local to each
//! call, and lets the compiler inline and vectorize the loop body.

use core::any::TypeId;
use core::marker::PhantomData;
use core::ops::ControlFlow;
use std::alloc::Layout;
use std::sync::Arc;

use crate::archetype::Archetype;
use crate::{ArchetypeId, ChunkId, ComponentId, ComponentMask, Entity, World};

/// Whether a query reads or writes a component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    /// Shared access: `&T`.
    Read,
    /// Exclusive access: `&mut T`.
    Write,
}

/// Error returned by [`World::prepare`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryError {
    /// The mask references a component that was never registered.
    UnknownComponent(ComponentId),
}

impl core::fmt::Display for QueryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownComponent(c) => write!(f, "unknown component {c:?}"),
        }
    }
}

impl std::error::Error for QueryError {}

/// Describes which components a query requires, excludes, and how it accesses
/// them.
///
/// A mask is built once, at prepare time, and never touched during iteration.
#[derive(Clone, Debug, Default)]
pub struct QueryMask {
    pub(crate) required: ComponentMask,
    pub(crate) excluded: ComponentMask,
    pub(crate) access: Vec<(ComponentId, Access)>,
}

impl QueryMask {
    /// Creates an empty mask.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requires shared (`&T`) access to `id`.
    pub fn read(&mut self, id: ComponentId) -> &mut Self {
        self.required.insert(id);
        self.push_access(id, Access::Read);
        self
    }

    /// Requires exclusive (`&mut T`) access to `id`.
    ///
    /// If `id` was already added with `read`, the access is upgraded to
    /// `write`.
    pub fn write(&mut self, id: ComponentId) -> &mut Self {
        self.required.insert(id);
        self.push_access(id, Access::Write);
        self
    }

    /// Excludes archetypes that carry `id`.
    pub fn exclude(&mut self, id: ComponentId) -> &mut Self {
        self.excluded.insert(id);
        self
    }

    fn push_access(&mut self, id: ComponentId, acc: Access) {
        if let Some(slot) = self.access.iter_mut().find(|(c, _)| *c == id) {
            if acc == Access::Write {
                slot.1 = Access::Write;
            }
        } else {
            self.access.push((id, acc));
        }
    }

    /// Required components.
    #[must_use]
    pub fn required(&self) -> &ComponentMask {
        &self.required
    }

    /// Excluded components.
    #[must_use]
    pub fn excluded(&self) -> &ComponentMask {
        &self.excluded
    }

    /// Access list in insertion order: `(ComponentId, Access)` pairs.
    #[must_use]
    pub fn access(&self) -> &[(ComponentId, Access)] {
        &self.access
    }

    /// Returns `true` if every component in the mask is accessed as `Read`.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.access.iter().all(|(_, a)| *a == Access::Read)
    }
}

/// A prepared query: mask plus cached archetype list.
///
/// The archetype list is refreshed lazily on the next iteration call if the
/// world's archetype set has changed since preparation. In a stable frame,
/// the refresh costs one `u64` comparison per iteration call — not per
/// chunk.
#[derive(Clone)]
pub struct Query {
    pub(crate) mask: QueryMask,
    pub(crate) archetypes: Arc<[ArchetypeId]>,
    pub(crate) version: u64,
}

impl Query {
    /// Returns the mask this query was prepared from.
    #[must_use]
    pub fn mask(&self) -> &QueryMask {
        &self.mask
    }

    /// Returns the archetypes the query matches as of the last refresh.
    ///
    /// May be stale if the world has grown archetypes since the last
    /// iteration call; use [`Self::for_each`] or another iteration method to
    /// trigger a refresh.
    #[must_use]
    pub fn archetypes(&self) -> &[ArchetypeId] {
        &self.archetypes
    }

    /// Re-fetches the archetype list if the world's version has changed.
    fn refresh(&mut self, world: &World) {
        if self.version == world.archetype_version {
            return;
        }
        self.archetypes =
            world.matching_archetypes(self.mask.required, self.mask.excluded);
        self.version = world.archetype_version;
    }

    /// Iterates every chunk in shared mode.
    ///
    /// # Panics
    ///
    /// Panics if the query's mask contains any write access.
    pub fn for_each<F>(&mut self, world: &World, f: F)
    where
        F: FnMut(ChunkView<'_>),
    {
        assert!(
            self.mask.is_read_only(),
            "for_each requires a read-only query; use for_each_mut",
        );
        self.refresh(world);
        // SAFETY: read-only query, shared world.
        unsafe { self.visit(UnsafeWorldCell::new(world), f) };
    }

    /// Iterates every chunk in mutable mode.
    pub fn for_each_mut<F>(&mut self, world: &mut World, f: F)
    where
        F: FnMut(ChunkView<'_>),
    {
        self.refresh(&*world);
        // SAFETY: exclusive borrow.
        unsafe { self.visit(UnsafeWorldCell::new_mut(world), f) };
    }

    /// Iterates every chunk in shared mode, with early exit.
    ///
    /// # Panics
    ///
    /// Panics if the query's mask contains any write access.
    pub fn try_for_each<F, B>(&mut self, world: &World, f: F) -> ControlFlow<B>
    where
        F: FnMut(ChunkView<'_>) -> ControlFlow<B>,
    {
        assert!(
            self.mask.is_read_only(),
            "try_for_each requires a read-only query",
        );
        self.refresh(world);
        // SAFETY: read-only query, shared world.
        unsafe { self.visit_control(UnsafeWorldCell::new(world), f) }
    }

    /// Iterates every chunk in mutable mode, with early exit.
    pub fn try_for_each_mut<F, B>(
        &mut self,
        world: &mut World,
        f: F,
    ) -> ControlFlow<B>
    where
        F: FnMut(ChunkView<'_>) -> ControlFlow<B>,
    {
        self.refresh(&*world);
        // SAFETY: exclusive borrow.
        unsafe { self.visit_control(UnsafeWorldCell::new_mut(world), f) }
    }

    /// Returns the first entity matching the predicate, if any.
    ///
    /// # Panics
    ///
    /// Same as [`Self::for_each`].
    pub fn find<F>(&mut self, world: &World, mut f: F) -> Option<Entity>
    where
        F: FnMut(&ChunkView<'_>) -> bool,
    {
        self.refresh(world);
        let mut found = None;
        // SAFETY: read-only query, shared world.
        let _: ControlFlow<()> = unsafe {
            self.visit_control(UnsafeWorldCell::new(world), |view| {
                if f(&view) {
                    found = view.entities().first().copied();
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })
        };
        found
    }

    /// Returns `true` if any chunk matches the predicate.
    ///
    /// # Panics
    ///
    /// Same as [`Self::for_each`].
    pub fn any<F>(&mut self, world: &World, mut f: F) -> bool
    where
        F: FnMut(&ChunkView<'_>) -> bool,
    {
        self.refresh(world);
        // SAFETY: read-only query, shared world.
        let cf: ControlFlow<()> = unsafe {
            self.visit_control(UnsafeWorldCell::new(world), |view| {
                if f(&view) {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })
        };
        cf.is_break()
    }

    /// Folds every chunk into an accumulator.
    ///
    /// # Panics
    ///
    /// Same as [`Self::for_each`].
    pub fn fold<B, F>(&mut self, world: &World, init: B, mut f: F) -> B
    where
        F: FnMut(B, &ChunkView<'_>) -> B,
    {
        self.refresh(world);
        // `Option::take` lets us move the accumulator out of the captured
        // state on each closure call, which `FnMut` does not permit
        // otherwise.
        let mut acc: Option<B> = Some(init);
        // SAFETY: read-only query, shared world.
        unsafe {
            self.visit(UnsafeWorldCell::new(world), |view| {
                let current =
                    acc.take().expect("fold state present before every call");
                acc = Some(f(current, &view));
            });
        }
        acc.expect("fold state present after visit")
    }

    /// Iterates chunks through a raw [`UnsafeWorldCell`].
    ///
    /// # Safety
    ///
    /// - If the mask contains write access, the caller must ensure the
    ///   underlying storage is not aliased by any other live reference.
    /// - The caller must not retain views beyond the call.
    pub unsafe fn for_each_cell<F>(&mut self, cell: UnsafeWorldCell<'_>, f: F)
    where
        F: FnMut(ChunkView<'_>),
    {
        // SAFETY: `cell` points to a live `World`; caller upholds the
        // aliasing contract.
        let world: &World = unsafe { &*cell.world };
        self.refresh(world);
        // SAFETY: caller upholds cell contract.
        unsafe { self.visit(cell, f) };
    }
}

impl core::fmt::Debug for Query {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Query")
            .field("mask", &self.mask)
            .field("archetypes", &self.archetypes.len())
            .finish_non_exhaustive()
    }
}

/// A flat, per-chunk view shared by every column of one archetype chunk.
///
/// The view owns nothing: it borrows the entities slice from the archetype
/// and holds raw pointers into the archetype's storages. Those pointers are
/// valid for as long as the `ChunkView` exists.
pub struct ChunkView<'w> {
    entities: &'w [Entity],
    columns: &'w [ColumnView],
    _marker: PhantomData<&'w ()>,
}

struct ColumnView {
    component: ComponentId,
    type_id: TypeId,
    is_opaque: bool,
    access: Access,
    ptr: *mut u8,
    len: u32,
    layout: Layout,
}

impl ColumnView {
    /// Returns `true` if the column can be accessed as `&[T]` / `&mut [T]`.
    #[inline]
    fn matches<T: 'static>(&self) -> bool {
        !self.is_opaque
            && self.type_id == TypeId::of::<T>()
            && self.layout.size() == core::mem::size_of::<T>()
    }
}

impl<'w> ChunkView<'w> {
    /// Entities in this chunk, in row order.
    #[must_use]
    pub fn entities(&self) -> &'w [Entity] {
        self.entities
    }

    /// Number of entities in this chunk.
    ///
    /// # Panics
    ///
    /// Panics if the row count exceeds `u32::MAX`. This is unreachable in
    /// practice: chunk row counts are bounded by the archetype's chunk
    /// capacity, which is derived from [`crate::archetype::DEFAULT_CHUNK_BYTES`].
    #[must_use]
    pub fn len(&self) -> u32 {
        u32::try_from(self.entities.len()).expect("chunk len overflow")
    }

    /// Returns `true` if the chunk holds no entities.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Number of columns exposed by this view.
    #[must_use]
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// Component identifier of column `i`.
    #[must_use]
    pub fn component_at(&self, i: usize) -> Option<ComponentId> {
        self.columns.get(i).map(|c| c.component)
    }

    /// Access kind of column `i`.
    #[must_use]
    pub fn access_at(&self, i: usize) -> Option<Access> {
        self.columns.get(i).map(|c| c.access)
    }

    /// Returns the column's storage layout.
    #[must_use]
    pub fn layout_at(&self, i: usize) -> Option<Layout> {
        self.columns.get(i).map(|c| c.layout)
    }

    /// Returns a shared typed slice for column `i`.
    ///
    /// Returns `None` if the column's type does not match `T` or if the
    /// column belongs to an opaque (runtime-registered) component.
    #[must_use]
    pub fn column<T: 'static>(&self, i: usize) -> Option<&[T]> {
        let col = self.columns.get(i)?;
        if !col.matches::<T>() {
            return None;
        }
        let n = col.len as usize;
        // SAFETY: `matches::<T>` verified the column's type. The pointer was
        // produced by `Storage::chunk_ptr`, so it is aligned and non-null;
        // the length is bounded by the chunk's row count.
        Some(unsafe { core::slice::from_raw_parts(col.ptr.cast::<T>(), n) })
    }

    /// Returns an exclusive typed slice for column `i`.
    ///
    /// Returns `None` if the column's type does not match `T`, if the column
    /// belongs to an opaque component, or if the column was not prepared
    /// with [`Access::Write`].
    ///
    /// The returned slice borrows `self` mutably, so at most one mutable
    /// column can be live at a time. Use [`Self::esc_column_mut`] to obtain
    /// two mutable columns simultaneously.
    #[must_use]
    pub fn column_mut<T: 'static>(&mut self, i: usize) -> Option<&mut [T]> {
        let col = self.columns.get(i)?;
        if col.access != Access::Write {
            return None;
        }
        if !col.matches::<T>() {
            return None;
        }
        let n = col.len as usize;
        // SAFETY: type and access verified; `&mut self` prevents any other
        // accessor from being live while the slice exists.
        Some(unsafe { core::slice::from_raw_parts_mut(col.ptr.cast::<T>(), n) })
    }

    /// Escape hatch: shared typed slice for column `i` through a shared
    /// borrow.
    ///
    /// The returned lifetime is the view's own (`'w`), decoupled from
    /// `&self`. This lets two escaped slices coexist; the borrow checker
    /// cannot police aliasing across them, hence `unsafe`.
    ///
    /// # Safety
    ///
    /// - `T` must be the actual type stored in column `i`.
    /// - If two escaped slices overlap in memory (same column of the same
    ///   chunk), the caller must not mutate through either of them.
    #[must_use]
    pub unsafe fn esc_column<T: 'static>(&self, i: usize) -> Option<&'w [T]> {
        let col = self.columns.get(i)?;
        if !col.matches::<T>() {
            return None;
        }
        let n = col.len as usize;
        // SAFETY: type verified; caller guarantees no aliasing.
        Some(unsafe { core::slice::from_raw_parts(col.ptr.cast::<T>(), n) })
    }

    /// Escape hatch: exclusive typed slice for column `i` through a shared
    /// borrow.
    ///
    /// Escape hatch for callers that need two mutable columns at once and
    /// can statically prove they do not alias. The returned slice's lifetime
    /// is the view's own lifetime, not the borrow of `self`, so multiple
    /// calls can coexist.
    ///
    /// # Safety
    ///
    /// The caller must guarantee:
    ///
    /// - `T` is the component type actually stored in column `i`.
    /// - The column's access kind is [`Access::Write`].
    /// - No other reference (shared or mutable) to the same column exists for
    ///   the lifetime of the returned slice.
    /// - If two calls return slices from the same chunk, they refer to
    ///   distinct columns.
    #[must_use]
    pub unsafe fn esc_column_mut<T: 'static>(
        &self,
        i: usize,
    ) -> Option<&'w mut [T]> {
        let col = self.columns.get(i)?;
        if col.access != Access::Write {
            return None;
        }
        if !col.matches::<T>() {
            return None;
        }
        let n = col.len as usize;
        // SAFETY: type and access verified; caller guarantees no aliasing.
        Some(unsafe { core::slice::from_raw_parts_mut(col.ptr.cast::<T>(), n) })
    }
}

impl core::fmt::Debug for ChunkView<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ChunkView")
            .field("entities", &self.entities.len())
            .field("columns", &self.columns.len())
            .finish()
    }
}

/// A shared-yet-mutable handle into a [`World`].
///
/// `UnsafeWorldCell` allows multiple concurrent views into disjoint parts of
/// the world. The caller is responsible for ensuring those views do not
/// alias: same archetype + same component means potential conflict; different
/// archetypes or different components never conflict.
#[derive(Clone, Copy)]
pub struct UnsafeWorldCell<'w> {
    world: *mut World,
    _marker: PhantomData<&'w ()>,
}

impl<'w> UnsafeWorldCell<'w> {
    /// Creates a cell from a shared borrow.
    ///
    /// The cell can mutate through the pointer; the caller must uphold the
    /// invariant that no aliasing reference exists for the same data.
    #[must_use]
    pub fn new(world: &'w World) -> Self {
        Self {
            world: core::ptr::from_ref(world).cast_mut(),
            _marker: PhantomData,
        }
    }

    /// Creates a cell from an exclusive borrow.
    #[must_use]
    pub fn new_mut(world: &'w mut World) -> Self {
        Self {
            world: core::ptr::from_mut(world),
            _marker: PhantomData,
        }
    }

    /// Returns a shared reference to a resource.
    ///
    /// # Safety
    ///
    /// The caller must guarantee no other live reference to the same resource
    /// exists. The returned reference's lifetime is tied to the cell, not to
    /// the borrow of `self`, so it can outlive intermediate calls.
    #[must_use]
    pub unsafe fn resource<T: 'static>(
        &self,
        id: crate::ResourceId,
    ) -> Option<&'w T> {
        // SAFETY: caller guarantees no aliasing.
        let world: &'w World = unsafe { &*self.world };
        world.resource::<T>(id).ok()
    }

    /// Returns an exclusive reference to a resource.
    ///
    /// # Safety
    ///
    /// The caller must guarantee no other live reference — shared or
    /// mutable — to the same resource exists.
    #[must_use]
    pub unsafe fn resource_mut<T: 'static>(
        &self,
        id: crate::ResourceId,
    ) -> Option<&'w mut T> {
        // SAFETY: caller guarantees no aliasing; `self.world` points to a
        // live `World`.
        let world: &'w mut World = unsafe { &mut *self.world };
        world.resource_mut::<T>(id).ok()
    }

    /// Constructs a cell from a raw pointer.
    ///
    /// # Safety
    ///
    /// `world` must point to a valid, live `World` whose lifetime outlives
    /// every use of the returned cell.
    #[must_use]
    pub(crate) unsafe fn from_raw(world: *mut World) -> Self {
        Self {
            world,
            _marker: PhantomData,
        }
    }
}

impl core::fmt::Debug for UnsafeWorldCell<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UnsafeWorldCell")
            .field("world", &self.world)
            .finish()
    }
}

impl Query {
    unsafe fn visit<F>(&self, cell: UnsafeWorldCell<'_>, mut f: F)
    where
        F: FnMut(ChunkView<'_>),
    {
        // SAFETY: caller upholds the aliasing contract of `cell`.
        let _: ControlFlow<()> = unsafe {
            self.visit_control(cell, |v| {
                f(v);
                ControlFlow::Continue(())
            })
        };
    }

    unsafe fn visit_control<F, B>(
        &self,
        cell: UnsafeWorldCell<'_>,
        mut f: F,
    ) -> ControlFlow<B>
    where
        F: FnMut(ChunkView<'_>) -> ControlFlow<B>,
    {
        // SAFETY: caller upholds the aliasing contract of `cell`.
        let world: &World = unsafe { &*cell.world };
        let access = self.mask.access();

        // Reused across every archetype: one column view per entry in
        // `access`, plus the storage-slot index for the archetype currently
        // being visited. Both vectors are cleared and refilled in place.
        let mut scratch: Vec<ColumnView> = Vec::with_capacity(access.len());
        let mut slots: Vec<usize> = Vec::with_capacity(access.len());

        for &arch_id in self.archetypes.iter() {
            let arch = world.archetype(arch_id);

            // Hoist the per-archetype storage lookup out of the chunk loop.
            // `storage_index_of` is a binary search over `arch.components()`;
            // doing it once per archetype instead of once per (chunk,
            // component) is the main cost the direct-indexing API buys.
            slots.clear();
            for (comp, _) in access {
                let slot = arch.storage_index_of(*comp).expect(
                    "query matched an archetype missing a required component",
                );
                slots.push(slot);
            }

            let chunk_count = arch.chunk_count();
            for chunk_index in 0..chunk_count {
                let chunk = ChunkId::from_raw(chunk_index);
                scratch.clear();
                // SAFETY: built for this (archetype, chunk) pair; columns
                // point into the archetype's storages and are valid for the
                // duration of the closure call.
                let entities = unsafe {
                    fill_columns(arch, chunk, access, &slots, &mut scratch)
                };
                let view = ChunkView {
                    entities,
                    columns: &scratch,
                    _marker: PhantomData,
                };
                match f(view) {
                    ControlFlow::Continue(()) => {}
                    ControlFlow::Break(b) => return ControlFlow::Break(b),
                }
            }
        }
        ControlFlow::Continue(())
    }
}

/// Fills `out` with one [`ColumnView`] per entry in `access` and returns the
/// entities slice for the given chunk.
///
/// `slots[i]` must be the storage-slot index of `access[i].0` inside `arch`;
/// the caller is expected to have computed these once per archetype via
/// [`Archetype::storage_index_of`].
///
/// # Safety
///
/// - `chunk < arch.chunk_count()`.
/// - `slots.len() == access.len()`.
/// - No aliasing reference to any of the touched storages may exist for the
///   lifetime of the produced views.
unsafe fn fill_columns<'w>(
    arch: &'w Archetype,
    chunk: ChunkId,
    access: &[(ComponentId, Access)],
    slots: &[usize],
    out: &mut Vec<ColumnView>,
) -> &'w [Entity] {
    debug_assert_eq!(slots.len(), access.len());

    let cap = arch.chunk_capacity() as usize;
    let start = chunk.index() * cap;
    let total = arch.entities().len();
    let len = total.saturating_sub(start).min(cap);
    let entities = &arch.entities()[start..start + len];

    for ((comp, acc), slot) in access.iter().zip(slots.iter()) {
        let storage = arch.storage_at(*slot);
        let info = storage.info();
        // SAFETY: `chunk < chunk_count` by the caller's contract.
        let ptr = unsafe { storage.chunk_ptr(chunk) }.cast_mut();
        out.push(ColumnView {
            component: *comp,
            type_id: info.type_id(),
            is_opaque: matches!(
                info.kind(),
                crate::component::ComponentKind::Opaque
            ),
            access: *acc,
            ptr,
            len: u32::try_from(len).expect("chunk row count overflow"),
            layout: info.layout(),
        });
    }
    entities
}
