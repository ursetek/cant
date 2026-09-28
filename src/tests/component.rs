//! Tests for runtime component metadata.

use core::alloc::Layout;

use crate::ComponentId;
use crate::component::{ComponentInfo, ComponentKind};

#[derive(Debug)]
struct Position(f32, f32, f32);

#[derive(Debug)]
struct Marker;

#[derive(Debug)]
struct Owned(String);

/// Non-ZST, `Copy`-less components must report the correct size and align,
/// and must not request a drop function.
#[test]
fn dense_component_records_layout() {
    let info = ComponentInfo::of::<Position>(
        ComponentId::from_raw(0),
        "Position",
        ComponentKind::Dense,
    );
    assert_eq!(info.layout().size(), 12);
    assert_eq!(info.layout().align(), 4);
    assert_eq!(info.kind(), ComponentKind::Dense);
    assert!(!info.needs_drop());
    assert_eq!(info.name(), "Position");
}

/// ZSTs must be registered with `ComponentKind::Zst` and must report a
/// zero-sized layout.
#[test]
fn zst_component_records_zero_size() {
    let info = ComponentInfo::of::<Marker>(
        ComponentId::from_raw(1),
        "Marker",
        ComponentKind::Zst,
    );
    assert_eq!(info.layout().size(), 0);
    assert_eq!(info.layout(), Layout::new::<Marker>());
    assert!(!info.needs_drop());
}

/// Mismatching a non-ZST type with `ComponentKind::Zst` must panic on
/// registration, not at runtime. This is a programming error.
#[test]
#[should_panic(expected = "zero-sized")]
fn zst_kind_with_non_zst_panics() {
    let _ = ComponentInfo::of::<Marker>(
        ComponentId::from_raw(2),
        "Marker",
        ComponentKind::Dense,
    );
}

/// A component with a non-trivial destructor must report `needs_drop() == true`.
#[test]
fn needs_drop_is_detected() {
    let info = ComponentInfo::of::<Owned>(
        ComponentId::from_raw(3),
        "Owned",
        ComponentKind::Dense,
    );
    assert!(info.needs_drop());
}

/// `drop_in_place` must run the destructor exactly once.
#[test]
fn drop_in_place_runs_destructor() {
    use core::sync::atomic::{AtomicUsize, Ordering};
    static DROPS: AtomicUsize = AtomicUsize::new(0);

    struct Counted(#[allow(dead_code)] u32);
    impl Drop for Counted {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    let info = ComponentInfo::of::<Counted>(
        ComponentId::from_raw(4),
        "Counted",
        ComponentKind::Dense,
    );
    let mut slot = core::mem::MaybeUninit::<Counted>::uninit();
    slot.write(Counted(0));

    // SAFETY:
    // `slot` was initialized immediately above, and we drop it exactly once
    // here. `MaybeUninit` does not drop its contents on scope exit, so there is
    // no risk of a double drop.
    unsafe { info.drop_in_place(slot.as_mut_ptr().cast::<u8>()) };

    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
}
