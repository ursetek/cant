//! Systems: units of work scheduled against the world.
//!
//! A [`System`] declares the components and resources it accesses, and how
//! (read or write). The [`Schedule`](crate::schedule::Schedule) uses these
//! declarations to order systems and, later, to run non-conflicting systems
//! in parallel.

use core::fmt;

use crate::query::{Access, ChunkView, Query, UnsafeWorldCell};
use crate::{Commands, ComponentId, ResourceId};

/// Declares the components and resources a system accesses.
///
/// `structural_write` is a coarse opt-in flag for systems that spawn or
/// otherwise mutate the entity table through [`Commands`](crate::Commands).
/// It conflicts with every other access — including itself — so such systems
/// always run alone in their batch.
#[derive(Clone, Debug, Default)]
pub struct AccessList {
    components: Vec<(ComponentId, Access)>,
    resources: Vec<(ResourceId, Access)>,
    structural_write: bool,
    cost_hint_ns: u32,
}

impl AccessList {
    /// Creates an empty access list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the estimated cost of this system, in nanoseconds.
    ///
    /// Zero (the default) means "no hint"; the scheduler then falls back to
    /// the batch-size threshold. A non-zero hint participates in the
    /// cost-based decision.
    pub fn set_cost_hint_ns(&mut self, ns: u32) -> &mut Self {
        self.cost_hint_ns = ns;
        self
    }

    /// Returns the cost hint, in nanoseconds.
    #[must_use]
    pub fn cost_hint_ns(&self) -> u32 {
        self.cost_hint_ns
    }

    /// Declares access to component `id`.
    ///
    /// If `id` was already declared with [`Access::Read`] and `acc` is
    /// [`Access::Write`], the access is upgraded.
    pub fn add_component(&mut self, id: ComponentId, acc: Access) -> &mut Self {
        if let Some(slot) = self.components.iter_mut().find(|(c, _)| *c == id) {
            if acc == Access::Write {
                slot.1 = Access::Write;
            }
        } else {
            self.components.push((id, acc));
        }
        self
    }

    /// Declares access to resource `id`. Same upgrade semantics as
    /// [`Self::add_component`].
    pub fn add_resource(&mut self, id: ResourceId, acc: Access) -> &mut Self {
        if let Some(slot) = self.resources.iter_mut().find(|(r, _)| *r == id) {
            if acc == Access::Write {
                slot.1 = Access::Write;
            }
        } else {
            self.resources.push((id, acc));
        }
        self
    }

    /// Marks this system as a structural writer.
    ///
    /// Mandatory for any system that calls
    /// [`Commands::spawn`](crate::Commands::spawn) or
    /// [`Commands::spawn_bundle`](crate::Commands::spawn_bundle). Omitting it
    /// while spawning from a parallel batch is caught by a runtime assert in
    /// `Commands`.
    pub fn mark_structural_write(&mut self) -> &mut Self {
        self.structural_write = true;
        self
    }

    /// Returns `true` if this system declared a structural write.
    #[must_use]
    pub fn is_structural_writer(&self) -> bool {
        self.structural_write
    }

    /// Builds an access list from a prepared query's component accesses.
    #[must_use]
    pub fn from_query(query: &Query) -> Self {
        Self {
            components: query.mask().access().to_vec(),
            resources: Vec::new(),
            structural_write: false,
            ..Default::default()
        }
    }

    /// Component accesses, in insertion order.
    #[must_use]
    pub fn components(&self) -> &[(ComponentId, Access)] {
        &self.components
    }

    /// Resource accesses, in insertion order.
    #[must_use]
    pub fn resources(&self) -> &[(ResourceId, Access)] {
        &self.resources
    }

    /// Returns `true` if the two access lists cannot run concurrently.
    ///
    /// A structural writer conflicts with everything, including another
    /// structural writer. Otherwise, two reads never conflict; any write
    /// conflicts with a read or write of the same component or resource.
    #[must_use]
    pub fn conflicts_with(&self, other: &Self) -> bool {
        if self.structural_write || other.structural_write {
            return true;
        }
        for &(c, a) in &self.components {
            for &(oc, oa) in &other.components {
                if c == oc && (a == Access::Write || oa == Access::Write) {
                    return true;
                }
            }
        }
        for &(r, a) in &self.resources {
            for &(or_, oa) in &other.resources {
                if r == or_ && (a == Access::Write || oa == Access::Write) {
                    return true;
                }
            }
        }
        false
    }
}

