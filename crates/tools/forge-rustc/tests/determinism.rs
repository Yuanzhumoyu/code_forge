//! 编译确定性门禁（F3 / A1）：同一源码编译两次，剥离地址后的指令序列
//! 必须一致。
//!
//! 背景：`collect_instances` 从 `collect_and_partition_mono_items` 收集实例，
//! 其 CGU 顺序/内部遍历受 rustc 影响（历史上 HashMap 布局抖动导致函数
//! 布局跨编译漂移，干扰回归比对与可复现调试）。A1 修复为按 mangled
//! 符号名稳定排序——本测试守护该行为。
//!
//! 比对口径：llvm-objdump 反汇编，按"助记符 + 操作数模式"比较
//! （地址、立即数、相对偏移一律归一化），避免链接器 ASLR/重定位差异
//! 干扰。用例刻意包含多函数 + 聚合 + alloc（Vec/String）放大排序影响。
//!
//! M3/M4（并行 CGU Stage A，-C codegen-units）：产物（剥离地址后指令序列）
//! 必须与 `-C codegen-units` 及 `FORGE_CODEGEN_THREADS` 无关——任务化
//! 编译 + 单对象按符号序归并不得改变函数布局（验收核心：行为不变优先）。
//! **M4（WA-38 根治）**：rustc 1.99+ WorkerLocal 查询引擎下 tcx 查询只能
//! 在 rustc 自建池线程执行（std::thread 死路）——真并行改为把函数降级
//! 任务以 `rustc_data_structures::sync::par_map` 提交 **rustc 查询池**
//! （门控 = `FORGE_CODEGEN_THREADS>1 && -Z threads>=2`，即
//! `jobs.frontend.is_some()`）；无 -Z threads 时走显式串行 map，行为与
//! T=1 恒等。本矩阵：cgu=1/2/4 × THREADS=1/4（无 -Z threads，串行路径
//! 产物一致）+ **cgu=1/4 × -Z threads=2 × THREADS=4**（真并行池路径——
//! rustc 前端并行 + forge 函数任务上池并行，指令序列必须与串行一致）。

use std::path::{Path, PathBuf};
use std::process::Command;

const SRC: &str = r#"#![no_std]
#![no_main]
extern crate alloc;
use core::alloc::{GlobalAlloc, Layout};
static mut HEAP: [u8; 8192] = [0; 8192];
struct A;
unsafe impl GlobalAlloc for A {
    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe { core::ptr::addr_of_mut!(HEAP) as *mut u8 } }
    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}
}
#[global_allocator]
static ALLOC: A = A;
#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> i32 {
    let mut v = alloc::vec::Vec::new(); v.push(1); v.push(2); v.push(3);
    let s = alloc::string::String::from("hi");
    let mut acc = 0i32; let mut i = 0; while i < 4 { acc += i; i += 1; }
    v.len() as i32 + s.len() as i32 + acc
}
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { loop {} }
"#;

/// 定位 backend dll（cargo test 构建产物）。
fn backend_dll() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let dll = Path::new(manifest)
        .join("..")
        .join("..")
        .join("..")
        .join("target")
        .join("debug")
        .join("forge_rustc.dll");
    assert!(
        dll.exists(),
        "backend dll not found: {} — run `cargo build -p forge-rustc` first",
        dll.display()
    );
    dll
}

