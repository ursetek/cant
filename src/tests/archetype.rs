//! Tests for archetype bookkeeping: chunked growth, component storage alignment,
//! row swap-removal, and drop semantics.

use crate::archetype::{Archetype, DEFAULT_CHUNK_BYTES};
use crate::storage::{DenseStorage, Storage};
use crate::{ArchetypeId, ComponentId, ComponentMask, Entity};

/// Component used to exercise arithmetic and read-back.
#[derive(Debug, PartialEq, Clone, Copy)]
struct P(f32, f32);

/// Second component, distinct type so we can check parallel storages.
#[derive(Debug, PartialEq, Clone, Copy)]
struct V(f32, f32);

const POS: ComponentId = ComponentId::from_raw(0);
const VEL: ComponentId = ComponentId::from_raw(1);

fn entity_at(index: u32) -> Entity {
    Entity::from_parts(index, 1)
}

/// Builds a `(Position, Velocity)` archetype for two 8-byte components.
fn make_pv() -> Archetype {
    let mut mask = ComponentMask::EMPTY;
    mask.insert(POS);
    mask.insert(VEL);

    let components = vec![POS, VEL].into_boxed_slice();
    let storages: Vec<Box<dyn Storage>> = vec![
        Box::new(DenseStorage::<P>::new(POS, "P")),
        Box::new(DenseStorage::<V>::new(VEL, "V")),
    ];

    Archetype::new(ArchetypeId::from_raw(0), mask, components, storages)
}

/// Pushes a `(P, V)` pair into a fresh row.
fn push_pv(arch: &mut Archetype, entity: Entity, p: P, v: V) -> u32 {
    arch.push_with(entity, |arch, row| {
        let p = core::mem::ManuallyDrop::new(p);
        let v = core::mem::ManuallyDrop::new(v);
        // SAFETY: the row is freshly reserved, slots are uninitialized, and
        // `p`/`v` point to valid, initialized values.
        unsafe {
            arch.write_raw(POS, row, core::ptr::addr_of!(*p).cast::<u8>());
            arch.write_raw(VEL, row, core::ptr::addr_of!(*v).cast::<u8>());
        }
    })
}

/// Reads component `P` at `row` through the typed dense storage.
fn read_p(arch: &Archetype, row: u32) -> P {
    let storage = arch.storage(POS).expect("POS storage present");
    let dense = storage
        .as_any()
        .downcast_ref::<DenseStorage<P>>()
        .expect("dense storage type matches");
    let chunk = arch.chunk_of(row);
    let r = arch.row_in_chunk(row);
    dense.chunk_slice(chunk)[r as usize]
}

/// Reads component `V` at `row`.
fn read_v(arch: &Archetype, row: u32) -> V {
    let storage = arch.storage(VEL).expect("VEL storage present");
    let dense = storage
        .as_any()
        .downcast_ref::<DenseStorage<V>>()
        .expect("dense storage type matches");
    let chunk = arch.chunk_of(row);
    let r = arch.row_in_chunk(row);
    dense.chunk_slice(chunk)[r as usize]
}

/// A freshly constructed archetype holds no entities and reports zero chunks.
#[test]
fn new_archetype_is_empty() {
    let arch = make_pv();
    assert_eq!(arch.len(), 0);
    assert!(arch.is_empty());
    assert_eq!(arch.chunk_count(), 0);
    assert!(arch.has_component(POS));
    assert!(arch.has_component(VEL));
    assert!(!arch.has_component(ComponentId::from_raw(999)));
}

/// Pushing an entity grows both storages in lockstep and reports the row.
#[test]
fn push_grows_storages_in_lockstep() {
    let mut arch = make_pv();
    let row = push_pv(&mut arch, entity_at(0), P(1.0, 2.0), V(3.0, 4.0));

    assert_eq!(row, 0);
    assert_eq!(arch.len(), 1);
    assert_eq!(arch.chunk_count(), 1);
    assert_eq!(read_p(&arch, 0), P(1.0, 2.0));
    assert_eq!(read_v(&arch, 0), V(3.0, 4.0));
}

/// Swap-removing the tail row destroys exactly that row's components and
/// decrements the length.
#[test]
fn swap_remove_tail_row() {
    let mut arch = make_pv();
    push_pv(&mut arch, entity_at(0), P(1.0, 0.0), V(0.0, 0.0));
    push_pv(&mut arch, entity_at(1), P(2.0, 0.0), V(0.0, 0.0));
    push_pv(&mut arch, entity_at(2), P(3.0, 0.0), V(0.0, 0.0));

    let result = arch.swap_remove(2);
    assert_eq!(result.removed, entity_at(2));
    assert!(
        result.moved.is_none(),
        "tail removal does not move anything"
    );
    assert_eq!(arch.len(), 2);

    // The two earlier rows are untouched.
    assert_eq!(read_p(&arch, 0), P(1.0, 0.0));
    assert_eq!(read_p(&arch, 1), P(2.0, 0.0));
}

