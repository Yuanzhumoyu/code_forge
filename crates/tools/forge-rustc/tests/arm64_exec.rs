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
    let out = run(Command::new(&rustc).current_dir(&dir).args([
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
/// **aarch64 + `core` 首光**（v27 M6-105）：`#![no_std]` crate 真的用到 LLVM 编的
/// `core`（`cmp::max/min`、`mem::size_of`），forge 后端编出对象，与
/// `libcore.rlib` + `libcompiler_builtins.rlib` 一起 `ld.lld` 链接后在 QEMU 真跑。
///
/// 为什么值得一条独立测试：
/// 1. **标准库来源**：离线装不上 `rust-std-aarch64-unknown-none`，但 `rust-src` 在本地
///    ⇒ 用 rustc 直接为目标编 `core`/`compiler_builtins`（本测试把它们缓存到
///    `target/tmp/arm64-core-sysroot`，缺了才编）。
/// 2. **debug 溢出检查**：`opt-level=0` 默认开 `debug-assertions` ⇒ `a + b` 会发
///    `SaddOverflow`（走谱里的 `ADDS`+`CSEL` 标志规则）；结果正确即证明标志位序列对。
/// 3. 跨编译器边界（forge 编的 crate ↔ LLVM 编的 core）的 aapcs64 调用与返回。
const CORE_PROBE_RS: &str = r#"
#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "C" fn forge_entry(a: i32, b: i32) -> i32 {
    let m = core::cmp::max(a, b);
    let n = core::cmp::min(a + 1, b + 2);
    let sz = core::mem::size_of::<u64>() as i32;
    m + n + sz
}
"#;

/// crt0（带实参与 `core` 调用）：`forge_entry(1,2)` = max(1,2)+min(2,4)+8 = 12。
const CORE_CRT0_S: &str = r#"
.text
.global _start
_start:
    ldr x9, =0x40080000
    mov sp, x9
    mov x0, #1
    mov x1, #2
    bl forge_entry
    sub sp, sp, #16
    movz x2, #0x2, lsl #16
    movk x2, #0x26
    str x2, [sp]
    str x0, [sp, #8]
    mov x1, sp
    mov x0, #0x18
    hlt #0xf000
"#;

/// **core 链接所需的 mem* 兜底**（v27 M6-112）：带 debug-assertions 的 core 会把
/// `escape_ascii`/`dec2flt` 这类"只编不跑"的代码也拉进来，它们引用 `memset/memcpy/
/// memcmp/__udivti3`（`compiler_builtins` 的 mem feature 只覆盖 aarch64 的一部分）。
/// 不参与被测路径，只让链接通过；不带断言的两档用不到它们、链上也无害。
const CORE_STUBS_S: &str = r#"
.text
.global memcmp
memcmp:
    cbz x2, 2f
1:
    ldrb w3, [x0], #1
    ldrb w4, [x1], #1
    subs w5, w3, w4
    b.ne 3f
    subs x2, x2, #1
    b.ne 1b
2:
    mov x0, #0
    ret
3:
    mov x0, x5
    ret

.global memset
memset:
    mov x3, x0
    cbz x2, 2f
1:
    strb w1, [x3], #1
    subs x2, x2, #1
    b.ne 1b
2:
    ret

.global memcpy
memcpy:
    mov x3, x0
    cbz x2, 2f
1:
    ldrb w4, [x1], #1
    strb w4, [x3], #1
    subs x2, x2, #1
    b.ne 1b
2:
    ret

.global __udivti3
__udivti3:
    mov x0, #0
    mov x1, #0
    ret
"#;
/// 为目标编一个标准库 rlib（core / compiler_builtins）：已存在就复用。
fn build_target_rlib(
    rustc: &Path,
    out_dir: &Path,
    name: &str,
    src: &Path,
    extra: &[&str],
) -> PathBuf {
    let lib = out_dir.join(format!("lib{name}.rlib"));
    if lib.is_file() {
        return lib;
    }
    let mut cmd = Command::new(rustc);
    cmd.args([
        "--target",
        "aarch64-unknown-none",
        "--crate-name",
        name,
        "--crate-type",
        "rlib",
        "-C",
        "panic=abort",
        "-C",
        "opt-level=2",
        "-C",
        "debuginfo=0",
        "--edition",
        "2024",
        "-Z",
        "force-unstable-if-unmarked",
    ]);
    cmd.args(extra);
    cmd.arg("--out-dir").arg(out_dir).arg(src);
    let out = run(&mut cmd);
    assert_ok(&out, &format!("rustc 编 {name}（aarch64-unknown-none）"));
    assert!(lib.is_file(), "{name} rlib 未产出");
    lib
}

#[test]
fn rustc_aarch64_core_probe_runs_under_qemu() {
    let dll = backend_dll();
    if !dll.is_file() {
        eprintln!("SKIP: 后端 dylib 缺失（先 `cargo build -p forge-rustc`）");
        return;
    }
    let (Some(clang), Some(ldlld)) = (tool("clang"), tool("ld.lld")) else {
        eprintln!("SKIP: 缺 clang / ld.lld");
        return;
    };
    let Some(qemu) = qemu_aarch64() else {
        eprintln!("SKIP: qemu-system-aarch64 未安装");
        return;
    };
    let rustc = std::env::var_os("RUSTC").map_or_else(|| "rustc".into(), PathBuf::from);
    // rust-src 定位：`rustc --print sysroot` + `/lib/rustlib/src/rust/library`。
    let sysroot = run(Command::new(&rustc).arg("--print").arg("sysroot"));
    assert_ok(&sysroot, "rustc --print sysroot");
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    let lib_dir = sysroot.join("lib/rustlib/src/rust/library");
    let core_src = lib_dir.join("core/src/lib.rs");
    let cb_src = lib_dir.join("compiler-builtins/compiler-builtins/src/lib.rs");
    if !core_src.is_file() || !cb_src.is_file() {
        eprintln!("SKIP: 缺 rust-src（{}）", lib_dir.display());
        return;
    }

    let dir = workspace_root()
        .join("target")
        .join("tmp")
        .join("arm64-core-probe");
    let dir = dir.join("core_probe");
    let sys = dir.join("sysroot");
    std::fs::create_dir_all(&sys).expect("创建产物目录");
    let core = build_target_rlib(&rustc, &sys, "core", &core_src, &[]);
    let cb = build_target_rlib(
        &rustc,
        &sys,
        "compiler_builtins",
        &cb_src,
        &[
            "--cfg",
            r#"feature="mem""#,
            "--cfg",
            r#"feature="compiler-builtins""#,
            "--extern",
            &format!("core={}", core.display()),
        ],
    );

    let src = dir.join("probe_core.rs");
    let obj = dir.join("probe_core.o");
    let crt0_s = dir.join("crt0_core.s");
    let crt0_o = dir.join("crt0_core.o");
    let elf = dir.join("probe_core.elf");
    std::fs::write(&src, CORE_PROBE_RS).expect("写 probe_core.rs");
    std::fs::write(&crt0_s, CORE_CRT0_S).expect("写 crt0_core.s");

    let out = run(Command::new(&rustc).current_dir(&dir).args([
        format!("-Zcodegen-backend={}", dll.display()).as_str(),
        "--target",
        "aarch64-unknown-none",
        "--edition",
        "2021",
        "--crate-type",
        "lib",
        "-C",
        "panic=abort",
        "-C",
        "opt-level=0",
        "--emit=obj",
        "--extern",
        &format!("core={}", core.display()),
        "--extern",
        &format!("compiler_builtins={}", cb.display()),
        src.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]));
    assert_ok(&out, "forge 编 #![no_std] + core 的 crate");

    let out = run(Command::new(&clang).args([
        "--target=aarch64-unknown-none",
        "-c",
        crt0_s.to_str().unwrap(),
        "-o",
        crt0_o.to_str().unwrap(),
    ]));
    assert_ok(&out, "clang 汇编 core crt0");
    let out = run(Command::new(&ldlld).args([
        "-m",
        "aarch64elf",
        "-Ttext=0x40000000",
        "--entry=_start",
        crt0_o.to_str().unwrap(),
        obj.to_str().unwrap(),
        core.to_str().unwrap(),
        cb.to_str().unwrap(),
        "-o",
        elf.to_str().unwrap(),
    ]));
    assert_ok(&out, "ld.lld 链接 core 探针");
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
        12,
        "core 探针结果应为 12（max(1,2)+min(2,4)+8），got {code}（产物：{}）",
        dir.display()
    );
}
/// **scaled 溢出真执行守卫**（v27 M6-107）：40 个跨调用活跃的标量值把寄存器压力拉满，
/// 溢出槽会落到 `sp + >255B`——只有 scaled `str/ldr [sp,#imm]` 能表达。该用例同时：
///
/// 1. QEMU 真跑出 `(Σ 期望 1039) % 100 = 39`；
/// 2. 对**对象字节**断言：机器码里 **没有** unscaled `stur/ldur`（顶层 0xF8），
///    但**有**至少一条 `str [sp, #imm>255]`（scaled imm12 的 imm ≥ 32）。
///
/// 为什么需要它：矩阵用例的帧都很浅（imm9 也能编过），单靠它们无法区分"换了 scaled
/// 形态"与"碰巧偏移还小"。
#[test]
fn rustc_aarch64_scaled_spill_probe_runs_under_qemu() {
    let dll = backend_dll();
    if !dll.is_file() {
        eprintln!("SKIP: 后端 dylib 缺失（先 `cargo build -p forge-rustc`）");
        return;
    }
    let (Some(clang), Some(ldlld)) = (tool("clang"), tool("ld.lld")) else {
        eprintln!("SKIP: 缺 clang / ld.lld");
        return;
    };
    let Some(qemu) = qemu_aarch64() else {
        eprintln!("SKIP: qemu-system-aarch64 未安装");
        return;
    };
    let rustc = std::env::var_os("RUSTC").map_or_else(|| "rustc".into(), PathBuf::from);
    let sysroot = run(Command::new(&rustc).arg("--print").arg("sysroot"));
    assert_ok(&sysroot, "rustc --print sysroot");
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    let lib_dir = sysroot.join("lib/rustlib/src/rust/library");
    let core_src = lib_dir.join("core/src/lib.rs");
    let cb_src = lib_dir.join("compiler-builtins/compiler-builtins/src/lib.rs");
    if !core_src.is_file() || !cb_src.is_file() {
        eprintln!("SKIP: 缺 rust-src（{}）", lib_dir.display());
        return;
    }

    let dir = workspace_root()
        .join("target")
        .join("tmp")
        .join("arm64-core-probe");
    let dir = dir.join("scaled_spill");
    let sys = dir.join("sysroot");
    std::fs::create_dir_all(&sys).expect("创建产物目录");
    let core = build_target_rlib(&rustc, &sys, "core", &core_src, &[]);
    let cb = build_target_rlib(
        &rustc,
        &sys,
        "compiler_builtins",
        &cb_src,
        &[
            "--cfg",
            r#"feature="mem""#,
            "--cfg",
            r#"feature="compiler-builtins""#,
            "--extern",
            &format!("core={}", core.display()),
        ],
    );

    // 40 个标量跨 `bump` 调用活跃 ⇒ 必然溢出（寄存器池装不下）。
    let mut src_text = String::from(
        r#"#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop {} }

#[inline(never)]
pub extern "C" fn bump(x: i64) -> i64 { x * 3 + 1 }

#[unsafe(no_mangle)]
pub extern "C" fn forge_entry(a: i32, b: i32) -> i32 {
"#,
    );
    for i in 0..40 {
        src_text.push_str(&format!("    let x{i} = (a as i64) + (b as i64) + {i};\n"));
    }
    src_text
        .push_str("    let x0 = bump(x0);\n    let x20 = bump(x20);\n    let x39 = bump(x39);\n");
    let sum = (0..40)
        .map(|i| format!("x{i}"))
        .collect::<Vec<_>>()
        .join(" + ");
    src_text.push_str(&format!("    let s = {sum};\n    (s % 100) as i32\n}}\n"));

    let src = dir.join("probe_scaled_spill.rs");
    let obj = dir.join("probe_scaled_spill.o");
    let crt0_s = dir.join("crt0_scaled_spill.s");
    let crt0_o = dir.join("crt0_scaled_spill.o");
    let elf = dir.join("probe_scaled_spill.elf");
    std::fs::write(&src, &src_text).expect("写 probe_scaled_spill.rs");
    std::fs::write(&crt0_s, CORE_CRT0_S).expect("写 crt0_scaled_spill.s");

    let out = run(Command::new(&rustc).current_dir(&dir).args([
        format!("-Zcodegen-backend={}", dll.display()).as_str(),
        "--target",
        "aarch64-unknown-none",
        "--edition",
        "2021",
        "--crate-type",
        "lib",
        "-C",
        "panic=abort",
        "-C",
        "opt-level=0",
        "-C",
        "debug-assertions=off",
        "-C",
        "overflow-checks=off",
        "--emit=obj",
        "--extern",
        &format!("core={}", core.display()),
        "--extern",
        &format!("compiler_builtins={}", cb.display()),
        src.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]));
    assert_ok(&out, "forge 编 scaled 溢出探针");

    // 字节层断言：没有 unscaled STUR/LDUR（顶层 0xF8），且有 scaled imm12 ≥ 32（>255B）。
    let bytes = std::fs::read(&obj).expect("读对象");
    let mut unscaled = 0usize;
    let mut wide_scaled = 0usize;
    for w in bytes.windows(4).step_by(4) {
        let word = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        if (word >> 24) == 0xF8 {
            unscaled += 1;
        }
        // `str/ldr x, [sp,#imm]`：顶层 0xF9、Rn=31、imm12（以 8 缩放）≥ 32。
        if (word >> 24) == 0xF9 && ((word >> 5) & 0x1F) == 31 && ((word >> 10) & 0xFFF) >= 32 {
            wide_scaled += 1;
        }
    }
    assert_eq!(
        unscaled,
        0,
        "溢出/帧件不应再发 unscaled STUR/LDUR（顶层 0xF8）：{unscaled} 条（产物：{}）",
        dir.display()
    );
    assert!(
        wide_scaled > 0,
        "应出现 scaled 且偏移 >255B 的 str/ldr [sp]（imm12≥32）：{wide_scaled} 条（产物：{}）",
        dir.display()
    );

    let out = run(Command::new(&clang).args([
        "--target=aarch64-unknown-none",
        "-c",
        crt0_s.to_str().unwrap(),
        "-o",
        crt0_o.to_str().unwrap(),
    ]));
    assert_ok(&out, "clang 汇编 scaled 溢出 crt0");
    let out = run(Command::new(&ldlld).args([
        "-m",
        "aarch64elf",
        "-Ttext=0x40000000",
        "--entry=_start",
        crt0_o.to_str().unwrap(),
        obj.to_str().unwrap(),
        core.to_str().unwrap(),
        cb.to_str().unwrap(),
        "-o",
        elf.to_str().unwrap(),
    ]));
    assert_ok(&out, "ld.lld 链接 scaled 溢出探针");
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
        39,
        "scaled 溢出探针结果应为 39（Σ 1039 % 100），got {code}（产物：{}）",
        dir.display()
    );
}
/// **ZST 聚合字段守卫**（v27 M6-116）：`()` 字段没有内存表示。
///
/// 聚合构造的字段写入路径（`lower/statement.rs`）对每个字段都 `lower_operand` + `Store`；
/// 单位字段的 `map_type` 是 VOID ⇒ 造出 `Iconst ty=VOID`（`ValDesc.bits = 0`）⇒
/// `Store no matching rule`（真实 fmt/sort 程序在 O0 全部卡在这里）。ZST 字段跳过即修好。
///
/// 断言真执行：a=1,b=2 ⇒ S{a=3} ⇒ 3+3+1 = **7**。
#[test]
fn rustc_aarch64_zst_aggregate_probe_runs_under_qemu() {
    let dll = backend_dll();
    if !dll.is_file() {
        eprintln!("SKIP: 后端 dylib 缺失（先 `cargo build -p forge-rustc`）");
        return;
    }
    let (Some(clang), Some(ldlld)) = (tool("clang"), tool("ld.lld")) else {
        eprintln!("SKIP: 缺 clang / ld.lld");
        return;
    };
    let Some(qemu) = qemu_aarch64() else {
        eprintln!("SKIP: qemu-system-aarch64 未安装");
        return;
    };
    let rustc = std::env::var_os("RUSTC").map_or_else(|| "rustc".into(), PathBuf::from);
    let sysroot = run(Command::new(&rustc).arg("--print").arg("sysroot"));
    assert_ok(&sysroot, "rustc --print sysroot");
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    let lib_dir = sysroot.join("lib/rustlib/src/rust/library");
    let core_src = lib_dir.join("core/src/lib.rs");
    let cb_src = lib_dir.join("compiler-builtins/compiler-builtins/src/lib.rs");
    if !core_src.is_file() || !cb_src.is_file() {
        eprintln!("SKIP: 缺 rust-src（{}）", lib_dir.display());
        return;
    }

    let dir = workspace_root()
        .join("target")
        .join("tmp")
        .join("arm64-core-probe")
        .join("zst_agg");
    let sys = dir.join("sysroot");
    std::fs::create_dir_all(&sys).expect("创建产物目录");
    let core = build_target_rlib(&rustc, &sys, "core", &core_src, &[]);
    let cb = build_target_rlib(
        &rustc,
        &sys,
        "compiler_builtins",
        &cb_src,
        &[
            "--cfg",
            r#"feature="mem""#,
            "--cfg",
            r#"feature="compiler-builtins""#,
            "--extern",
            &format!("core={}", core.display()),
        ],
    );

    let src_text = r#"#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop {} }

struct S { a: i32, b: () }

#[unsafe(no_mangle)]
pub extern "C" fn forge_entry(a: i32, b: i32) -> i32 {
    let s = S { a: a + b, b: () };
    let t = (s.a, ());
    s.a + t.0 + a
}
"#;

    let src = dir.join("probe_zst_agg.rs");
    let obj = dir.join("probe_zst_agg.o");
    let crt0_s = dir.join("crt0_zst_agg.s");
    let crt0_o = dir.join("crt0_zst_agg.o");
    let elf = dir.join("probe_zst_agg.elf");
    std::fs::write(&src, src_text).expect("写 probe_zst_agg.rs");
    std::fs::write(&crt0_s, CORE_CRT0_S).expect("写 crt0_zst_agg.s");

    let out = run(Command::new(&rustc).current_dir(&dir).args([
        format!("-Zcodegen-backend={}", dll.display()).as_str(),
        "--target",
        "aarch64-unknown-none",
        "--edition",
        "2021",
        "--crate-type",
        "lib",
        "-C",
        "panic=abort",
        "-C",
        "opt-level=0",
        "-C",
        "debug-assertions=off",
        "-C",
        "overflow-checks=off",
        "--emit=obj",
        "--extern",
        &format!("core={}", core.display()),
        "--extern",
        &format!("compiler_builtins={}", cb.display()),
        src.to_str().unwrap(),
        "-o",
        obj.to_str().unwrap(),
    ]));
    assert_ok(&out, "forge 编 ZST 聚合探针");

    let out = run(Command::new(&clang).args([
        "--target=aarch64-unknown-none",
        "-c",
        crt0_s.to_str().unwrap(),
        "-o",
        crt0_o.to_str().unwrap(),
    ]));
    assert_ok(&out, "clang 汇编 ZST 聚合 crt0");
    let out = run(Command::new(&ldlld).args([
        "-m",
        "aarch64elf",
        "-Ttext=0x40000000",
        "--entry=_start",
        crt0_o.to_str().unwrap(),
        obj.to_str().unwrap(),
        core.to_str().unwrap(),
        cb.to_str().unwrap(),
        "-o",
        elf.to_str().unwrap(),
    ]));
    assert_ok(&out, "ld.lld 链接 ZST 聚合探针");
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
        7,
        "ZST 聚合探针结果应为 7（got {code}；产物：{}）",
        dir.display()
    );
}
/// **大帧守卫**（v27 M6-115）：帧 > 4095 字节时 `sub sp, sp, #imm12` 一条写不出来。
///
/// 之前只在 `frame_alloc` 上按槽的 `min/max` 分块；`frame_set`（fp ← sp + frame）
/// 仍是一条 `add imm12`，实测 `[u64; 600]`（4800B，帧 5936B）直接编译失败：
/// `Addimmx: immediate 5936 out of range [0, 4095]`。现在两处都按槽范围分块，
/// 落点仍是 frame_size（中间值不参与语义）。
///
/// 断言真执行：a=1 ⇒ big[i] = 3i+1 ⇒ big[0]+big[599] = 1+1798 = **1799**。
#[test]
fn rustc_aarch64_big_frame_probe_runs_under_qemu() {
    let dll = backend_dll();
    if !dll.is_file() {
        eprintln!("SKIP: 后端 dylib 缺失（先 `cargo build -p forge-rustc`）");
        return;
    }
    let (Some(clang), Some(ldlld)) = (tool("clang"), tool("ld.lld")) else {
        eprintln!("SKIP: 缺 clang / ld.lld");
        return;
    };
    let Some(qemu) = qemu_aarch64() else {
        eprintln!("SKIP: qemu-system-aarch64 未安装");
        return;
    };
    let rustc = std::env::var_os("RUSTC").map_or_else(|| "rustc".into(), PathBuf::from);
    let sysroot = run(Command::new(&rustc).arg("--print").arg("sysroot"));
    assert_ok(&sysroot, "rustc --print sysroot");
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    let lib_dir = sysroot.join("lib/rustlib/src/rust/library");
    let core_src = lib_dir.join("core/src/lib.rs");
    let cb_src = lib_dir.join("compiler-builtins/compiler-builtins/src/lib.rs");
    if !core_src.is_file() || !cb_src.is_file() {
        eprintln!("SKIP: 缺 rust-src（{}）", lib_dir.display());
        return;
    }

    let dir = workspace_root()
        .join("target")
        .join("tmp")
        .join("arm64-core-probe")
        .join("big_frame");
    let sys = dir.join("sysroot");
    std::fs::create_dir_all(&sys).expect("创建产物目录");
    let core = build_target_rlib(&rustc, &sys, "core", &core_src, &[]);
    let cb = build_target_rlib(
        &rustc,
        &sys,
        "compiler_builtins",
        &cb_src,
        &[
            "--cfg",
            r#"feature="mem""#,
            "--cfg",
            r#"feature="compiler-builtins""#,
            "--extern",
            &format!("core={}", core.display()),
        ],
    );

    let src_text = r#"#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop {} }

#[unsafe(no_mangle)]
pub extern "C" fn forge_entry(a: i32) -> i32 {
    let mut big = [0u64; 600];
    let mut i = 0usize;
    while i < big.len() {
        big[i] = (i as u64).wrapping_mul(3).wrapping_add(a as u64);
        i += 1;
    }
    big[0].wrapping_add(big[599]) as i32
}
"#
    .to_string();

    let src = dir.join("probe_big_frame.rs");
    let crt0_s = dir.join("crt0_big_frame.s");
    let crt0_o = dir.join("crt0_big_frame.o");
    std::fs::write(&src, &src_text).expect("写 probe_big_frame.rs");
    std::fs::write(&crt0_s, CORE_CRT0_S).expect("写 crt0_big_frame.s");

    for opt in ["0", "2"] {
        let obj = dir.join(format!("probe_big_frame_o{opt}.o"));
        let elf = dir.join(format!("probe_big_frame_o{opt}.elf"));
        let out = run(Command::new(&rustc).current_dir(&dir).args([
            format!("-Zcodegen-backend={}", dll.display()).as_str(),
            "--target",
            "aarch64-unknown-none",
            "--edition",
            "2021",
            "--crate-type",
            "lib",
            "-C",
            "panic=abort",
            "-C",
            &format!("opt-level={opt}"),
            "-C",
            "debug-assertions=off",
            "-C",
            "overflow-checks=off",
            "--emit=obj",
            "--extern",
            &format!("core={}", core.display()),
            "--extern",
            &format!("compiler_builtins={}", cb.display()),
            src.to_str().unwrap(),
            "-o",
            obj.to_str().unwrap(),
        ]));
        assert_ok(&out, &format!("forge 编大帧探针（O{opt}）"));

        let out = run(Command::new(&clang).args([
            "--target=aarch64-unknown-none",
            "-c",
            crt0_s.to_str().unwrap(),
            "-o",
            crt0_o.to_str().unwrap(),
        ]));
        assert_ok(&out, "clang 汇编大帧 crt0");
        let out = run(Command::new(&ldlld).args([
            "-m",
            "aarch64elf",
            "-Ttext=0x40000000",
            "--entry=_start",
            crt0_o.to_str().unwrap(),
            obj.to_str().unwrap(),
            core.to_str().unwrap(),
            cb.to_str().unwrap(),
            "-o",
            elf.to_str().unwrap(),
        ]));
        assert_ok(&out, &format!("ld.lld 链接大帧探针（O{opt}）"));
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
        let code = out.status.code().unwrap_or_else(|| {
            panic!(
                "qemu 异常终止（O{opt}）：\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        assert_eq!(
            code & 0xFF,
            1799 & 0xFF,
            "大帧探针结果应为 1799（got {code}，O{opt}；产物：{}）",
            dir.display()
        );
    }
}
/// **fmt 守卫**（v27 M6-118）：`core::fmt` 整数格式化 + 手写排序真执行。
///
/// 根因：`frame_layout_info` 的 **fp-inside** 分支只把栈槽平移设成 `fp_push_bytes`
/// （=16），漏了 callee-saved 保存区（x19..x28，最多 80B）。于是局部槽会落进
/// `[fp - fp_push - cs_bytes, fp)`：QEMU gdbstub 写观察点实测 `write_str` 的局部
/// `fp-0x40` 正是 `x24` 的保存槽 ⇒ 返回后 `x24` 被局部值覆盖 ⇒ 内层
/// `core::fmt::write` 的 `blr x24` 跳到 0。修法：栈槽平移 = `fp_push + cs_bytes`。
///
/// 断言真执行：a=1,b=2 ⇒ s=165、v[0]=3、v[9]=30、count_ones(165)=4、collatz(27)=111、
/// 格式化 `"165:3:30"` 长 8 ⇒ 165+3+30+4+111+8 = 321 ⇒ 低 8 位 **65**。
#[test]
fn rustc_aarch64_fmt_probe_runs_under_qemu() {
    let dll = backend_dll();
    if !dll.is_file() {
        eprintln!("SKIP: 后端 dylib 缺失（先 `cargo build -p forge-rustc`）");
        return;
    }
    let (Some(clang), Some(ldlld)) = (tool("clang"), tool("ld.lld")) else {
        eprintln!("SKIP: 缺 clang / ld.lld");
        return;
    };
    let Some(qemu) = qemu_aarch64() else {
        eprintln!("SKIP: qemu-system-aarch64 未安装");
        return;
    };
    let rustc = std::env::var_os("RUSTC").map_or_else(|| "rustc".into(), PathBuf::from);
    let sysroot = run(Command::new(&rustc).arg("--print").arg("sysroot"));
    assert_ok(&sysroot, "rustc --print sysroot");
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    let lib_dir = sysroot.join("lib/rustlib/src/rust/library");
    let core_src = lib_dir.join("core/src/lib.rs");
    let cb_src = lib_dir.join("compiler-builtins/compiler-builtins/src/lib.rs");
    if !core_src.is_file() || !cb_src.is_file() {
        eprintln!("SKIP: 缺 rust-src（{}）", lib_dir.display());
        return;
    }

    let dir = workspace_root()
        .join("target")
        .join("tmp")
        .join("arm64-core-probe")
        .join("fmt");
    let sys = dir.join("sysroot");
    std::fs::create_dir_all(&sys).expect("创建产物目录");
    let core = build_target_rlib(&rustc, &sys, "core", &core_src, &[]);
    let cb = build_target_rlib(
        &rustc,
        &sys,
        "compiler_builtins",
        &cb_src,
        &[
            "--cfg",
            r#"feature="mem""#,
            "--cfg",
            r#"feature="compiler-builtins""#,
            "--extern",
            &format!("core={}", core.display()),
        ],
    );

    let src_text = r#"#![no_std]
#![no_main]

use core::fmt::Write;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop {} }

struct Buf { data: [u8; 64], len: usize }

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let mut i = 0usize;
        while i < s.len() {
            if self.len < self.data.len() {
                self.data[self.len] = s.as_bytes()[i];
                self.len += 1;
            }
            i += 1;
        }
        Ok(())
    }
}

fn collatz(mut x: u32) -> u32 {
    let mut steps = 0;
    while x != 1 {
        if x % 2 == 0 { x /= 2; } else { x = x * 3 + 1; }
        steps += 1;
    }
    steps
}

#[unsafe(no_mangle)]
pub extern "C" fn forge_entry(a: i32, b: i32) -> i32 {
    let mut v = [0i32; 10];
    let mut i = 0usize;
    while i < v.len() { v[i] = (a + b) * (i as i32 + 1); i += 1; }
    let mut s = 0i32;
    let mut k = 0usize;
    while k < v.len() { s += v[k]; k += 1; }
    let mut i2 = 1usize;
    while i2 < v.len() {
        let mut j = i2;
        while j > 0 && v[j - 1] > v[j] {
            let t = v[j - 1];
            v[j - 1] = v[j];
            v[j] = t;
            j -= 1;
        }
        i2 += 1;
    }
    let mut buf = Buf { data: [0u8; 64], len: 0 };
    let _ = write!(buf, "{}:{}:{}", s, v[0], v[9]);
    let opt: Option<i32> = if s > 0 { Some(s) } else { None };
    let bits = match opt { Some(x) => x.count_ones() as i32, None => 0 };
    let steps = collatz(27) as i32;
    (s + v[0] + v[9] + bits + steps + buf.len as i32) & 0xFF
}
"#;

    let src = dir.join("probe_fmt.rs");
    let crt0_s = dir.join("crt0_fmt.s");
    let crt0_o = dir.join("crt0_fmt.o");
    let stubs_s = dir.join("stubs_fmt.s");
    let stubs_o = dir.join("stubs_fmt.o");
    std::fs::write(&src, src_text).expect("写 probe_fmt.rs");
    std::fs::write(&crt0_s, CORE_CRT0_S).expect("写 crt0_fmt.s");
    std::fs::write(&stubs_s, CORE_STUBS_S).expect("写 stubs_fmt.s");

    for (src_s, out_o, what) in [(&crt0_s, &crt0_o, "crt0"), (&stubs_s, &stubs_o, "stubs")] {
        let out = run(Command::new(&clang).args([
            "--target=aarch64-unknown-none",
            "-c",
            src_s.to_str().unwrap(),
            "-o",
            out_o.to_str().unwrap(),
        ]));
        assert_ok(&out, &format!("clang 汇编 fmt {what}"));
    }

    // **O0/O1/O2 全档**（v27 M6-119）：O1/O2 会跑 IR pass 与 verifier——这一档是
    // 「`const-fold`/`pre` 产出合法 IR」与「优化后语义不变」的双重证明。
    for level in ["0", "1", "2"] {
        let obj = dir.join(format!("probe_fmt_o{level}.o"));
        let elf = dir.join(format!("probe_fmt_o{level}.elf"));
        let out = run(Command::new(&rustc).current_dir(&dir).args([
            format!("-Zcodegen-backend={}", dll.display()).as_str(),
            "--target",
            "aarch64-unknown-none",
            "--edition",
            "2021",
            "--crate-type",
            "lib",
            "-C",
            "panic=abort",
            "-C",
            &format!("opt-level={level}"),
            "-C",
            "debug-assertions=off",
            "-C",
            "overflow-checks=off",
            "--emit=obj",
            "--extern",
            &format!("core={}", core.display()),
            "--extern",
            &format!("compiler_builtins={}", cb.display()),
            src.to_str().unwrap(),
            "-o",
            obj.to_str().unwrap(),
        ]));
        assert_ok(&out, &format!("forge 编 fmt 探针（-O{level}）"));
        let out = run(Command::new(&ldlld).args([
            "-m",
            "aarch64elf",
            "-Ttext=0x40000000",
            "--entry=_start",
            crt0_o.to_str().unwrap(),
            stubs_o.to_str().unwrap(),
            obj.to_str().unwrap(),
            core.to_str().unwrap(),
            cb.to_str().unwrap(),
            "-o",
            elf.to_str().unwrap(),
        ]));
        assert_ok(&out, &format!("ld.lld 链接 fmt 探针（-O{level}）"));
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
        let code = out.status.code().unwrap_or_else(|| {
            panic!(
                "qemu 异常终止（-O{level}）：\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        assert_eq!(
            code & 0xFF,
            65,
            "fmt 探针 -O{level} 结果应为 65（got {code}；产物：{}）",
            dir.display()
        );
    }
}

/// **core::slice::sort_unstable 守卫**（v27 M6-122）：真实排序入口真执行。
///
/// 这是迄今最"重"的 core 程序：`ipnsort`/`smallsort`/`CopyOnDrop` + `ptr::swap`/
/// `ptr::copy`/`copy_nonoverlapping` + 函数项比较器（`&mut <i32 as PartialOrd>::lt`）。
/// 它一次性暴露了四类后端缺陷（全部在本轮修掉）：
///
/// - `CopyNonOverlapping` 每元素固定 8 字节（i32 越界 4 字节 + 4 对齐地址 `str x`
///   ⇒ arm64 Data Abort），且 `ptr::copy` 缺 memmove 反向分支（元素右移被复制成
///   `[2,2,4,3]`）；
/// - 小聚合（`MaybeUninit<[u8;4]>`）被 `map_type` 兜成 PTR ⇒ `swap_chunk` 用 8 字节
///   读写 4 字节对象；
/// - `TerminatorKind::Drop` 只连边、不调 drop glue ⇒ `CopyOnDrop` 从不写回 gap 元素；
/// - `size_of_val_raw::<[T]>` 按 unsized layout 得 0 ⇒ `swap_nonoverlapping` 整个跳过。
///
/// 修前 `[2,1].sort_unstable()` 得 `[2,2]`；修后对 16 元乱序数组排序并校验
/// 连续性（返回 `v[0]+v[15]==15`）。
///
/// 跑 **-O0/O1/O2 三档**（v27 M6-123）：`-O1+` 时 rustc 的 MIR inliner 会把
/// `#[inline]` 实例标成 available-externally 且不进 CGU 分区，而 forge 优化器
/// 没有内联 pass ⇒ 直接调用点会留未定义符号（`CopyOnDrop::drop`/`revswap`）。
/// 后端现在按 lowering 期登记的"被引用实例"做 fixpoint，把缺失实例补编进一个
/// 合成 CGU（`upstream_monomorphization` 为 None 的才补），三档都能链接真跑。
#[test]
fn rustc_aarch64_sort_probe_runs_under_qemu() {
    let dll = backend_dll();
    if !dll.is_file() {
        eprintln!("SKIP: 后端 dylib 缺失（先 `cargo build -p forge-rustc`）");
        return;
    }
    let (Some(clang), Some(ldlld)) = (tool("clang"), tool("ld.lld")) else {
        eprintln!("SKIP: 缺 clang / ld.lld");
        return;
    };
    let Some(qemu) = qemu_aarch64() else {
        eprintln!("SKIP: qemu-system-aarch64 未安装");
        return;
    };
    let rustc = std::env::var_os("RUSTC").map_or_else(|| "rustc".into(), PathBuf::from);
    let sysroot = run(Command::new(&rustc).arg("--print").arg("sysroot"));
    assert_ok(&sysroot, "rustc --print sysroot");
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    let lib_dir = sysroot.join("lib/rustlib/src/rust/library");
    let core_src = lib_dir.join("core/src/lib.rs");
    let cb_src = lib_dir.join("compiler-builtins/compiler-builtins/src/lib.rs");
    if !core_src.is_file() || !cb_src.is_file() {
        eprintln!("SKIP: 缺 rust-src（{}）", lib_dir.display());
        return;
    }

    let dir = workspace_root()
        .join("target")
        .join("tmp")
        .join("arm64-core-probe")
        .join("sort");
    let sys = dir.join("sysroot");
    std::fs::create_dir_all(&sys).expect("创建产物目录");
    let core = build_target_rlib(&rustc, &sys, "core", &core_src, &[]);
    let cb = build_target_rlib(
        &rustc,
        &sys,
        "compiler_builtins",
        &cb_src,
        &[
            "--cfg",
            r#"feature="mem""#,
            "--cfg",
            r#"feature="compiler-builtins""#,
            "--extern",
            &format!("core={}", core.display()),
        ],
    );

    let src_text = r#"#![no_std]
#![no_main]

use core::cmp::Ordering;
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop {} }


#[unsafe(no_mangle)]
pub extern "C" fn forge_entry(a: i32, b: i32) -> i32 {
    let _ = (a, b);
    let mut v: [i32; 16] = [9, 3, 14, 1, 7, 12, 5, 2, 15, 8, 6, 11, 4, 13, 10, 0];
    v.sort_unstable();
    let mut k = 1usize;
    while k < v.len() {
        if v[k] != v[k - 1] + 1 { return 201; }
        k += 1;
    }
    // `Ord::cmp`（三路比较 `BinOp::Cmp` ⇒ `Ordering` repr(i8)，Less=-1 是负值）：
    // -O0 不内联/不常量折叠这条路径，负判别参与 switchInt 时必须按符号扩展
    //（否则 A64 的 32 位 CMPIMMW 用 0xFF 比 0xFFFF_FFFF 恒假）。
    let mut m = 0i32;
    if matches!(7.cmp(&3), Ordering::Greater) { m |= 1; }
    if matches!(3.cmp(&7), Ordering::Less) { m |= 2; }
    if matches!(5.cmp(&5), Ordering::Equal) { m |= 4; }
    if m != 7 { return 100 + m; }
    // `&str → &[u8]` 的胖指针 transmute（-O2 把 `str::as_bytes` 内联成它）：
    // 常量不在栈槽里，必须按 ScalarPair 写 ptr+len（旧实现写 `null`，
    // `PtrMetadata` 读 len=0）。
    let s: &str = "abcd";
    if s.as_bytes().len() != 4 { return 202; }
    if s.as_bytes()[0] != b'a' { return 203; }
    (v[0] + v[15]) & 0xFF
}
"#;

    let src = dir.join("probe_sort.rs");
    let crt0_s = dir.join("crt0_sort.s");
    let crt0_o = dir.join("crt0_sort.o");
    let stubs_s = dir.join("stubs_sort.s");
    let stubs_o = dir.join("stubs_sort.o");
    std::fs::write(&src, src_text).expect("写 probe_sort.rs");
    std::fs::write(&crt0_s, CORE_CRT0_S).expect("写 crt0_sort.s");
    std::fs::write(&stubs_s, CORE_STUBS_S).expect("写 stubs_sort.s");
    for (src_s, out_o, what) in [(&crt0_s, &crt0_o, "crt0"), (&stubs_s, &stubs_o, "stubs")] {
        let out = run(Command::new(&clang).args([
            "--target=aarch64-unknown-none",
            "-c",
            src_s.to_str().unwrap(),
            "-o",
            out_o.to_str().unwrap(),
        ]));
        assert_ok(&out, &format!("clang 汇编 sort {what}"));
    }

    // **O0/O1/O2 全档**（v27 M6-123）：-O1+ 的 MIR inliner 会把 `#[inline]`
    // 实例标成 available-externally 且不进 CGU 分区；这次靠后端的"按需补编"
    // 把被引用但未发射的实例补进合成 CGU，所以三档都能链接并真跑。
    for level in ["0", "1", "2"] {
        let obj = dir.join(format!("probe_sort_o{level}.o"));
        let elf = dir.join(format!("probe_sort_o{level}.elf"));
        let out = run(Command::new(&rustc).current_dir(&dir).args([
            format!("-Zcodegen-backend={}", dll.display()).as_str(),
            "--target",
            "aarch64-unknown-none",
            "--edition",
            "2021",
            "--crate-type",
            "lib",
            "-C",
            "panic=abort",
            "-C",
            &format!("opt-level={level}"),
            "-C",
            "debug-assertions=off",
            "-C",
            "overflow-checks=off",
            "--emit=obj",
            "--extern",
            &format!("core={}", core.display()),
            "--extern",
            &format!("compiler_builtins={}", cb.display()),
            src.to_str().unwrap(),
            "-o",
            obj.to_str().unwrap(),
        ]));
        assert_ok(&out, &format!("forge 编 sort 探针（-O{level}）"));
        let out = run(Command::new(&ldlld).args([
            "-m",
            "aarch64elf",
            "-Ttext=0x40000000",
            "--entry=_start",
            crt0_o.to_str().unwrap(),
            stubs_o.to_str().unwrap(),
            obj.to_str().unwrap(),
            core.to_str().unwrap(),
            cb.to_str().unwrap(),
            "-o",
            elf.to_str().unwrap(),
        ]));
        assert_ok(&out, &format!("ld.lld 链接 sort 探针（-O{level}）"));
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
        let code = out.status.code().unwrap_or_else(|| {
            panic!(
                "qemu 异常终止（sort 探针 -O{level}）：\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        assert_eq!(
            code & 0xFF,
            15,
            "sort 探针 -O{level} 结果应为 15（排序后连续 0..15，首尾和；got {code}；\
             产物：{}）",
            dir.display()
        );
    }
}

/// **迭代器守卫**（v27 M6-117）：`v.iter()` / `next()` / `.copied().sum()` 真执行。
///
/// 根因（v27 M6-116 补漏）：结构体逐字段构造循环**没有跳过 ZST 字段**——
/// `Iter` 的 `_marker: PhantomData`（0 字节）被写成 8 字节，写到 `fp-16`，
/// 即被调方（`Iter::new`）的 **x29 保存槽**；返回后调用者 fp 变 0，下一条
/// fp 相对 store 直接 Data Abort（QEMU `-d int`：FAR=-0x20、ELR 在
/// `slice::<impl [T]>::iter`；gdbstub 写观察点抓到真凶 PC 写 0 到该槽）。
///
/// 断言真执行：a=1 ⇒ v=[1,2,3,4] ⇒ 两个 10 相加 = **20**。
#[test]
fn rustc_aarch64_iterator_probe_runs_under_qemu() {
    let dll = backend_dll();
    if !dll.is_file() {
        eprintln!("SKIP: 后端 dylib 缺失（先 `cargo build -p forge-rustc`）");
        return;
    }
    let (Some(clang), Some(ldlld)) = (tool("clang"), tool("ld.lld")) else {
        eprintln!("SKIP: 缺 clang / ld.lld");
        return;
    };
    let Some(qemu) = qemu_aarch64() else {
        eprintln!("SKIP: qemu-system-aarch64 未安装");
        return;
    };
    let rustc = std::env::var_os("RUSTC").map_or_else(|| "rustc".into(), PathBuf::from);
    let sysroot = run(Command::new(&rustc).arg("--print").arg("sysroot"));
    assert_ok(&sysroot, "rustc --print sysroot");
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    let lib_dir = sysroot.join("lib/rustlib/src/rust/library");
    let core_src = lib_dir.join("core/src/lib.rs");
    let cb_src = lib_dir.join("compiler-builtins/compiler-builtins/src/lib.rs");
    if !core_src.is_file() || !cb_src.is_file() {
        eprintln!("SKIP: 缺 rust-src（{}）", lib_dir.display());
        return;
    }

    let dir = workspace_root()
        .join("target")
        .join("tmp")
        .join("arm64-core-probe")
        .join("iterator");
    let sys = dir.join("sysroot");
    std::fs::create_dir_all(&sys).expect("创建产物目录");
    let core = build_target_rlib(&rustc, &sys, "core", &core_src, &[]);
    let cb = build_target_rlib(
        &rustc,
        &sys,
        "compiler_builtins",
        &cb_src,
        &[
            "--cfg",
            r#"feature="mem""#,
            "--cfg",
            r#"feature="compiler-builtins""#,
            "--extern",
            &format!("core={}", core.display()),
        ],
    );

    let src_text = r#"#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop {} }

#[unsafe(no_mangle)]
pub extern "C" fn forge_entry(a: i32, b: i32) -> i32 {
    let _ = b;
    let mut v = [0i32; 4];
    let mut i = 0usize;
    while i < v.len() { v[i] = a + i as i32; i += 1; }
    let mut s1 = 0i32;
    let mut it = v.iter();
    while let Some(x) = it.next() { s1 += *x; }
    let s2: i32 = v.iter().copied().sum();
    // 闭包适配器（`Map`/`Filter`）：-O0 不内联，闭包经 `FnMut::call_mut`
    // 间接调用；`Map` 是 16 字节 ZST-尾巴聚合（Pair 几何）+ 闭包环境，
    // 曾因 ABI 分类不一致（Map::fold 的 self 当指针读）死循环。
    let s3: i32 = v.iter().copied().map(|x| x * 2).sum();
    if s3 != 20 { return 140; }
    let s4: i32 = v.iter().copied().filter(|x| x % 2 == 0).sum();
    if s4 != 6 { return 141; }

    (s1 + s2) & 0xFF
}
"#;

    let src = dir.join("probe_iter.rs");
    let crt0_s = dir.join("crt0_iter.s");
    let crt0_o = dir.join("crt0_iter.o");
    std::fs::write(&src, src_text).expect("写 probe_iter.rs");
    std::fs::write(&crt0_s, CORE_CRT0_S).expect("写 crt0_iter.s");

    let stubs_s = dir.join("stubs_iter.s");
    let stubs_o = dir.join("stubs_iter.o");
    std::fs::write(&stubs_s, CORE_STUBS_S).expect("写 stubs_iter.s");
    let out = run(Command::new(&clang).args([
        "--target=aarch64-unknown-none",
        "-c",
        crt0_s.to_str().unwrap(),
        "-o",
        crt0_o.to_str().unwrap(),
    ]));
    assert_ok(&out, "clang 汇编迭代器 crt0");
    let out = run(Command::new(&clang).args([
        "--target=aarch64-unknown-none",
        "-c",
        stubs_s.to_str().unwrap(),
        "-o",
        stubs_o.to_str().unwrap(),
    ]));
    assert_ok(&out, "clang 汇编迭代器 stubs");

    // **O0/O1/O2 全档**（v27 M6-120）：这条用例曾只在 O0 跑——O1 暴露了 `const-fold`
    // 按结果位宽（i1）截断比较操作数的缺陷（`0 < 4` 折成 false → `ub_checks` 断言 panic）。
    for level in ["0", "1", "2"] {
        let obj = dir.join(format!("probe_iter_o{level}.o"));
        let elf = dir.join(format!("probe_iter_o{level}.elf"));
        let out = run(Command::new(&rustc).current_dir(&dir).args([
            format!("-Zcodegen-backend={}", dll.display()).as_str(),
            "--target",
            "aarch64-unknown-none",
            "--edition",
            "2021",
            "--crate-type",
            "lib",
            "-C",
            "panic=abort",
            "-C",
            &format!("opt-level={level}"),
            "-C",
            "debug-assertions=off",
            "-C",
            "overflow-checks=off",
            "--emit=obj",
            "--extern",
            &format!("core={}", core.display()),
            "--extern",
            &format!("compiler_builtins={}", cb.display()),
            src.to_str().unwrap(),
            "-o",
            obj.to_str().unwrap(),
        ]));
        assert_ok(&out, &format!("forge 编迭代器探针（-O{level}）"));
        let out = run(Command::new(&ldlld).args([
            "-m",
            "aarch64elf",
            "-Ttext=0x40000000",
            "--entry=_start",
            crt0_o.to_str().unwrap(),
            stubs_o.to_str().unwrap(),
            obj.to_str().unwrap(),
            core.to_str().unwrap(),
            cb.to_str().unwrap(),
            "-o",
            elf.to_str().unwrap(),
        ]));
        assert_ok(&out, &format!("ld.lld 链接迭代器探针（-O{level}）"));
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
        let code = out.status.code().unwrap_or_else(|| {
            panic!(
                "qemu 异常终止（-O{level}）：\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        assert_eq!(
            code & 0xFF,
            20,
            "迭代器探针 -O{level} 结果应为 20（got {code}；产物：{}）",
            dir.display()
        );
    }
}

/// **core 真程序守卫**（v27 M6-111）：`core::mem::swap` + 循环在 **O0/O2 两档**都要跑对。
///
/// 这条用例覆盖的是本轮修掉的两个数据缺口：
///
/// 1. arm64 的 `B`/`B_*`/`CBZ`/`CBNZ` 原先**没有 `effect`** ⇒ 生成物
///    `is_branch()`/`branch_targets()` 恒假/恒空 ⇒ 活区间的 CFG **没有分支边**
///    （循环回边丢失）⇒ "preheader 定义、循环头使用"的值被当成已死，寄存器在循环体里
///    被复用（`core::mem::swap` 的循环上界被指针冲掉，指针越界 fault）；
/// 2. 修好 CFG 后溢出压力真实上升，出现"一条指令要 3 个 scratch"⇒ 谱面 scratch 从
///    2 个补到 3 个。
///
/// 断言真执行结果：a=1,b=2 ⇒ swap 后 (y−x)+(x+y)+Σ[1,2,3] = −1+3+6 = **8**。
#[test]
fn rustc_aarch64_core_swap_probe_runs_under_qemu() {
    let dll = backend_dll();
    if !dll.is_file() {
        eprintln!("SKIP: 后端 dylib 缺失（先 `cargo build -p forge-rustc`）");
        return;
    }
    let (Some(clang), Some(ldlld)) = (tool("clang"), tool("ld.lld")) else {
        eprintln!("SKIP: 缺 clang / ld.lld");
        return;
    };
    let Some(qemu) = qemu_aarch64() else {
        eprintln!("SKIP: qemu-system-aarch64 未安装");
        return;
    };
    let rustc = std::env::var_os("RUSTC").map_or_else(|| "rustc".into(), PathBuf::from);
    let sysroot = run(Command::new(&rustc).arg("--print").arg("sysroot"));
    assert_ok(&sysroot, "rustc --print sysroot");
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    let lib_dir = sysroot.join("lib/rustlib/src/rust/library");
    let core_src = lib_dir.join("core/src/lib.rs");
    let cb_src = lib_dir.join("compiler-builtins/compiler-builtins/src/lib.rs");
    if !core_src.is_file() || !cb_src.is_file() {
        eprintln!("SKIP: 缺 rust-src（{}）", lib_dir.display());
        return;
    }

    let dir = workspace_root()
        .join("target")
        .join("tmp")
        .join("arm64-core-probe");
    let dir = dir.join("core_swap");
    let sys = dir.join("sysroot");
    std::fs::create_dir_all(&sys).expect("创建产物目录");
    let core = build_target_rlib(&rustc, &sys, "core", &core_src, &[]);
    let cb = build_target_rlib(
        &rustc,
        &sys,
        "compiler_builtins",
        &cb_src,
        &[
            "--cfg",
            r#"feature="mem""#,
            "--cfg",
            r#"feature="compiler-builtins""#,
            "--extern",
            &format!("core={}", core.display()),
        ],
    );

    let src_text = r#"#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop {} }

// `GlobalAddr` 的真实使用：静态数据的地址由 MOVW 四片重定位给出，必须能读回 5
//（只编不跑不算数——链接器给错片/漏片时这里会读成垃圾）。
static MAGIC: i32 = 5;

#[unsafe(no_mangle)]
pub extern "C" fn forge_entry(a: i32, b: i32) -> i32 {
    let mut x = a;
    let mut y = b;
    core::mem::swap(&mut x, &mut y);
    let arr = [1i32, 2, 3];
    let mut s = 0i32;
    let mut i = 0usize;
    while i < arr.len() {
        s += arr[i];
        i += 1;
    }
    let m = unsafe { core::ptr::read_volatile(&MAGIC as *const i32) };
    (y - x) + (x + y) + s + m
}
"#;
    let src = dir.join("probe_core_swap.rs");
    let crt0_s = dir.join("crt0_core_swap.s");
    let crt0_o = dir.join("crt0_core_swap.o");
    std::fs::write(&src, src_text).expect("写 probe_core_swap.rs");
    std::fs::write(&crt0_s, CORE_CRT0_S).expect("写 crt0_core_swap.s");
    let out = run(Command::new(&clang).args([
        "--target=aarch64-unknown-none",
        "-c",
        crt0_s.to_str().unwrap(),
        "-o",
        crt0_o.to_str().unwrap(),
    ]));
    assert_ok(&out, "clang 汇编 core swap crt0");
    let stubs_s = dir.join("core_stubs.s");
    let stubs_o = dir.join("core_stubs.o");
    std::fs::write(&stubs_s, CORE_STUBS_S).expect("写 core_stubs.s");
    let out = run(Command::new(&clang).args([
        "--target=aarch64-unknown-none",
        "-c",
        stubs_s.to_str().unwrap(),
        "-o",
        stubs_o.to_str().unwrap(),
    ]));
    assert_ok(&out, "clang 汇编 core stubs");

    // 第三档是 **debug-assertions 开** 的 O0：这一档会编出 core 的
    // precondition/overflow 检查，覆盖 `GlobalAddr`（MOVW 四片重定位）与
    // `UmulOverflow`（UDIV 判溢出）——正是 v27 M6-112 新增的两条数据。
    for (label, level, dbg_off, want) in [
        ("o0", "0", true, 13i32),
        ("o2", "2", true, 13i32),
        ("o0dbg", "0", false, 13i32),
    ] {
        let obj = dir.join(format!("probe_core_swap_{label}.o"));
        let elf = dir.join(format!("probe_core_swap_{label}.elf"));
        let mut compile_flags: Vec<String> = vec![
            format!("-Zcodegen-backend={}", dll.display()),
            "--target".into(),
            "aarch64-unknown-none".into(),
            "--edition".into(),
            "2021".into(),
            "--crate-type".into(),
            "lib".into(),
            "-C".into(),
            "panic=abort".into(),
            "-C".into(),
            format!("opt-level={level}"),
            "-C".into(),
            "overflow-checks=off".into(),
            "--emit=obj".into(),
            "--extern".into(),
            format!("core={}", core.display()),
            "--extern".into(),
            format!("compiler_builtins={}", cb.display()),
            src.to_str().unwrap().into(),
            "-o".into(),
            obj.to_str().unwrap().into(),
        ];
        if dbg_off {
            compile_flags.push("-C".into());
            compile_flags.push("debug-assertions=off".into());
        }
        let out = run(Command::new(&rustc).current_dir(&dir).args(&compile_flags));
        assert_ok(&out, &format!("forge 编 core swap 探针（{label}）"));
        let out = run(Command::new(&ldlld).args([
            "-m",
            "aarch64elf",
            "-Ttext=0x40000000",
            "--entry=_start",
            crt0_o.to_str().unwrap(),
            stubs_o.to_str().unwrap(),
            obj.to_str().unwrap(),
            core.to_str().unwrap(),
            cb.to_str().unwrap(),
            "-o",
            elf.to_str().unwrap(),
        ]));
        assert_ok(&out, &format!("ld.lld 链接 core swap 探针（{label}）"));
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
        let code = out.status.code().unwrap_or_else(|| {
            panic!(
                "qemu 异常终止（{label}）：\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        assert_eq!(
            code & 0xFF,
            want & 0xFF,
            "core swap 探针 {label} 应返回 13（含静态数据 5），got {code}（产物：{}）",
            dir.display()
        );
    }
}