/// A unit of work that runs against the world.
pub trait System: Send + Sync + 'static {
    /// Human-readable name, used for diagnostics.
    fn name(&self) -> &str;

    /// Access list declared by this system.
    fn access(&self) -> &AccessList;

    /// Runs the system.
    ///
    /// The scheduler guarantees no other system with a conflicting access
    /// list runs concurrently with this call. Deferred structural changes
    /// go through `commands`; they are applied at the end of the stage.
    fn run(&mut self, world: UnsafeWorldCell<'_>, commands: &mut Commands);
}

impl fmt::Debug for dyn System {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("System")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

/// A [`System`] backed by a closure.
pub struct FnSystem<F> {
    name: String,
    access: AccessList,
    f: F,
}

impl<F> fmt::Debug for FnSystem<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FnSystem")
            .field("name", &self.name)
            .field("access", &self.access)
            .finish_non_exhaustive()
    }
}

impl<F> FnSystem<F>
where
    F: FnMut(UnsafeWorldCell<'_>, &mut Commands) + Send + Sync + 'static,
{
    /// Creates a closure-backed system.
    ///
    /// The closure receives the shared world cell and the deferred command
    /// buffer. It must not retain any reference derived from either beyond
    /// the call.
    pub fn new(name: impl Into<String>, access: AccessList, f: F) -> Self {
        Self {
            name: name.into(),
            access,
            f,
        }
    }
}

impl<F> System for FnSystem<F>
where
    F: FnMut(UnsafeWorldCell<'_>, &mut Commands) + Send + Sync + 'static,
{
    fn name(&self) -> &str {
        &self.name
    }
    fn access(&self) -> &AccessList {
        &self.access
    }
    fn run(&mut self, world: UnsafeWorldCell<'_>, commands: &mut Commands) {
        (self.f)(world, commands);
    }
}

/// A [`System`] that iterates a prepared [`Query`].
///
/// The system's [`AccessList`] is derived from the query's mask. The closure
/// receives a [`ChunkView`] per chunk and must not retain it beyond the call.
pub struct QuerySystem<F> {
    name: String,
    query: Query,
    access: AccessList,
    f: F,
}

impl<F> fmt::Debug for QuerySystem<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QuerySystem")
            .field("name", &self.name)
            .field("query", &self.query)
            .field("access", &self.access)
            .finish_non_exhaustive()
    }
}

impl<F> QuerySystem<F>
where
    F: FnMut(ChunkView<'_>) + Send + Sync + 'static,
{
    /// Creates a query-driven system.
    pub fn new(name: impl Into<String>, query: Query, f: F) -> Self {
        let access = AccessList::from_query(&query);
        Self {
            name: name.into(),
            query,
            access,
            f,
        }
    }

    /// Returns the query this system iterates.
    #[must_use]
    pub fn query(&self) -> &Query {
        &self.query
    }
}

impl<F> QuerySystem<F>
where
    F: FnMut(ChunkView<'_>) + Send + Sync + 'static,
{
    /// Sets the system's cost hint in nanoseconds; see
    /// [`AccessList::set_cost_hint_ns`].
    #[must_use]
    pub fn with_cost_hint_ns(mut self, ns: u32) -> Self {
        self.access.set_cost_hint_ns(ns);
        self
    }
}

impl<F> System for QuerySystem<F>
where
    F: FnMut(ChunkView<'_>) + Send + Sync + 'static,
{
    fn name(&self) -> &str {
        &self.name
    }
    fn access(&self) -> &AccessList {
        &self.access
    }
    fn run(&mut self, world: UnsafeWorldCell<'_>, _commands: &mut Commands) {
        let f = &mut self.f;
        // SAFETY: the scheduler guarantees no conflicting access runs
        // concurrently; the closure does not retain views beyond each call.
        unsafe {
            self.query.for_each_cell(world, f);
        }
    }
}
