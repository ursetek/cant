//! Tests for `ComponentMask` — the only structure that touches every
//! archetype, so its invariants matter more than they look.

use crate::{ComponentId, ComponentMask, MASK_WORDS, MAX_COMPONENTS};

fn id(raw: u32) -> ComponentId {
    ComponentId::from_raw(raw)
}

/// Two cache lines, `Copy`, and no interior pointers — this is the contract
/// that lets masks sit inside archetypes and be hashed cheaply.
#[test]
fn mask_layout_is_two_cache_lines() {
    assert_eq!(MASK_WORDS, 16);
    assert_eq!(MAX_COMPONENTS, 1024);
    assert_eq!(core::mem::size_of::<ComponentMask>(), 128);
    assert!(!core::mem::needs_drop::<ComponentMask>());
}

/// Bits must be settable, testable, and clearable at every edge: first word,
/// last word, first and last bit.
#[test]
fn insert_contains_remove_at_edges() {
    let mut m = ComponentMask::EMPTY;
    for raw in [0u32, 1, 63, 64, 65, 1022, 1023] {
        assert!(!m.contains(id(raw)), "bit {raw} starts clear");
        m.insert(id(raw));
        assert!(m.contains(id(raw)), "bit {raw} is set");
    }
    assert_eq!(m.count_ones(), 7);

    for raw in [0u32, 64, 1023] {
        m.remove(id(raw));
        assert!(!m.contains(id(raw)), "bit {raw} was cleared");
    }
    assert_eq!(m.count_ones(), 4);
}

/// Empty masks are the fast path of `matches`. This test locks in that
/// `EMPTY.matches(&EMPTY, &EMPTY) == true` and that `is_empty` agrees.
#[test]
fn empty_mask_matches_everything() {
    let empty = ComponentMask::EMPTY;
    assert!(empty.is_empty());
    assert!(empty.matches(&ComponentMask::EMPTY, &ComponentMask::EMPTY));
}

/// `matches` must enforce both sides of the contract: required bits present,
/// excluded bits absent.
#[test]
fn matches_required_and_excluded() {
    let mut have = ComponentMask::EMPTY;
    have.insert(id(1));
    have.insert(id(5));

    let mut req = ComponentMask::EMPTY;
    req.insert(id(1));

    let mut exc = ComponentMask::EMPTY;
    exc.insert(id(9));

    assert!(
        have.matches(&req, &exc),
        "required present, excluded absent"
    );

    exc.insert(id(5));
    assert!(!have.matches(&req, &exc), "excluded bit present in have");

    req.insert(id(7));
    assert!(!have.matches(&req, &exc), "required bit absent in have");
}

/// `is_subset_of` must be reflexive and must reject a single differing bit.
#[test]
fn subset_semantics() {
    let mut big = ComponentMask::EMPTY;
    big.insert(id(1));
    big.insert(id(2));
    big.insert(id(3));

    let mut small = ComponentMask::EMPTY;
    small.insert(id(1));
    small.insert(id(2));

    assert!(small.is_subset_of(&big));
    assert!(!big.is_subset_of(&small));
    assert!(big.is_subset_of(&big), "reflexive");

    small.insert(id(9));
    assert!(!small.is_subset_of(&big), "extra bit breaks subset");
}

/// Union and intersection must be pointwise bit ops over the full mask.
#[test]
fn union_and_intersection() {
    let mut a = ComponentMask::EMPTY;
    a.insert(id(0));
    a.insert(id(100));

    let mut b = ComponentMask::EMPTY;
    b.insert(id(100));
    b.insert(id(1023));

    let u = a.union(&b);
    assert_eq!(u.count_ones(), 3);
    assert!(u.contains(id(0)));
    assert!(u.contains(id(100)));
    assert!(u.contains(id(1023)));

    let i = a.intersection(&b);
    assert_eq!(i.count_ones(), 1);
    assert!(i.contains(id(100)));
}

/// Iteration order is word-major, bit-minor. Callers rely on this to build
/// deterministic component lists for archetypes.
#[test]
fn iter_is_deterministic_and_ascending() {
    let mut m = ComponentMask::EMPTY;
    for raw in [1023u32, 65, 64, 63, 1, 0] {
        m.insert(id(raw));
    }
    let got: Vec<u32> = m.iter().map(ComponentId::raw).collect();
    assert_eq!(got, vec![0, 1, 63, 64, 65, 1023], "ascending order");
}
