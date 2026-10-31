//! v2 动态 ISA 插件夹具（M3）：只加谱 + 一行导出，宿主零 ISA 代码。
//!
//! 用真实 x86 谱，编出的机器码可在本机真执行；模块名改 `demoplugin` 以免与
//! 宿主内置 `x86_64` 同名。

forge_dsl::isa_from_file!("tests/isa/x86.toml", name = "demoplugin");

forge_codegen::export_isa_plugin!(demoplugin, targets = ["demoplugin64"]);
