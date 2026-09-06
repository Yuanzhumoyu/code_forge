//! 子命令实现。按规格 `src/cmd/{build,run,doctor,backend,init}.rs` 拆分，
//! run 复用 build 的构建逻辑（`build::execute`），避免重复。
pub mod doctor;