fn compile(
    workdir: &Path,
    name: &str,
    extra: &[&str],
    envs: &[(&str, &str)],
) -> Result<PathBuf, String> {
    let src = workdir.join(format!("{name}.rs"));
    let exe = workdir.join(format!("{name}.exe"));
    std::fs::write(&src, SRC).map_err(|e| e.to_string())?;
    let mut cmd = Command::new("rustc");
    cmd.arg("-Zcodegen-backend=".to_string() + &backend_dll().display().to_string())
        .args(["-C", "panic=abort", "-C", "overflow-checks=off", "--edition", "2024"])
        .args(extra)
        .arg("-C")
        .arg("link-args=/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .envs(envs.iter().copied());
    let out = cmd
        .output()
        .map_err(|e| format!("failed to spawn rustc: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "compile failed: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .take(10)
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Ok(exe)
}

/// 判断 token 是否为十六进制字节列（"48"、"89"、"e5" 等 2 位十六进制）。
fn is_hex_bytes(t: &str) -> bool {
    t.len() == 2 && t.chars().all(|c| c.is_ascii_hexdigit())
}

/// 归一化单条反汇编行：剥离地址、字节列、立即数常量、相对偏移。
fn normalize(line: &str) -> Option<String> {
    // 排除文件头/段头/标签/空行（指令行必含冒号且非文件头）
    if line.contains('<')
        || line.contains("file format")
        || line.contains("Disassembly of section")
        || !line.contains(':')
    {
        return None;
    }
    // 冒号后 = 字节列 + 助记符 + 操作数
    let rest = line.split(':').nth(1)?;
    let inst: Vec<&str> = rest
        .split_whitespace()
        .filter(|t| !is_hex_bytes(t)) // 跳过 2 位十六进制字节列
        .collect();
    if inst.is_empty() {
        return None;
    }
    let norm = inst
        .join(" ")
        .replace("$0x", "$imm") // 立即数（含地址类 imm）
        .replace("0x", "addr") // 相对偏移/符号地址
        .replace("addrffffffffffffff", "addr"); // -0x... 归一
    Some(norm)
}

/// 提取反汇编指令序列：`mnemonic + 操作数模式`（地址/立即数/偏移归一化）。
fn inst_seq(exe: &Path) -> Vec<String> {
    let objdump = std::env::var("FORGE_E2E_OBJDUMP").unwrap_or_else(|_| "llvm-objdump".to_string());
    let out = Command::new(&objdump)
        .arg("-d")
        .arg(exe)
        .output()
        .expect("objdump spawn");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(normalize)
        .collect()
}

fn assert_seq_eq(label_a: &str, a: &[String], label_b: &str, b: &[String]) {
    assert_eq!(
        a.len(),
        b.len(),
        "instruction count differs between {label_a} and {label_b} (A={}, B={}) — determinism broken",
        a.len(),
        b.len()
    );
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(
            x, y,
            "instruction sequence differs at #{i} between {label_a} and {label_b} — determinism broken (M3 worker 分块/归并不得改变产物)"
        );
    }
}

#[test]
fn deterministic_output_across_runs() {
    let workdir = std::env::temp_dir().join(format!("forge_det_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create workdir");
    let exe_a = compile(&workdir, "det_a", &[], &[]).expect("compile a");
    let exe_b = compile(&workdir, "det_b", &[], &[]).expect("compile b");
    let seq_a = inst_seq(&exe_a);
    let seq_b = inst_seq(&exe_b);
    let _ = std::fs::remove_dir_all(&workdir);

    assert!(!seq_a.is_empty(), "no instructions extracted from A");
    assert_seq_eq("compile A", &seq_a, "compile B", &seq_b);
    println!("PASS  determinism inst_seq_len={}", seq_a.len());
}

/// M3/M4（并行 CGU Stage A，-C codegen-units=N）：产物（剥离地址后的指令
/// 序列）必须与 `-C codegen-units` 及 `FORGE_CODEGEN_THREADS` 无关——
/// 任务化编译 + 单对象按符号序归并不得改变函数布局（验收核心：行为不变
/// 优先）。矩阵：cgu=1/2/4（T=1 串行 vs env 显式并行）+ THREADS=1 逃生口
/// 对照 + cgu=1 下 THREADS=4 对照 + **M4 真并行池维**：
/// `-Z threads=2`（rustc 前端并行池 + `jobs.frontend.is_some()` 门控）
/// + `FORGE_CODEGEN_THREADS=4`（forge 函数任务 par_map 上池并行）。
#[test]
fn deterministic_across_codegen_units_and_threads() {
    let workdir = std::env::temp_dir().join(format!("forge_det_m3_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create workdir");
    let variants: [(&str, &[&str], &[(&str, &str)]); 7] = [
        ("cgu1", &["-C", "codegen-units=1"], &[]),
        ("cgu2", &["-C", "codegen-units=2"], &[]),
        ("cgu4", &["-C", "codegen-units=4"], &[]),
        // 逃生口：FORGE_CODEGEN_THREADS=1 强制串行（即使 cgu>1）
        (
            "cgu4_t1",
            &["-C", "codegen-units=4"],
            &[("FORGE_CODEGEN_THREADS", "1")],
        ),
        // cgu=1 时 THREADS=4：无 -Z threads → 门控不满足，仍串行（与 cgu1
        // 同产物；仅 stderr 提示"需 -Z threads"）
        (
            "cgu1_t4",
            &["-C", "codegen-units=1"],
            &[("FORGE_CODEGEN_THREADS", "4")],
        ),
        // M4 真并行池：rustc -Z threads=2（前端并行 + jobs.frontend=Some）
        // + FORGE_CODEGEN_THREADS=4 → forge 函数任务 par_map 上池并行。
        (
            "cgu4_zt2_t4",
            &["-C", "codegen-units=4", "-Z", "threads=2"],
            &[("FORGE_CODEGEN_THREADS", "4")],
        ),
        (
            "cgu1_zt2_t4",
            &["-C", "codegen-units=1", "-Z", "threads=2"],
            &[("FORGE_CODEGEN_THREADS", "4")],
        ),
    ];
    let mut seqs: Vec<(String, Vec<String>)> = Vec::new();
    for (name, extra, envs) in variants {
        let exe = compile(&workdir, name, extra, envs)
            .map_err(|e| format!("{name}: {e}"))
            .expect("compile variant");
        let seq = inst_seq(&exe);
        assert!(!seq.is_empty(), "no instructions extracted from {name}");
        seqs.push((name.to_string(), seq));
    }
    let _ = std::fs::remove_dir_all(&workdir);

    let (base_name, base) = &seqs[0];
    for (name, seq) in &seqs[1..] {
        assert_seq_eq(base_name, base, name, seq);
    }
    println!(
        "PASS  determinism_across_codegen_units inst_seq_len={} variants=7 (incl. -Z threads 真并行池 x2)",
        base.len()
    );
}
