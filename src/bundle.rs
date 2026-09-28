//! Typed spawn bundles.

use crate::archetype::Archetype;
use crate::{ComponentId, World};

/// A set of components that can be spawned together.
pub trait Bundle: 'static {
    /// Fills `out` with the bundle's component identifiers in tuple order.
    ///
    /// # Panics
    ///
    /// Implementations panic if a component type is not registered.
    fn collect_ids(world: &World, out: &mut Vec<ComponentId>);

    /// Fills `out` with `(size, align)` per component, in tuple order.
    fn collect_layout(out: &mut Vec<(u32, u32)>);

    /// Writes every component value into the freshly reserved row `row`.
    ///
    /// # Safety
    ///
    /// - `arch` must be the archetype registered for this bundle type.
    /// - `slots[i]` must be the storage-slot index of the i-th tuple
    ///   component in `arch`.
    /// - `row` must be a freshly reserved row: every slot for these
    ///   components is uninitialized.
    unsafe fn write_into(self, arch: &mut Archetype, row: u32, slots: &[usize]);

    /// Writes each component value into `arena` at the given offsets.
    ///
    /// # Safety
    ///
    /// - `offsets.len()` must equal the number of components in the bundle.
    /// - For each `i`, `arena.len() >= offsets[i] + size_of::<Component_i>()`.
    /// - `arena` must not overlap any live value of the component types.
    unsafe fn write_to_arena(self, arena: &mut [u8], offsets: &[u32]);
}

impl Bundle for () {
    fn collect_ids(_world: &World, _out: &mut Vec<ComponentId>) {}
    fn collect_layout(_out: &mut Vec<(u32, u32)>) {}
    unsafe fn write_into(
        self,
        _arch: &mut Archetype,
        _row: u32,
        _slots: &[usize],
    ) {
    }
    unsafe fn write_to_arena(self, _arena: &mut [u8], _offsets: &[u32]) {}
}

macro_rules! impl_bundle_tuple {
    ($($name:ident),+) => {
        impl<$($name: Send + Sync + 'static),+> Bundle for ($($name,)+) {
            fn collect_ids(world: &World, out: &mut Vec<ComponentId>) {
                $(
                    let id = world
                        .component_id::<$name>()
                        .unwrap_or_else(|| panic!(
                            "component `{}` is not registered",
                            stringify!($name),
                        ));
                    out.push(id);
                )+
            }

            #[allow(clippy::cast_possible_truncation)]
            fn collect_layout(out: &mut Vec<(u32, u32)>) {
                $(
                    out.push((
                        core::mem::size_of::<$name>() as u32,
                        core::mem::align_of::<$name>() as u32,
                    ));
                )+
            }

            unsafe fn write_into(
                self,
                arch: &mut Archetype,
                row: u32,
                slots: &[usize],
            ) {
                #[allow(non_snake_case)]
                let ($($name,)+) = self;
                let mut __slots = slots.iter();
                $(
                    let __slot = *__slots.next().expect("slots shorter than bundle");
                    let value = core::mem::ManuallyDrop::new($name);
                    let ptr = core::ptr::addr_of!(*value).cast::<u8>();
                    // SAFETY: caller guarantees row is uninitialized.
                    unsafe { arch.write_component_at_slot(__slot, row, ptr) };
                )+
            }

            unsafe fn write_to_arena(self, arena: &mut [u8], offsets: &[u32]) {
                #[allow(non_snake_case)]
                let ($($name,)+) = self;
                let mut __i = 0usize;
                $(
                    let __val = core::mem::ManuallyDrop::new($name);
                    let __size = core::mem::size_of::<$name>();
                    let __off = offsets[__i] as usize;
                    let __src = core::ptr::addr_of!(*__val).cast::<u8>();
                    // SAFETY: caller guarantees `__off + __size <= arena.len()`.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            __src,
                            arena.as_mut_ptr().add(__off),
                            __size,
                        );
                    }
                    __i += 1;
                )+
            }
        }
    };
}

impl_bundle_tuple!(A);
impl_bundle_tuple!(A, B);
impl_bundle_tuple!(A, B, C);
impl_bundle_tuple!(A, B, C, D);
impl_bundle_tuple!(A, B, C, D, E);
impl_bundle_tuple!(A, B, C, D, E, F);
impl_bundle_tuple!(A, B, C, D, E, F, G);
impl_bundle_tuple!(A, B, C, D, E, F, G, H);
