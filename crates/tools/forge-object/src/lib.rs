//! Forge Object — ELF/PE/Mach-O object file writer.

pub mod object_writer;
pub mod target;

pub use object_writer::ObjectWriter;
pub use target::TargetConfig;
