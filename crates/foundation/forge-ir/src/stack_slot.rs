//! Stack slot management — frame objects for spills, allocas, and emergencies.
//!
//! Stack slots represent fixed-size regions in the function's stack frame.
//! They are used both for explicit user allocations (alloca) and for
//! compiler-inserted spills during register allocation.

use super::entity::TypeId;
use super::string_pool::InternedStr;

// ============================================================
// StackSlotId
// ============================================================

/// Handle to a stack slot in the function's frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct StackSlotId(pub u32);

// ============================================================
// StackSlotKind
// ============================================================

/// The purpose of a stack slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackSlotKind {
    /// User-declared allocation (from alloca or explicit stack_addr).
    Explicit,
    /// Register spill slot — compiler-inserted during regalloc.
    Spill,
    /// Emergency spill slot — reserved for special cases.
    Emergency,
    /// Incoming argument passed on the stack.
    IncomingArg,
    /// Outgoing argument for a call.
    OutgoingArg,
}

// ============================================================
// StackSlotData
// ============================================================

/// Data for a single stack slot.
#[derive(Clone, Debug)]
pub struct StackSlotData {
    /// Size of the slot in bytes.
    pub size: u32,
    /// Required alignment in bytes.
    pub alignment: u32,
    /// Type of data stored (for debug info).
    pub ty: Option<TypeId>,
    /// Kind of slot.
    pub kind: StackSlotKind,
    /// Optional name for debugging.
    pub name: Option<InternedStr>,
    /// Offset from the frame pointer (computed during frame layout).
    /// Negative = below FP (typical), positive = above FP (outgoing args).
    pub offset: Option<i32>,
}

impl StackSlotData {
    /// Create a new explicit stack slot.
    pub fn explicit(size: u32, alignment: u32) -> Self {
        Self {
            size,
            alignment,
            ty: None,
            kind: StackSlotKind::Explicit,
            name: None,
            offset: None,
        }
    }

    /// Create a new spill slot for register allocation.
    pub fn spill(size: u32, alignment: u32) -> Self {
        Self {
            size,
            alignment,
            ty: None,
            kind: StackSlotKind::Spill,
            name: None,
            offset: None,
        }
    }

    /// Create a new emergency spill slot.
    pub fn emergency(size: u32, alignment: u32) -> Self {
        Self {
            size,
            alignment,
            ty: None,
            kind: StackSlotKind::Emergency,
            name: None,
            offset: None,
        }
    }

    /// Set the type of data stored in this slot.
    pub fn with_type(mut self, ty: TypeId) -> Self {
        self.ty = Some(ty);
        self
    }

    /// Set the name for debugging.
    pub fn with_name(mut self, name: InternedStr) -> Self {
        self.name = Some(name);
        self
    }
}

// ============================================================
// StackSlots
// ============================================================

/// Manager for a function's stack slots.
///
/// Stored in `Function` and used during frame layout, register allocation,
/// and code emission.
#[derive(Clone, Debug, Default)]
pub struct StackSlots {
    slots: Vec<StackSlotData>,
}

impl StackSlots {
    /// Create an empty stack slot manager.
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// Create a new stack slot and return its ID.
    pub fn create(&mut self, data: StackSlotData) -> StackSlotId {
        let id = StackSlotId(self.slots.len() as u32);
        self.slots.push(data);
        id
    }

    /// Create an explicit slot for user allocation.
    pub fn create_explicit(&mut self, size: u32, alignment: u32) -> StackSlotId {
        self.create(StackSlotData::explicit(size, alignment))
    }

    /// Create a spill slot for register allocation.
    pub fn create_spill(&mut self, size: u32, alignment: u32) -> StackSlotId {
        self.create(StackSlotData::spill(size, alignment))
    }

    /// Create an emergency slot.
    pub fn create_emergency(&mut self, size: u32, alignment: u32) -> StackSlotId {
        self.create(StackSlotData::emergency(size, alignment))
    }

    /// Get slot data by ID.
    pub fn get(&self, id: StackSlotId) -> &StackSlotData {
        &self.slots[id.0 as usize]
    }

    /// Get mutable slot data by ID.
    pub fn get_mut(&mut self, id: StackSlotId) -> &mut StackSlotData {
        &mut self.slots[id.0 as usize]
    }

    /// Set the frame offset for a slot.
    pub fn set_offset(&mut self, id: StackSlotId, offset: i32) {
        self.slots[id.0 as usize].offset = Some(offset);
    }

    /// Number of slots.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Check if there are no slots.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Iterate over all slots.
    pub fn iter(&self) -> impl Iterator<Item = (StackSlotId, &StackSlotData)> {
        self.slots
            .iter()
            .enumerate()
            .map(|(i, data)| (StackSlotId(i as u32), data))
    }

    /// Compute the total frame size needed for all slots.
    /// Returns (total_size, max_alignment).
    pub fn frame_size(&self) -> (u32, u32) {
        let mut total = 0u32;
        let mut max_align = 1u32;
        for slot in &self.slots {
            max_align = max_align.max(slot.alignment);
            total = (total + slot.alignment - 1) & !(slot.alignment - 1); // align up
            total += slot.size;
        }
        // Align total to max alignment
        total = (total + max_align - 1) & !(max_align - 1);
        (total, max_align)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stack_slots_basic() {
        let mut slots = StackSlots::new();
        let id1 = slots.create_explicit(4, 4);
        let id2 = slots.create_spill(8, 8);

        assert_eq!(slots.len(), 2);
        assert_eq!(slots.get(id1).size, 4);
        assert_eq!(slots.get(id2).kind, StackSlotKind::Spill);
    }

    #[test]
    fn test_frame_size() {
        let mut slots = StackSlots::new();
        slots.create_explicit(4, 4); // 0..4
        slots.create_explicit(8, 8); // 8..16 (padded from 4→8)
        let (total, align) = slots.frame_size();
        assert!(total >= 16); // 4 + pad(4) + 8 = 16
        assert_eq!(align, 8); // max alignment
    }

    #[test]
    fn test_emergency_slot() {
        let mut slots = StackSlots::new();
        let id = slots.create_emergency(16, 16);
        assert_eq!(slots.get(id).kind, StackSlotKind::Emergency);
        assert_eq!(slots.get(id).size, 16);
    }
}
