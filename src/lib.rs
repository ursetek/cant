//! `cant` — a dynamic archetype-based ECS.
//!
//! # Design principles
//!
//! - **Dense identifiers.** Everything registered at runtime receives a dense
//!   `u32` index. Hot paths use direct `Vec` indexing, never hash maps.
//! - **Struct of arrays.** Components of the same type live in `Vec<T>`,
//!   chunked per archetype. Array-of-structs is never used for component
//!   storage.
//! - **One `dyn` per chunk / system call.** Virtual dispatch happens at the
//!   chunk boundary, not per entity or per component.
//! - **Compile-time when possible, runtime when not.** Native systems are
//!   checked by the compiler; dynamically registered systems are validated
//!   once during registration.
//! - **Zero-copy projection.** External consumers (WASM modules, editors,
//!   debuggers) observe flat `(entities[], comp_a[], comp_b[], len)` slices.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_debug_implementations)]

pub mod archetype;
pub mod bundle;
pub mod commands;
pub mod component;
pub mod entity;
pub mod id;
pub mod mask;
pub mod projection;
pub mod query;
pub mod schedule;
pub mod storage;
pub mod system;
pub mod world;

#[cfg(test)]
mod tests;

pub use archetype::{Archetype, SwapRemoveResult};
pub use bundle::Bundle;
pub use commands::Commands;
pub use component::{ComponentInfo, ComponentKind};
pub use entity::{Entities, Entity, EntityMeta, Location};
pub use id::{
    ArchetypeId, BundleId, ChunkId, ComponentId, ResourceId, SystemId,
};
pub use mask::{ComponentMask, MASK_WORDS, MAX_COMPONENTS};
pub use projection::{
    ChunkSink, ChunkSource, RawChunk, RawChunkMut, RawColumn, RawColumnMut,
};
pub use query::{
    Access, ChunkView, Query, QueryError, QueryMask, UnsafeWorldCell,
};
pub use schedule::Schedule;
pub use system::{AccessList, FnSystem, QuerySystem, System};
pub use world::{ResourceError, World};

// ---------------------------------------------------------------------------
// Component trait (for #[derive(Component)])
// ---------------------------------------------------------------------------

/// Marker trait for types registered as ECS components.
///
/// Implement this via `#[derive(Component)]`. The `NAME` constant is used by
/// [`World::register`] to give the component a stable, human-readable name
/// in diagnostics.
pub trait Component: Send + Sync + 'static {
    /// Stable, human-readable name.
    const NAME: &'static str;
}

impl World {
    /// Registers a component by its derived name.
    ///
    /// # Panics
    ///
    /// Panics if [`MAX_COMPONENTS`] is reached.
    pub fn register<T: Component>(&mut self) -> ComponentId {
        self.register_component::<T>(T::NAME)
    }
}

pub use cant_macros::{Component as ComponentDerive, system};
