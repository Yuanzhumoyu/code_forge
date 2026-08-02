//! forge-tests 的 unicorn 构建脚本（简化版——不依赖系统 libclang/pkg-config/预装 unicorn）。
//!
//! 用 cmake 编译 vendored 的 unicorn C 源码（vendor/unicorn）为静态库，
//! 生成手写 FFI 绑定（src/exec/unicorn_ffi.rs）所需的链接目标。
//!
//! 唯一外部依赖：cmake + 一个 C 编译器（VS Build Tools / gcc / clang）。
//! 换环境（另一台机器）时只需有 cmake+C 编译器即可，无需 libclang、pkg-config、
//! 系统 unicorn 或预编译 DLL。

use std::env;
use std::path::PathBuf;

fn main() {
    // 仅当 exec-unicorn feature 启用时编译 unicorn
    let enabled = env::var("CARGO_FEATURE_EXEC_UNICORN").is_ok();
    if !enabled {
        return;
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let uc_dir = manifest.join("vendor").join("unicorn");

    // 用 cmake 编译静态库（x86 + aarch64 + riscv 架构）
    // 显式指定生成器：cmake crate 默认会探测新版本 VS（如 VS 18），
    // 老 cmake 不认识；本机 VS 2022 用 "Visual Studio 17 2022"。
    // 用 cmake 编译静态库（x86 + aarch64 + riscv 架构）
    // 显式指定生成器：cmake crate 默认会探测新版本 VS（如 VS 18），
    // 老 cmake 不认识；本机 VS 2022 用 "Visual Studio 17 2022"。
    // 只 build target（跳过 install——install 的符号链接在 Windows 需要特权）。
    let dst = cmake::Config::new(&uc_dir)
        .generator("Visual Studio 17 2022")
        .define("UNICORN_ARCH", "x86;aarch64;riscv")
        .define("UNICORN_SHARED", "OFF")
        .define("UNICORN_STATIC", "ON")
        .define("UNICORN_BUILD_TESTS", "OFF")
        .build_target("unicorn_static")
        .build();

    println!(
        "cargo:rustc-link-search=native={}",
        dst.join("build").join("Debug").display()
    );
    println!("cargo:rustc-link-lib=static=unicorn_static");
    println!("cargo:rustc-link-lib=static=unicorn-common");
    println!("cargo:rustc-link-lib=static=x86_64-softmmu");
    println!("cargo:rustc-link-lib=static=aarch64-softmmu");
    println!("cargo:rustc-link-lib=static=riscv64-softmmu");
    println!("cargo:rustc-link-lib=static=riscv32-softmmu");
    // Windows 无需 pthread/m（unicorn 静态库已内联；Unix 需要）
    if env::var("CARGO_CFG_WINDOWS").is_err() {
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=m");
    }

    // 头文件目录（FFI 绑定可能需要，虽然手写绑定不实际 include）
    println!("cargo:include={}", uc_dir.join("include").display());
}
