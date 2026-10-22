//! `--emit=obj -o <path>` 必须真的写出那个路径（v27 M6 回归守卫）。
//!
//! ## 为什么值得一条独立测试
//!
//! 自定义 `CodegenBackend` **不走** `rustc_codegen_ssa::back::write` 的
//! `copy_if_one_unit`——那一步才是"把 CGU 临时对象复制到用户 `-o`"的地方。
//! 我们只把对象写在自己的固定名下并报给 rustc，于是 `-o` **静默不生效**：
//! 编译"成功"，用户要的文件却不存在。实测（修之前）：
//!
//! ```text
//! rustc -Zcodegen-backend=… --emit=obj -o honored.obj → 只得到 forge_codegen_output.o
//! rustc（LLVM 后端）            --emit=obj -o honored_llvm.obj → 得到 honored_llvm.obj
//! ```
//!
//! 这一类"看起来成功、产物在别处"最容易浪费排查时间（所有反汇编对拍都会先卡在这里），
//! 所以钉两条：
//!
//! 1. **`-o <具体文件>`** ⇒ 该文件存在、非空，且目录里**不留**我们自己的临时对象；
//! 2. **`--out-dir`（没点名文件）** ⇒ 固定名 `forge_codegen_output.o` 仍在
//!    （确定性与数据符号两类测试按这个前缀认产物）。

use std::path::{Path, PathBuf};
use std::process::Command;

const SRC: &str = r#"
#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> i32 {
    42
}
"#;

fn backend_dll() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace 根可解析");
    root.join("target").join("debug").join("forge_rustc.dll")
}

/// 编一次对象，返回 rustc 的 stderr（失败时用于断言信息）。
fn compile(dll: &Path, src: &Path, extra: &[&str]) -> String {
    let out = Command::new("rustc")
        .arg(format!("-Zcodegen-backend={}", dll.display()))
        .args(["-Z", "threads=1"])
        .args(["-C", "panic=abort"])
        .args(["--edition", "2024"])
        .args(["--target", "x86_64-pc-windows-msvc"])
        .args(extra)
        .arg(src)
        .output()
        .expect("启动 rustc");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "编译失败：\n{stderr}");
    stderr
}

#[test]
fn emit_obj_honours_explicit_output_path() {
    let dll = backend_dll();
    assert!(dll.is_file(), "找不到后端 dylib：{}", dll.display());

    let dir = std::env::temp_dir().join("forge_rustc_obj_out");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let src = dir.join("probe.rs");
    std::fs::write(&src, SRC).expect("写源文件");

    // ① 点名的具体文件：必须存在，且我们自己的固定名**不留**。
    let want = dir.join("named_output.obj");
    compile(
        &dll,
        &src,
        &["--emit=obj", "-o", want.to_str().expect("路径可转字符串")],
    );
    assert!(
        want.is_file(),
        "`-o {}` 没有生效——这是自定义后端漏掉 copy_if_one_unit 的老毛病",
        want.display()
    );
    let n = std::fs::metadata(&want).expect("读产物元数据").len();
    assert!(n > 0, "`-o` 产出的对象是空文件");
    let stray = dir.join("forge_codegen_output.o");
    assert!(
        !stray.is_file(),
        "点名了具体输出文件时不该再留下 `{}`",
        stray.display()
    );

    // ② `--out-dir` 形态：固定名仍在（测试与工具按它认产物）。
    let outdir = dir.join("od");
    std::fs::create_dir_all(&outdir).expect("建 out-dir");
    compile(
        &dll,
        &src,
        &[
            "--crate-type",
            "lib",
            "--emit=obj",
            "--out-dir",
            outdir.to_str().expect("路径可转字符串"),
        ],
    );
    assert!(
        outdir.join("forge_codegen_output.o").is_file(),
        "`--out-dir` 形态下固定名对象不见了：测试/工具都按这个前缀认产物"
    );
}
