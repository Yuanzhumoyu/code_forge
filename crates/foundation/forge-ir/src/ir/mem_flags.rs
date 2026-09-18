//! Memory operation flags — alignment, volatility, ordering hints.
//!
//! Attached to Load/Store instructions to control memory ordering,
//! aliasing, and access semantics.

use bitflags::bitflags;

bitflags! {
    /// Memory access flags for Load/Store instructions.
    ///
    /// These control volatility, cache hints, atomic ordering,
    /// and aliasing guarantees.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct MemFlags: u16 {
        /// No flags.
        const NONE = 0;

        /// Volatile — cannot be eliminated, merged, or reordered.
        /// Like C `volatile` or LLVM `volatile` keyword.
        const VOLATILE = 1 << 0;

        /// Non-temporal — streaming store / prefetch hint.
        /// Indicates the access is unlikely to be reused soon.
        const NON_TEMPORAL = 1 << 1;

        /// Acquire semantic — prevents subsequent memory operations
        /// from being reordered before this load.
        const ACQUIRE = 1 << 2;

        /// Release semantic — prevents preceding memory operations
        /// from being reordered after this store.
        const RELEASE = 1 << 3;

        /// The loaded value is invariant within its scope.
        /// Allows CSE across calls and loops.
        const INVARIANT = 1 << 4;

        /// This access does not alias any other memory access.
        /// Strong guarantee — only use when proven.
        const NO_ALIAS = 1 << 5;

        /// This access does not trap on out-of-bounds access.
        /// Used for speculative loads.
        const NOTRAP = 1 << 6;

        /// Little-endian access (overrides target default).
        const LITTLE_ENDIAN = 1 << 7;

        /// Big-endian access (overrides target default).
        const BIG_ENDIAN = 1 << 8;

        /// The access is unaligned — the pointer may not satisfy
        /// the type's natural alignment.
        const UNALIGNED = 1 << 9;

        /// Read-only access guarantee (for stores: does not write new data).
        const READ_ONLY = 1 << 10;

        /// Write-only access (for loads: the loaded value is always
        /// overwritten before use).
        const WRITE_ONLY = 1 << 11;

        /// This is a heap access (vs. stack or global).
        const HEAP = 1 << 12;

        /// This access is to thread-local storage.
        const TLS = 1 << 13;
    }
}

impl MemFlags {
    /// Check if the access is volatile.
    pub fn is_volatile(self) -> bool {
        self.contains(Self::VOLATILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_none() {
        assert_eq!(MemFlags::NONE, MemFlags::empty());
    }

    #[test]
    fn test_volatile() {
        assert!(MemFlags::VOLATILE.is_volatile());
        assert!(!MemFlags::NONE.is_volatile());
    }
}
