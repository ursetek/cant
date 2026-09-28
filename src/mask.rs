//! Component set bitmask.
//!
//! A [`ComponentMask`] is a fixed-size bitset describing which component
//! types an entity (or archetype) carries. It is the primary key of archetype
//! identity and the matcher used by queries.

use core::fmt;
use core::hash::{Hash, Hasher};

use crate::ComponentId;

/// Maximum number of components distinguishable by the mask.
pub const MAX_COMPONENTS: usize = 1024;

/// Number of 64-bit words in the mask.
pub const MASK_WORDS: usize = MAX_COMPONENTS / 64; // 16

/// Bitset describing a set of component types.
///
/// `Copy`, `Eq`, `Hash`. The mask is 128 bytes — two cache lines. It is never
/// touched inside a hot iteration loop: only during archetype identity and
/// query matching.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct ComponentMask {
    words: [u64; MASK_WORDS],
}

impl ComponentMask {
    /// The empty mask.
    pub const EMPTY: Self = Self {
        words: [0; MASK_WORDS],
    };

    /// Returns `true` if no bits are set.
    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        let mut i = 0;
        while i < MASK_WORDS {
            if self.words[i] != 0 {
                return false;
            }
            i += 1;
        }
        true
    }

    /// Sets the bit for `id`.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `id` is outside `[0, MAX_COMPONENTS)`.
    #[inline]
    pub fn insert(&mut self, id: ComponentId) {
        let raw = id.raw() as usize;
        debug_assert!(raw < MAX_COMPONENTS, "ComponentId out of mask bounds");
        self.words[raw / 64] |= 1u64 << (raw % 64);
    }

    /// Clears the bit for `id`.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `id` is outside `[0, MAX_COMPONENTS)`.
    #[inline]
    pub fn remove(&mut self, id: ComponentId) {
        let raw = id.raw() as usize;
        debug_assert!(raw < MAX_COMPONENTS, "ComponentId out of mask bounds");
        self.words[raw / 64] &= !(1u64 << (raw % 64));
    }

    /// Returns `true` if the bit for `id` is set.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `id` is outside `[0, MAX_COMPONENTS)`.
    #[inline]
    #[must_use]
    pub fn contains(&self, id: ComponentId) -> bool {
        let raw = id.raw() as usize;
        debug_assert!(raw < MAX_COMPONENTS, "ComponentId out of mask bounds");
        (self.words[raw / 64] >> (raw % 64)) & 1 == 1
    }

    /// Returns `true` if `self` contains every bit of `required` and no bit
    /// of `excluded`.
    #[inline]
    #[must_use]
    pub fn matches(&self, required: &Self, excluded: &Self) -> bool {
        let has_req = !required.is_empty();
        let has_exc = !excluded.is_empty();

        if !has_req && !has_exc {
            return true;
        }

        for i in 0..MASK_WORDS {
            let w = self.words[i];
            if has_req && (w & required.words[i]) != required.words[i] {
                return false;
            }
            if has_exc && (w & excluded.words[i]) != 0 {
                return false;
            }
        }
        true
    }

    /// Returns `true` if `self` is a subset of `other` (`self & !other == 0`).
    #[inline]
    #[must_use]
    pub fn is_subset_of(&self, other: &Self) -> bool {
        for i in 0..MASK_WORDS {
            if self.words[i] & !other.words[i] != 0 {
                return false;
            }
        }
        true
    }

    /// Returns the union of `self` and `other`.
    #[inline]
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        let mut out = *self;
        for i in 0..MASK_WORDS {
            out.words[i] |= other.words[i];
        }
        out
    }

    /// Returns the intersection of `self` and `other`.
    #[inline]
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        let mut out = *self;
        for i in 0..MASK_WORDS {
            out.words[i] &= other.words[i];
        }
        out
    }

    /// Returns the number of set bits.
    #[inline]
    #[must_use]
    pub fn count_ones(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum()
    }

    /// Iterates over the [`ComponentId`]s whose bits are set in the mask.
    pub fn iter(&self) -> impl Iterator<Item = ComponentId> + '_ {
        self.words.iter().enumerate().flat_map(|(wi, &w)| {
            let mut w = w;
            core::iter::from_fn(move || {
                if w == 0 {
                    return None;
                }
                let bit = w.trailing_zeros();
                w &= w - 1;
                // `MASK_WORDS == 16`, so `wi * 64 + bit < 1024`, fits in u32.
                #[allow(clippy::cast_possible_truncation)]
                Some(ComponentId::from_raw((wi as u32) * 64 + bit))
            })
        })
    }
}

/// Direct, non-`SipHash` hashing suitable for use with `FxHashMap`.
impl Hash for ComponentMask {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        for w in &self.words {
            state.write_u64(*w);
        }
    }
}

impl fmt::Debug for ComponentMask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Mask{")?;
        let mut first = true;
        for id in self.iter() {
            if !first {
                f.write_str(",")?;
            }
            write!(f, "{}", id.raw())?;
            first = false;
        }
        f.write_str("}")
    }
}
