//! **rustc → aarch64 首光**（v27 M6-98）：`--target aarch64-unknown-none` 的
//! no_core crate 经 forge 后端编成对象，与 clang 汇编的 crt0 用 `ld.lld` 链接，
//! 在 `qemu-system-aarch64` 上**真执行**并以 semihosting 退出码断言结果。
//!
//! 为什么值得单独一条：它同时钉住三处会让"换目标"静默坏掉的地方——
//!
//! 1. 对象文件必须按 `--target` 建（曾 `TargetConfig::host()` ⇒ 写出 x86_64 COFF）；
//! 2. 裸机三元组 `*-none` 的 binary format 是 `Unknown`，必须按 ELF 处理；
//! 3. arm64 的 `BL` 调用重定位由**目标约定数据**给出（`R_AARCH64_CALL26` 位段），
//!    不是 4 字节字节位移；以及 arm64 谱面的 StackAddr/Load/Store 降级规则
//!    （`main` 里就有栈槽与调用）。
//!
//! 缺 `clang`/`ld.lld`/QEMU/后端 dylib 时**明确 SKIP**（不假绿、也不误红）。

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

fn backend_dll() -> PathBuf {
    workspace_root()
        .join("target")
        .join("debug")
        .join("forge_rustc.dll")
}

/// PATH 里找工具（Windows 补 `.exe`）。
fn tool(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    if let Ok(paths) = std::env::var("PATH") {
        for d in std::env::split_paths(&paths) {
            let p = d.join(&exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // clang/lld 的常见安装位置（LLVM 官方安装器）。
    for base in [r"D:\Program Files\LLVM\bin", r"C:\Program Files\LLVM\bin"] {
        let p = Path::new(base).join(&exe);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// QEMU system-mode aarch64：`QEMU_AARCH64` 覆盖 → 默认安装路径 → PATH。
fn qemu_aarch64() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("QEMU_AARCH64") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    for c in [
        r"D:\Program Files\qemu\qemu-system-aarch64.exe",
        r"C:\Program Files\qemu\qemu-system-aarch64.exe",
    ] {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Some(p);
        }
    }
    tool("qemu-system-aarch64")
}

fn run(cmd: &mut Command) -> std::process::Output {
    cmd.output()
        .unwrap_or_else(|e| panic!("启动 {:?} 失败：{e}", cmd))
}

fn assert_ok(out: &std::process::Output, what: &str) {
    assert!(
        out.status.success(),
        "{what} 失败：\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// no_core crate：`pick(42,7)` 走 **真实的 Call 约定**（aapcs64：x0/x1 → x0），
/// `main` 里还有一个栈槽（常量 42 先落栈再读回）——覆盖 StackAddr/Load/Store。
const PROBE_RS: &str = r#"
#![feature(no_core, lang_items)]
#![no_core]
#![no_main]
#![allow(internal_features)]

#[lang = "sized"]
trait Sized: MetaSized {}
#[lang = "meta_sized"]
trait MetaSized: PointeeSized {}
#[lang = "pointee_sized"]
trait PointeeSized {}
#[lang = "copy"]
trait Copy {}

#[inline(never)]
pub extern "C" fn pick(a: u64, _b: u64) -> u64 { a }

#[unsafe(no_mangle)]
pub extern "C" fn main() -> u64 { pick(42, 7) }
"#;

/// crt0：设栈 → `bl main` → semihosting `SYS_EXIT`（reason 0x20026，status=x0）。
/// 与 `forge-tests` 的 qemu 夹具同构，但这里刻意用 clang 汇编而不是手写字节。
const CRT0_S: &str = r#"
.text
.global _start
_start:
    ldr x0, =0x40080000
    mov sp, x0
    bl main
    sub sp, sp, #16
    movz x2, #0x2, lsl #16
    movk x2, #0x26
    str x2, [sp]
    str x0, [sp, #8]
    mov x1, sp
    mov x0, #0x18
    hlt #0xf000
"#;

#[test]
fn rustc_aarch64_object_links_and_runs_under_qemu() {
    let dll = backend_dll();
    if !dll.is_file() {
        eprintln!(
            "SKIP: 后端 dylib 缺失（{}）——先 `cargo build -p forge-rustc`",
            dll.display()
        );
        return;
    }
    let (Some(clang), Some(ldlld)) = (tool("clang"), tool("ld.lld")) else {
        eprintln!("SKIP: 缺 clang / ld.lld（aarch64 交叉汇编/链接需要）");
        return;
    };
    let Some(qemu) = qemu_aarch64() else {
        eprintln!("SKIP: qemu-system-aarch64 未安装");
        return;
    };
    let rustc = std::env::var_os("RUSTC").map_or_else(|| "rustc".into(), PathBuf::from);

    let dir = workspace_root()
        .join("target")
        .join("tmp")
        .join("arm64-rustc-exec");
    std::fs::create_dir_all(&dir).expect("创建产物目录");
    let src = dir.join("probe.rs");
    let crt0_s = dir.join("crt0.s");
    let obj = dir.join("probe.o");
    let crt0_o = dir.join("crt0.o");
    let elf = dir.join("probe.elf");
    std::fs::write(&src, PROBE_RS).expect("写 probe.rs");
    std::fs::write(&crt0_s, CRT0_S).expect("写 crt0.s");

    // ① forge 后端编出 aarch64 对象。
    let out = run(Command::new(&rustc).args([
        format!("-Zcodegen-backend={}", dll.display()).as_str(),
        "--target",
        "aarch64-unknown-none",
        "-C",
        "panic=abort",
        "-C",
        "opt-level=0",
        "--emit=obj",
        src.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]));
    assert_ok(&out, "rustc --target aarch64-unknown-none");
    // ② 对象必须是 **ELF64 / AArch64**（不是 host 的 COFF/x86-64）。
    let bytes = std::fs::read(&obj).expect("读对象");
    assert_eq!(
        &bytes[0..4],
        b"\x7fELF",
        "对象不是 ELF（目标三元组没生效？）"
    );
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    assert_eq!(
        machine, 183,
        "e_machine 必须是 EM_AARCH64(183)，实际 {machine}"
    );

    // ③ crt0 + 链接。
    let out = run(Command::new(clang).args([
        "--target=aarch64-unknown-none",
        "-c",
        crt0_s.to_str().unwrap(),
        "-o",
        crt0_o.to_str().unwrap(),
    ]));
    assert_ok(&out, "clang 汇编 crt0");
    let out = run(Command::new(ldlld).args([
        "-m",
        "aarch64elf",
        "-Ttext=0x40000000",
        "--entry=_start",
        crt0_o.to_str().unwrap(),
        obj.to_str().unwrap(),
        "-o",
        elf.to_str().unwrap(),
    ]));
    assert_ok(&out, "ld.lld 链接");
    assert!(elf.is_file(), "链接产物缺失");

    // ④ QEMU 真执行：`pick(42,7)` → x0=42 → semihosting 退出码 42。
    let out = run(Command::new(&qemu).args([
        "-M",
        "virt",
        "-cpu",
        "cortex-a57",
        "-kernel",
        elf.to_str().unwrap(),
        "-nographic",
        "-semihosting-config",
        "enable=on,target=native",
        "-monitor",
        "none",
        "-serial",
        "none",
        "-no-reboot",
    ]));
    let code = out
        .status
        .code()
        .unwrap_or_else(|| panic!("qemu 异常终止：\n{}", String::from_utf8_lossy(&out.stderr)));
    assert_eq!(
        code & 0xFF,
        42,
        "aarch64 执行结果应为 42（pick(42,7)），got {code}（产物：{}）",
        dir.display()
    );
}
