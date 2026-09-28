//! Type-erased resource registry.

use core::any::TypeId;

use crate::ResourceId;

use super::World;

/// Error returned when a resource lookup fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceError {
    /// No resource is registered under this identifier.
    NotRegistered,
    /// A resource is registered but its type does not match the request.
    TypeMismatch,
}

impl core::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotRegistered => f.write_str("resource not registered"),
            Self::TypeMismatch => f.write_str("resource type mismatch"),
        }
    }
}

impl std::error::Error for ResourceError {}

impl World {
    /// Registers a resource and returns its identifier.
    ///
    /// If a resource of the same type already exists, it is replaced and the
    /// identifier stays the same.
    ///
    /// # Panics
    ///
    /// Panics if the resource count exceeds `u32::MAX`. Unreachable in
    /// practice: `ResourceId` is `u32`-backed and each registration adds at
    /// most one slot.
    pub fn register_resource<T: Send + Sync + 'static>(
        &mut self,
        value: T,
    ) -> ResourceId {
        let tid = TypeId::of::<T>();
        if let Some(&id) = self.resource_index.get(&tid) {
            self.resources[id.index()] = Box::new(value);
            return id;
        }

        let id = ResourceId::from_raw(
            u32::try_from(self.resources.len()).expect("resource id overflow"),
        );
        self.resources.push(Box::new(value));
        self.resource_index.insert(tid, id);
        id
    }

    /// Returns the identifier of a registered resource type, if any.
    #[must_use]
    pub fn resource_id<T: 'static>(&self) -> Option<ResourceId> {
        self.resource_index.get(&TypeId::of::<T>()).copied()
    }

    /// Returns a shared reference to the resource.
    ///
    /// # Errors
    ///
    /// Returns [`ResourceError::NotRegistered`] if the resource type was not
    /// registered, or [`ResourceError::TypeMismatch`] if the identifier
    /// points to a different type.
    pub fn resource<T: 'static>(
        &self,
        id: ResourceId,
    ) -> Result<&T, ResourceError> {
        let slot = self
            .resources
            .get(id.index())
            .ok_or(ResourceError::NotRegistered)?;
        slot.downcast_ref::<T>().ok_or(ResourceError::TypeMismatch)
    }

    /// Returns a mutable reference to the resource.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::resource`].
    pub fn resource_mut<T: 'static>(
        &mut self,
        id: ResourceId,
    ) -> Result<&mut T, ResourceError> {
        let slot = self
            .resources
            .get_mut(id.index())
            .ok_or(ResourceError::NotRegistered)?;
        slot.downcast_mut::<T>().ok_or(ResourceError::TypeMismatch)
    }

    /// Number of registered resources.
    ///
    /// # Panics
    ///
    /// Panics if the count exceeds `u32::MAX`. Unreachable: registration
    /// adds one slot at a time and `ResourceId` is `u32`-backed.
    #[must_use]
    pub fn resource_count(&self) -> u32 {
        u32::try_from(self.resources.len()).expect("resource count overflow")
    }
}
