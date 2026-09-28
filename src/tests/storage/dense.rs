//! Tests for `DenseStorage<T>`: the SoA chunked layout, its drop semantics,
//! and the low-level move/write/drop primitives used by archetype migration.

use core::mem::ManuallyDrop;
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::storage::{DenseStorage, Storage};
use crate::{ChunkId, ComponentId};

#[derive(Debug, PartialEq, Clone, Copy)]
struct P(f32, f32);

/// Builds a storage with room for exactly one chunk of `cap` elements.
fn make_p(cap: u32) -> DenseStorage<P> {
    let mut s = DenseStorage::<P>::new(ComponentId::from_raw(0), "P");
    s.init_capacity(cap);
    s.grow_to(1);
    s
}

/// Writes `value` into `(chunk, row)` without dropping the source. The
/// caller is responsible for forgetting `value`.
fn write_slot<T>(s: &mut DenseStorage<T>, chunk: ChunkId, row: u32, value: T)
where
    T: core::marker::Sync + core::marker::Send + 'static,
{
    let value = ManuallyDrop::new(value);
    let src = ptr::addr_of!(*value).cast::<u8>();
    // SAFETY: slot is uninitialized (test invariant), `src` points to a
    // valid `T`.
    unsafe { s.write_raw(chunk, row, src) };
}

/// Writing into a chunk and reading it back through the typed slice must
/// preserve the value bit-for-bit.
#[test]
fn write_then_read_roundtrip() {
    let mut s = make_p(4);
    write_slot(&mut s, ChunkId::from_raw(0), 0, P(1.0, 2.0));
    // SAFETY: exactly one slot was initialized above.
    unsafe { s.set_len(1) };

    assert_eq!(s.chunk_slice(ChunkId::from_raw(0)), &[P(1.0, 2.0)]);
}

/// Slots past `len` must not appear in `chunk_slice`, even after `grow_to`.
#[test]
fn chunk_slice_respects_len() {
    let mut s = make_p(4);
    write_slot(&mut s, ChunkId::from_raw(0), 0, P(1.0, 0.0));
    write_slot(&mut s, ChunkId::from_raw(0), 1, P(2.0, 0.0));
    // SAFETY: two slots were initialized above.
    unsafe { s.set_len(2) };

    assert_eq!(s.chunk_slice(ChunkId::from_raw(0)).len(), 2);
}

/// `move_raw` must transfer a value without running any destructor on the
/// source slot. This is the primitive used to relocate a component during
/// archetype migration.
#[test]
fn move_raw_transfers_value_and_leaves_source_uninit() {
    let mut s = make_p(4);
    write_slot(&mut s, ChunkId::from_raw(0), 0, P(1.0, 2.0));
    write_slot(&mut s, ChunkId::from_raw(0), 1, P(3.0, 4.0));
    // SAFETY: two slots initialized.
    unsafe { s.set_len(2) };

    // SAFETY: (0, 1) is initialized, (0, 0) is uninitialized after the
    // conceptual move — we drop the value at (0, 1) via the move itself.
    unsafe { s.move_raw(ChunkId::from_raw(0), 1, ChunkId::from_raw(0), 0) };
    // SAFETY: we now only consider the first slot initialized.
    unsafe { s.set_len(1) };

    assert_eq!(s.chunk_slice(ChunkId::from_raw(0)), &[P(3.0, 4.0)]);
}

/// `Drop` must destroy exactly `len` values, not the whole chunk capacity.
/// This catches the classic "forgot to bound by len" bug.
#[test]
fn drop_runs_for_initialized_prefix_only() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Tracked(#[allow(dead_code)] u32);
    impl Drop for Tracked {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    DROPS.store(0, Ordering::SeqCst);

    {
        let mut s =
            DenseStorage::<Tracked>::new(ComponentId::from_raw(0), "Tracked");
        s.init_capacity(4);
        s.grow_to(1);
        write_slot(&mut s, ChunkId::from_raw(0), 0, Tracked(0));
        write_slot(&mut s, ChunkId::from_raw(0), 1, Tracked(1));
        // SAFETY: two slots initialized.
        unsafe { s.set_len(2) };
        // Storage drops here.
    }

    assert_eq!(
        DROPS.load(Ordering::SeqCst),
        2,
        "only initialized slots drop"
    );
}

/// Growing to multiple chunks must not corrupt values in earlier chunks.
#[test]
fn grow_to_preserves_earlier_chunks() {
    let mut s = make_p(2);
    write_slot(&mut s, ChunkId::from_raw(0), 0, P(1.0, 1.0));
    write_slot(&mut s, ChunkId::from_raw(0), 1, P(2.0, 2.0));
    // SAFETY: two slots initialized.
    unsafe { s.set_len(2) };

    s.grow_to(2);
    write_slot(&mut s, ChunkId::from_raw(1), 0, P(3.0, 3.0));
    // SAFETY: three slots initialized across two chunks.
    unsafe { s.set_len(3) };

    assert_eq!(s.chunk_count(), 2);
    assert_eq!(
        s.chunk_slice(ChunkId::from_raw(0)),
        &[P(1.0, 1.0), P(2.0, 2.0)]
    );
    assert_eq!(s.chunk_slice(ChunkId::from_raw(1)), &[P(3.0, 3.0)]);
}

/// `init_capacity` is a one-shot setup: the archetype decides the capacity
/// once, and it must never change afterwards.
#[test]
#[should_panic(expected = "already initialized")]
fn init_capacity_is_one_shot() {
    let mut s = DenseStorage::<P>::new(ComponentId::from_raw(0), "P");
    s.init_capacity(4);
    s.init_capacity(8);
}

/// `grow_to` before `init_capacity` is a logic error and must panic.
#[test]
#[should_panic(expected = "init_capacity must be called")]
fn grow_before_init_panics() {
    let mut s = DenseStorage::<P>::new(ComponentId::from_raw(0), "P");
    s.grow_to(1);
}
