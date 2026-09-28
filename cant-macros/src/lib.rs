//! Procedural macros for the `cant` ECS.
//!
//! Exposes two macros:
//!
//! - [`macro@Component`] — derives [`cant::Component`], providing a stable
//!   `NAME` constant.
//! - [`macro@system`] — turns a plain Rust function into a [`cant::System`]
//!   implementation, wiring queries, resources, local state, and command
//!   buffers from attributes on the function.
//!
//! See [`system`] for the full grammar.

use proc_macro::TokenStream;

mod component;
mod naming;
mod system;

/// Derives `cant::Component`, providing a stable `NAME` constant.
///
/// The derived name is the Rust type name as written, with any path prefix
/// stripped. For example, `math::Position` derives `NAME = "Position"`.
#[proc_macro_derive(Component)]
pub fn derive_component(input: TokenStream) -> TokenStream {
    component::derive(input.into()).into()
}

/// Declares a system from a plain Rust function.
///
/// Accepts two equivalent forms:
///
/// **Combined:**
///
/// ```ignore
/// #[system(
///     hint(8000),
///     resources(dt: f32, counter: mut u32),
///     locals(frame: u64 = 0),
///     commands(cmd),
/// )]
/// fn lifetime(entities: &[Entity], lifetimes: &mut [Lifetime]) { ... }
/// ```
///
/// **Separate attributes:**
///
/// ```ignore
/// #[system]
/// #[hint(8000)]
/// #[resources(dt: f32, counter: mut u32)]
/// #[locals(frame: u64 = 0)]
/// #[commands(cmd)]
/// fn lifetime(entities: &[Entity], lifetimes: &mut [Lifetime]) { ... }
/// ```
///
/// In the separate form, `#[system]` must be the first attribute.
/// Mixing forms is an error.
///
/// # Function parameters
///
/// Only three parameter shapes are recognized:
///
/// - `&[Entity]` — the entity slice of the current chunk (at most once).
/// - `&[T]` — read access to component `T`.
/// - `&mut [T]` — write access to component `T`.
///
/// Everything else (resources, locals, commands) is declared through
/// attributes.
#[proc_macro_attribute]
pub fn system(attr: TokenStream, item: TokenStream) -> TokenStream {
    system::expand(attr.into(), item.into()).into()
}
