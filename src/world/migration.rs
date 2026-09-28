//! Structural changes: adding and removing components, moving entities
//! between archetypes.

use crate::{ArchetypeId, ComponentId, Entity, Location};

use super::{World, archetype_index};

impl World {
    /// Adds or replaces a component on an entity.
    ///
    /// Returns `false` if `T` is not registered or the entity is dead.
    pub fn add_component<T: Send + Sync + 'static>(
        &mut self,
        entity: Entity,
        value: T,
    ) -> bool {
        let Some(comp) = self.component_id::<T>() else {
            return false;
        };
        let data = core::ptr::addr_of!(value).cast::<u8>();
        // SAFETY: `data` points to a valid, initialized `T`. Ownership of
        // `value` transfers to the archetype if `ok`.
        let ok = unsafe { self.add_component_raw(entity, comp, data) };
        if ok {
            core::mem::forget(value);
        }
        // else: `value` drops normally.
        ok
    }

    /// Adds or replaces a component on an entity from untyped bytes.
    ///
    /// # Safety
    ///
    /// - `data` must point to a valid, initialized value of the component
    ///   type registered under `comp`.
    /// - The caller transfers ownership of the value to the archetype; the
    ///   bytes at `data` must not be used or dropped afterward if the method
    ///   returns `true`.
    pub(crate) unsafe fn add_component_raw(
        &mut self,
        entity: Entity,
        comp: ComponentId,
        data: *const u8,
    ) -> bool {
        let Some(loc) = self.entities.location(entity) else {
            return false;
        };

        let arch = &mut self.archetypes[archetype_index(loc.archetype)];
        if arch.has_component(comp) {
            // SAFETY: component exists at `(comp, loc.row)`; `data` is valid.
            unsafe { arch.replace_component(comp, loc.row, data) };
            return true;
        }

        let target = self.add_edge(loc.archetype, comp);
        // SAFETY: `comp` is absent from the source, present in the target;
        // `data` transfers ownership of the value.
        unsafe { self.move_entity(entity, loc, target, Some((comp, data))) };
        true
    }

    /// Removes a component from an entity.
    ///
    /// Returns `false` if the entity is dead or does not carry `comp`.
    pub fn remove_component(
        &mut self,
        entity: Entity,
        comp: ComponentId,
    ) -> bool {
        let Some(loc) = self.entities.location(entity) else {
            return false;
        };
        if !self.archetypes[archetype_index(loc.archetype)].has_component(comp)
        {
            return false;
        }

        let target = self.remove_edge(loc.archetype, comp);
        // SAFETY: `comp` is present in the source, absent in the target.
        unsafe { self.move_entity(entity, loc, target, None) };
        true
    }

    /// Moves a single entity between archetypes.
    ///
    /// # Safety
    ///
    /// - `to != from.archetype`.
    /// - If `added` is `Some((c, data))`, `c` must be present in `to`,
    ///   absent from `from.archetype`, and `data` must point to a valid,
    ///   initialized value whose ownership this call takes over.
    /// - Every component present in `from.archetype` but absent from `to` is
    ///   dropped by this call.
    unsafe fn move_entity(
        &mut self,
        entity: Entity,
        from: Location,
        to: ArchetypeId,
        added: Option<(ComponentId, *const u8)>,
    ) {
        let from_idx = archetype_index(from.archetype);
        let to_idx = archetype_index(to);
        debug_assert_ne!(from_idx, to_idx, "migration must change archetype");

        // Split so both ends are uniquely borrowed; `split_at_mut` requires
        // `from_idx != to_idx`, which holds by the assertion above.
        let (src, dst) = if from_idx < to_idx {
            let (l, r) = self.archetypes.split_at_mut(to_idx);
            (&mut l[from_idx], &mut r[0])
        } else {
            let (l, r) = self.archetypes.split_at_mut(from_idx);
            (&mut r[0], &mut l[to_idx])
        };

        let src_row = from.row;
        let dst_row = dst.reserve_row(entity);

        // Move shared components from src to dst.
        let src_count = src.components().len();
        for i in 0..src_count {
            let comp = src.components()[i];
            if dst.has_component(comp) {
                let size = src.component_size(comp);
                if size == 0 {
                    continue;
                }
                // SAFETY: `src_row` is live, `dst_row` is reserved and
                // uninitialized; both pointers refer to distinct archetypes.
                unsafe {
                    let from = src.slot_ptr(comp, src_row);
                    let to = dst.slot_ptr_mut(comp, dst_row);
                    core::ptr::copy_nonoverlapping(from, to, size);
                }
            } else {
                // SAFETY: `src_row` is live.
                unsafe { src.drop_component(comp, src_row) };
            }
        }

        // Write the component added by this migration, if any.
        if let Some((comp, data)) = added {
            // SAFETY: `dst_row` is uninitialized for `comp`.
            unsafe { dst.write_component(comp, dst_row, data) };
        }

        // SAFETY: every slot in `dst_row` is now initialized.
        unsafe { dst.commit_row() };

        // Pop the source row without dropping anything: its components were
        // either moved into `dst` or explicitly dropped above.
        let last_row = src.len() - 1;
        let moved_entity = if src_row == last_row {
            None
        } else {
            // Bitwise-move the last row into the source row. No destructors
            // run: the source row's slots are already empty, the last row's
            // slots are being conceptually relocated.
            let src_count = src.components().len();
            for i in 0..src_count {
                let comp = src.components()[i];
                let size = src.component_size(comp);
                if size == 0 {
                    continue;
                }
                // SAFETY: both rows are live and distinct; `copy_nonoverlapping`
                // relocates raw bytes without invoking drop.
                unsafe { src.relocate_raw(comp, last_row, src_row) };
            }
            Some(src.entity_at(last_row))
        };
        src.pop_last_entity_and_swap(src_row);
        // SAFETY: the last row's slots are logically uninitialized.
        unsafe { src.shrink_len_no_drop() };

        // Fix up location bookkeeping.
        if let Some(moved) = moved_entity {
            self.entities.set_location(
                moved,
                Location {
                    archetype: from.archetype,
                    row: src_row,
                },
            );
        }
        self.entities.set_location(
            entity,
            Location {
                archetype: to,
                row: dst_row,
            },
        );
    }
}
