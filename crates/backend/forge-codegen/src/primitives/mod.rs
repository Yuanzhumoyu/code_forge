//! ISA-agnostic encoding primitives.
//!
//! Bit-field packing and LEB128 are the only encoding utilities that live
//! in the framework. All ISA-specific helpers are DSL-generated.

pub mod leb128;