/// Swap-removing a middle row moves the tail row into its place. The returned
/// `moved` field names the relocated entity, so the caller can fix up its
/// location.
#[test]
fn swap_remove_middle_row_moves_tail() {
    let mut arch = make_pv();
    push_pv(&mut arch, entity_at(0), P(10.0, 0.0), V(0.0, 0.0));
    push_pv(&mut arch, entity_at(1), P(20.0, 0.0), V(0.0, 0.0));
    push_pv(&mut arch, entity_at(2), P(30.0, 0.0), V(0.0, 0.0));

    let result = arch.swap_remove(1);
    assert_eq!(result.removed, entity_at(1));
    assert_eq!(result.moved, Some(entity_at(2)));
    assert_eq!(arch.len(), 2);

    // Row 0 unchanged, row 1 now holds the former tail row.
    assert_eq!(read_p(&arch, 0), P(10.0, 0.0));
    assert_eq!(read_p(&arch, 1), P(30.0, 0.0));
}

/// Chunk capacity is the minimum across non-ZST components: for two 8-byte
/// components it is `DEFAULT_CHUNK_BYTES / 8`.
#[test]
fn chunk_capacity_is_min_across_components() {
    let arch = make_pv();
    #[allow(clippy::cast_possible_truncation)]
    // ^--- Intended truncation.
    let expected = (DEFAULT_CHUNK_BYTES / core::mem::size_of::<P>()) as u32;
    assert_eq!(arch.chunk_capacity(), expected);
    assert_eq!(arch.chunk_capacity(), 2048);
}

/// Pushing past the first chunk boundary grows a second chunk, and rows resolve
/// to the correct chunk and row-in-chunk.
#[test]
// `cap` is `chunk_capacity` for an 8-byte component: 16384 / 8 = 2048, well
// within `f32`'s exact-integer range (< 2^23). The cast is lossless here.
#[allow(clippy::cast_precision_loss)]
fn crosses_chunk_boundary() {
    let mut arch = make_pv();
    let cap = arch.chunk_capacity();
    let last_of_first = cap - 1;
    for i in 0..=last_of_first {
        push_pv(&mut arch, entity_at(i), P(i as f32, 0.0), V(0.0, 0.0));
    }
    assert_eq!(arch.chunk_count(), 1);

    // ^--- Intended truncation.
    let crossing =
        push_pv(&mut arch, entity_at(cap), P(cap as f32, 0.0), V(0.0, 0.0));
    assert_eq!(crossing, cap);
    assert_eq!(arch.chunk_count(), 2);

    assert_eq!(arch.chunk_of(cap), crate::ChunkId::from_raw(1));
    assert_eq!(arch.row_in_chunk(cap), 0);
    assert_eq!(arch.chunk_of(last_of_first), crate::ChunkId::from_raw(0));
    assert_eq!(arch.row_in_chunk(last_of_first), last_of_first);
}

/// Dropping the archetype destroys all live components, and only those.
#[test]
fn archetype_drop_runs_component_destructors() {
    use core::sync::atomic::{AtomicUsize, Ordering};
    static DROPS: AtomicUsize = AtomicUsize::new(0);

    struct Tracked(#[allow(dead_code)] u32);
    impl Drop for Tracked {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    let comp = ComponentId::from_raw(5);
    let mut mask = ComponentMask::EMPTY;
    mask.insert(comp);
    let components = vec![comp].into_boxed_slice();
    let storages: Vec<Box<dyn Storage>> =
        vec![Box::new(DenseStorage::<Tracked>::new(comp, "Tracked"))];

    DROPS.store(0, Ordering::SeqCst);

    {
        let mut arch = Archetype::new(
            ArchetypeId::from_raw(1),
            mask,
            components,
            storages,
        );
        for i in 0..3u32 {
            arch.push_with(entity_at(i), |arch, row| {
                let v = core::mem::ManuallyDrop::new(Tracked(i));
                // SAFETY:
                // row is freshly reserved, slot is uninitialized, `v` is a
                // valid initialized `Tracked`.
                unsafe {
                    arch.write_raw(
                        comp,
                        row,
                        core::ptr::addr_of!(*v).cast::<u8>(),
                    );
                }
            });
        }
        // Two rows are live; the archetype drops at the end of this block.
    }

    assert_eq!(DROPS.load(Ordering::SeqCst), 3, "all three rows dropped");
}
