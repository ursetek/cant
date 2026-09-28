//! Runtime metadata for registered component types.

use core::alloc::Layout;
use core::any::TypeId;
use core::fmt;

use crate::ComponentId;

/// Strategy used to store a component type inside an archetype.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    /// Dense SoA storage, one array per chunk, indexed by row.
    Dense,
    /// Sparse-set storage for rare components.
    Sparse,
    /// Zero-sized type: no data, tracked as a bit in the archetype mask.
    Zst,
    /// Type-erased opaque blob with known layout but unknown semantics.
    /// Used for components registered from external runtimes.
    Opaque,
}

/// Type-erased metadata for a registered component type.
///
/// Holds everything the ECS needs to manipulate a component without knowing
/// its Rust type at compile time: size, alignment, kind, and (if needed) a
/// drop function.
#[derive(Clone)]
pub struct ComponentInfo {
    id: ComponentId,
    name: &'static str,
    type_id: TypeId,
    layout: Layout,
    kind: ComponentKind,
    drop_fn: Option<unsafe fn(*mut u8)>,
}

impl ComponentInfo {
    /// Builds metadata for a concrete Rust type `T`.
    ///
    /// # Panics
    ///
    /// Panics if `kind == ComponentKind::Zst` disagrees with
    /// `size_of::<T>() == 0`.
    #[must_use]
    pub fn of<T: 'static>(
        id: ComponentId,
        name: &'static str,
        kind: ComponentKind,
    ) -> Self {
        let layout = Layout::new::<T>();
        let is_zst = layout.size() == 0;
        assert_eq!(
            kind == ComponentKind::Zst,
            is_zst,
            "ComponentKind::Zst must be used iff T is zero-sized",
        );
        Self {
            id,
            name,
            type_id: TypeId::of::<T>(),
            layout,
            kind,
            drop_fn: if core::mem::needs_drop::<T>() {
                Some(drop_impl::<T>)
            } else {
                None
            },
        }
    }

    /// Builds metadata for an opaque component registered at runtime.
    ///
    /// # Panics
    ///
    /// Panics if `layout.size() == 0`. Zero-sized components must be
    /// registered through [`ComponentKind::Zst`] instead: they carry no
    /// payload, so a byte-level storage would be meaningless.
    ///
    /// # Safety
    ///
    /// The caller must guarantee:
    ///
    /// - `layout.size() > 0` — zero-sized types must be registered through
    ///   [`ComponentKind::Zst`] instead.
    /// - `drop_fn`, if `Some`, is a valid drop function for a value of the
    ///   given layout and is safe to call exactly once on a properly aligned,
    ///   initialized pointer.
    #[must_use]
    pub unsafe fn of_opaque(
        id: ComponentId,
        name: &'static str,
        layout: Layout,
        drop_fn: Option<unsafe fn(*mut u8)>,
    ) -> Self {
        assert!(layout.size() > 0, "opaque component must not be zero-sized");
        Self {
            id,
            name,
            type_id: TypeId::of::<()>(),
            layout,
            kind: ComponentKind::Opaque,
            drop_fn,
        }
    }

    /// Returns the dense identifier.
    #[inline]
    #[must_use]
    pub fn id(&self) -> ComponentId {
        self.id
    }

    /// Returns the human-readable name (used for logs and debugging).
    #[inline]
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the Rust [`TypeId`], or `TypeId::of::<()>()` for opaque
    /// components.
    #[inline]
    #[must_use]
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// Returns the size and alignment of the component.
    #[inline]
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Returns the storage strategy.
    #[inline]
    #[must_use]
    pub fn kind(&self) -> ComponentKind {
        self.kind
    }

    /// Returns `true` if the component has a non-trivial destructor.
    #[inline]
    #[must_use]
    pub fn needs_drop(&self) -> bool {
        self.drop_fn.is_some()
    }

    /// Drops the value pointed to by `ptr`.
    ///
    /// # Safety
    ///
    /// - `ptr` must be properly aligned and point to an initialized value of
    ///   this component type.
    /// - The value must not be used afterwards.
    /// - The value must not be dropped twice.
    #[inline]
    pub unsafe fn drop_in_place(&self, ptr: *mut u8) {
        if let Some(f) = self.drop_fn {
            // SAFETY: contract of `drop_in_place` mirrors `drop_fn`.
            unsafe { f(ptr) };
        }
    }
}

impl fmt::Debug for ComponentInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentInfo")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("layout", &self.layout)
            .field("kind", &self.kind)
            .field("needs_drop", &self.needs_drop())
            .finish_non_exhaustive()
    }
}

/// Monomorphized drop trampoline.
///
/// # Safety
///
/// `ptr` must point to a properly aligned, initialized `T`.
unsafe fn drop_impl<T>(ptr: *mut u8) {
    // SAFETY: caller of `drop_impl` guarantees `ptr` is valid.
    unsafe { ptr.cast::<T>().drop_in_place() };
}
