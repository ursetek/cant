//! Dense identifier newtypes.
//!
//! Every identifier is a `#[repr(transparent)]` wrapper over a `u32` or a
//! `NonZeroU32`. Dense IDs are the backbone of the ECS: they index `Vec`s
//! directly and never participate in hash lookups on the hot path.

use core::fmt;
use core::num::NonZeroU32;

/// Generates a dense-ID newtype over `u32`.
///
/// The zero value is valid. Use this for identifier kinds that do not need
/// a niche.
macro_rules! dense_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(transparent)]
        pub struct $name(pub(crate) u32);

        impl $name {
            /// Creates an identifier from a raw value.
            ///
            /// `raw` must have been produced by the ECS registry.
            #[inline(always)]
            #[must_use]
            pub const fn from_raw(raw: u32) -> Self {
                Self(raw)
            }

            /// Returns the raw `u32` value.
            #[inline(always)]
            #[must_use]
            pub const fn raw(self) -> u32 {
                self.0
            }

            /// Returns the identifier as a `usize`, for indexing into a `Vec`.
            #[inline(always)]
            #[must_use]
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}

/// Generates a dense-ID newtype backed by `NonZeroU32`.
///
/// Use this when the identifier must have a niche so that types containing
/// `Option<Self>` do not pay for a discriminant. Currently only [`ArchetypeId`]
/// uses it: [`Location`](crate::Location) stores it together with a row and
/// relies on the niche to keep `Option<Location>` at 8 bytes.
macro_rules! dense_id_nonzero {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(transparent)]
        pub struct $name(NonZeroU32);

        impl $name {
            /// Creates an identifier from a raw value.
            ///
            /// Zero is reserved for the niche and is silently normalized to
            /// `1`. The registry never produces raw `0` for this type; a
            /// caller passing `0` is asking for the canonical "invalid"
            /// value, which is represented here as the smallest legal one.
            #[inline(always)]
            #[must_use]
            pub const fn from_raw(raw: u32) -> Self {
                let nz = if raw == 0 { 1 } else { raw };
                // SAFETY: `nz >= 1`, so `NonZeroU32::new_unchecked` is valid.
                Self(unsafe { NonZeroU32::new_unchecked(nz) })
            }

            /// Returns the raw `u32` value. Always non-zero.
            #[inline(always)]
            #[must_use]
            pub const fn raw(self) -> u32 {
                self.0.get()
            }

            /// Returns the identifier as a `usize`, for indexing into a `Vec`.
            #[inline(always)]
            #[must_use]
            pub const fn index(self) -> usize {
                self.0.get() as usize
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0.get())
            }
        }
    };
}

dense_id! {
    /// Dense identifier of a registered component type.
    ComponentId
}

dense_id! {
    /// Dense identifier of a registered resource type.
    ResourceId
}

dense_id! {
    /// Dense identifier of a system in the schedule.
    SystemId
}

dense_id_nonzero! {
    /// Dense identifier of an archetype.
    ///
    /// Backed by `NonZeroU32`, so [`Location`](crate::Location) gets a niche
    /// and `Option<Location>` stays at 8 bytes. The archetype registry
    /// allocates identifiers starting from `1`.
    ArchetypeId
}

dense_id! {
    /// Dense identifier of a chunk inside an archetype.
    ChunkId
}

dense_id! {
    /// Dense identifier of a registered bundle type.
    BundleId
}
