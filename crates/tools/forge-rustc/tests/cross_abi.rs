//! **跨后端 ABI 互操作**（v27 P1 / `WA-52` 的真值门）：forge 编的代码与 LLVM 编的
//! 代码在**同一条 128 位标量边界**上双向对接。
//!
//! ## 为什么必须跨后端
//!
//! 后端自己编调用方、自己编被调方时，两边用同一套（哪怕错误的）形态也**自洽**——
//! `u128` 的 `RCX:RDX` 对 `RAX:RDX` 在自编自调下永远看不出问题（v27 M5 之前的全套 e2e
//! 就是这样全绿的）。真实错误只在**混链**时暴露：LLVM（Win64）把 `extern "C"` 的
//! `u128` 形参按**引用**传（`mov 0x8(%rcx),%rax`）、按 **`XMM0`** 返回
//! （`movdqa (%rsp),%xmm0`）。把我们的 `RCX:RDX`/`RAX:RDX` 接上去就是 AV/错值。
//!
//! 本文件把两个方向都钉住：
//!
//! 1. **forge 调用方 + LLVM 被调方**（`cg_c_wide_int_caller_forge`）；
//! 2. **LLVM 调用方 + forge 被调方**（`cg_c_wide_int_callee_forge`）。
//!
//! 两侧都用 `extern "C"`（Win64 的 C 家族），并且**同时**覆盖"按引用实参"与
//! "向量寄存器返回"——只测一侧会漏掉另一半的形态。
//!
//! 产物落 `target/cross-abi/`（仓库规矩：跑出来的东西不进工作区）。

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// 被调方：三个函数覆盖"按引用实参 / 向量返回 / 两者同时"。
const CALLEE_SRC: &str = r#"
#[unsafe(no_mangle)]
pub extern "C" fn wide_arg(a: u128) -> u64 {
    (a >> 64) as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn wide_ret(a: u64) -> u128 {
    ((a as u128) << 64) | 3
}

#[unsafe(no_mangle)]
pub extern "C" fn wide_round(a: u128) -> u128 {
    a
}
"#;

/// 调用方：真跑并给出 42（任一子项不对就返回错误码，便于定位是哪一项）。
const CALLER_SRC: &str = r#"
#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

unsafe extern "C" {
    fn wide_arg(a: u128) -> u64;
    fn wide_ret(a: u64) -> u128;
    fn wide_round(a: u128) -> u128;
}

unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let v = core::hint::black_box(((7u128) << 64) | 11);
    let arg = unsafe { wide_arg(v) };
    let ret = unsafe { wide_ret(core::hint::black_box(9u64)) };
    let round = unsafe { wide_round(core::hint::black_box(v ^ 0x1234)) };
    let code = if arg != 7 {
        1
    } else if (ret >> 64) as u64 != 9 || (ret & 0xFF) as u64 != 3 {
        2
    } else if round != (v ^ 0x1234) {
        3
    } else {
        42
    };
    unsafe { ExitProcess(code as u32) }
}
"#;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace 根可解析")
}

fn backend_dll() -> PathBuf {
    let dll = workspace_root()
        .join("target")
        .join("debug")
        .join("forge_rustc.dll");
    assert!(
        dll.is_file(),
        "找不到后端 dylib：{}（先跑 `cargo build -p forge-rustc`）",
        dll.display()
    );
    dll
}

/// 每个方向一个独立目录（两个测试并行时不抢产物）。
fn case_dir(name: &str) -> PathBuf {
    let dir = workspace_root().join("target").join("cross-abi").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建产物目录");
    dir
}

/// Windows 控制台程序的最小链接参数（与 e2e 同口径；`mainCRTStartup` 是我们的入口）。
fn entry_link_args() -> String {
    "/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib"
        .to_string()
}

fn run_rustc(args: &[&str]) -> String {
    let out = Command::new("rustc")
        .args(args)
        .output()
        .expect("启动 rustc");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "rustc 失败：\n{stderr}");
    stderr
}

fn write_sources(dir: &Path) -> (PathBuf, PathBuf) {
    let callee = dir.join("callee.rs");
    let caller = dir.join("caller.rs");
    std::fs::write(&callee, CALLEE_SRC).expect("写被调方");
    std::fs::write(&caller, CALLER_SRC).expect("写调用方");
    (callee, caller)
}

fn run_exe(exe: &Path) -> i32 {
    let status = Command::new(exe).status().expect("运行产物");
    status.code().unwrap_or(-1)
}

/// **方向一**：被调方由 **LLVM** 编成 `.obj`，调用方由 **forge** 编并链接。
#[test]
fn cg_c_wide_int_caller_forge() {
    let dll = backend_dll();
    let dir = case_dir("caller-forge");
    let (callee_src, caller_src) = write_sources(&dir);
    let callee_obj = dir.join("callee_llvm.obj");
    let exe = dir.join("caller_forge.exe");

    // 被调方：系统 rustc（LLVM 后端）。`--crate-type=lib` 只为拿到两个 `no_mangle` 符号。
    run_rustc(&[
        "--edition",
        "2024",
        "--crate-type=lib",
        "--emit=obj",
        "-C",
        "panic=abort",
        callee_src.to_str().expect("路径"),
        "-o",
        callee_obj.to_str().expect("路径"),
    ]);

    // 调用方：forge 后端。
    let dll_arg = format!("-Zcodegen-backend={}", dll.display());
    let link = entry_link_args();
    let obj_arg = format!("link-arg={}", callee_obj.display());
    run_rustc(&[
        &dll_arg,
        "--edition",
        "2024",
        "-C",
        "panic=abort",
        "-C",
        &format!("link-args={link}"),
        "-C",
        &obj_arg,
        caller_src.to_str().expect("路径"),
        "-o",
        exe.to_str().expect("路径"),
    ]);

    assert_eq!(
        run_exe(&exe),
        42,
        "跨后端失败：forge 调用方 + LLVM 被调方的 128 位 C ABI 互操作不正确（1=实参 2=返回 3=两者）"
    );
}

/// **方向二**：被调方由 **forge** 编成 `.obj`，调用方由 **LLVM** 编并链接。
#[test]
fn cg_c_wide_int_callee_forge() {
    let dll = backend_dll();
    let dir = case_dir("callee-forge");
    let (callee_src, caller_src) = write_sources(&dir);
    let callee_obj = dir.join("callee_forge.obj");
    let exe = dir.join("caller_llvm.exe");

    // 被调方：forge 后端（`--emit=obj`）。
    let dll_arg = format!("-Zcodegen-backend={}", dll.display());
    run_rustc(&[
        &dll_arg,
        "--edition",
        "2024",
        "--crate-type=lib",
        "--emit=obj",
        "-C",
        "panic=abort",
        callee_src.to_str().expect("路径"),
        "-o",
        callee_obj.to_str().expect("路径"),
    ]);

    // 调用方：系统 rustc（LLVM）。
    let link = entry_link_args();
    let obj_arg = format!("link-arg={}", callee_obj.display());
    run_rustc(&[
        "--edition",
        "2024",
        "-C",
        "panic=abort",
        "-C",
        &format!("link-args={link}"),
        "-C",
        &obj_arg,
        caller_src.to_str().expect("路径"),
        "-o",
        exe.to_str().expect("路径"),
    ]);

    assert_eq!(
        run_exe(&exe),
        42,
        "跨后端失败：LLVM 调用方 + forge 被调方的 128 位 C ABI 互操作不正确（1=实参 2=返回 3=两者）"
    );
}
