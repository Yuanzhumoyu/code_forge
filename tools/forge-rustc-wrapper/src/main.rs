//! forge-rustc rustc wrapper（cargo 集成）。
//!
//! 原理：`-Zcodegen-backend` 是 crate 级全局设置，build-std 重编的
//! core/compiler_builtins 无法用 forge backend 编译（大量不支持的
//! intrinsic）。本 wrapper 拦截 cargo 调用的 rustc：对系统库 crate
//! 剥离 `-Zcodegen-backend`（回退 LLVM），其余 crate 保留 forge。
//!
//! 构建：`cargo build --manifest-path tools/forge-rustc-wrapper/Cargo.toml`
//! 用法：`RUSTC_WRAPPER=<path>/forge-rustc-wrapper.exe cargo +nightly -Z build-std=core build`
use std::process::Command;

const SYSTEM_CRATES: &[&str] = &[
    "core",
    "compiler_builtins",
    "alloc",
    "std",
    "panic_abort",
    "panic_unwind",
    // build script（编译宿主构建工具，绝不用 forge backend）
    "build_script_build",
];

fn main() {
    // 参数 0 = wrapper 自身路径；参数 1 = 真实 rustc 路径（cargo 传入）；
    // 其余 = rustc 参数
    let mut args = std::env::args().skip(1);
    let rustc_path = args.next().expect("rustc path as first arg");
    let rest: Vec<String> = args.collect();

    // 提取 --crate-name
    let crate_name = rest
        .windows(2)
        .find(|w| w[0] == "--crate-name")
        .map(|w| w[1].clone())
        .unwrap_or_default();
    let is_system = SYSTEM_CRATES.contains(&crate_name.as_str());

    // 过滤 -Zcodegen-backend（仅系统 crate）
    let filtered: Vec<String> = if is_system {
        rest.iter()
            .filter(|a| !a.starts_with("-Zcodegen-backend="))
            .cloned()
            .collect()
    } else {
        rest
    };

    let status = Command::new(&rustc_path).args(&filtered).status().unwrap();
    std::process::exit(status.code().unwrap_or(1));
}
