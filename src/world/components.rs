//! Component registry.

use crate::component::{ComponentInfo, ComponentKind};
use crate::storage::DenseStorage;
use crate::{ComponentId, MAX_COMPONENTS};

use super::World;

impl World {
    /// Registers a component type and returns its dense identifier.
    ///
    /// The kind is inferred from `size_of::<T>()`: zero-sized types become
    /// [`ComponentKind::Zst`], everything else [`ComponentKind::Dense`].
    ///
    /// # Panics
    ///
    /// Panics if [`MAX_COMPONENTS`] components have already been registered.
    pub fn register_component<T: Send + Sync + 'static>(
        &mut self,
        name: &'static str,
    ) -> ComponentId {
        assert!(
            self.components.len() < MAX_COMPONENTS,
            "component limit reached: {MAX_COMPONENTS}",
        );

        let id = ComponentId::from_raw(
            u32::try_from(self.components.len())
                .expect("component id overflow"),
        );
        let kind = if core::mem::size_of::<T>() == 0 {
            ComponentKind::Zst
        } else {
            ComponentKind::Dense
        };

        self.components.push(ComponentInfo::of::<T>(id, name, kind));
        self.component_factories
            .push(|info| Box::new(DenseStorage::<T>::new_with_info(info)));

        id
    }

    /// Returns the metadata for the given component.
    ///
    /// # Panics
    ///
    /// Panics if `id` was not produced by [`Self::register_component`].
    #[must_use]
    pub fn component_info(&self, id: ComponentId) -> &ComponentInfo {
        &self.components[id.index()]
    }

    /// Looks up the identifier for a previously registered component type.
    ///
    /// Linear scan over registered components. Not on the hot path: native
    /// code caches the returned identifier after registration.
    #[must_use]
    pub fn component_id<T: 'static>(&self) -> Option<ComponentId> {
        let target = core::any::TypeId::of::<T>();
        self.components
            .iter()
            .find(|c| c.type_id() == target)
            .map(ComponentInfo::id)
    }

    /// Number of registered component types.
    ///
    /// # Panics
    ///
    /// Panics if the count exceeds `u32::MAX`. Unreachable:
    /// [`Self::register_component`] rejects registration past
    /// [`MAX_COMPONENTS`](crate::MAX_COMPONENTS), which is 1024.
    #[must_use]
    pub fn component_count(&self) -> u32 {
        u32::try_from(self.components.len()).expect("component count overflow")
    }
}
