//! 编译确定性门禁（F3 / A1 / M5 / M6）：
//! 同一源码重复编译的产物必须确定（对象文件逐字节稳定）。
//!
//! 背景：`collect_instances`（Stage A）/ `build_plans`（M6 Stage B）从
//! `collect_and_partition_mono_items` 收集实例，其 CGU 顺序/内部遍历受 rustc
//! 影响（历史上 HashMap 布局抖动导致函数布局跨编译漂移）。A1 修复为按
//! mangled 符号名稳定排序——本测试守护该行为。
//!
//! 比对口径：llvm-objdump 反汇编，按"助记符 + 操作数模式"比较
//!（地址、立即数、相对偏移一律归一化），避免链接器 ASLR/重定位差异干扰；
//! 对象文件则直接逐字节比较（`-C save-temps` 保留 forge 写出的 .o）。
//!
//! # M6（Stage B，每 CGU 独立对象文件）口径更新
//! Stage A 单对象：产物指令序列与 `-C codegen-units` 无关（跨 CGU 摊平 +
//! 全局符号序，函数布局恒定）。Stage B 多对象：对象/布局由 **CGU 分区**
//! 决定——不同 `codegen-units` 配置的函数分组/对象数/最终 .text 布局**有意
//! 不同**（每 CGU .text 内仍按符号序、对象按 CGU 名字序——组内布局确定）。
//! 因此确定性矩阵口径调整为：
//! 1. **同配置重复编译**：逐对象字节全等 + 链接产物指令序列全等
//!   （`-C codegen-units=1/4/16` 各两次；cgu=4 另加 `-Z threads=2` +
//!   `FORGE_CODEGEN_THREADS=4` 真并行池——并行与串行的 .o 字节必须全等）；
//! 2. **跨配置**：退出码/运行行为一致（对象布局可不同——不再比较指令序列）。
//! M5 符号稳定测试改为跨全部对象聚合（`__slice_*`/`__vtable_*` 只定义一次，
//! owner CGU 归属确定）。
//!
//! 注意：rustc 分区在**非增量 + 默认 codegen-units** 下会把小 CGU 合并
//!（NON_INCR_MIN_CGU_SIZE，见 rustc_monomorphize::partitioning）——单模块
//! 小 crate 通常只有 1 个 CGU → 单对象路径（Stage A 形态）；本文件的
//! 多对象矩阵用**显式 `-C codegen-units`** + 多模块源码（每模块一个 CGU）
//! 强制多 CGU，守护多对象确定性。

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

/// M6 多对象应力源：多模块（每模块一个 CGU）+ static/static mut + trait
/// dyn vtable + slice/promoted 数据段 + 泛型单态化——在显式
/// `-C codegen-units=N` 下强制多 CGU 分区（rustc 对单模块小 crate 默认合并
/// 成单 CGU）。返回退出码恒 75。
fn m6_src() -> String {
    r#"#![no_std]
#![no_main]
static GLOBAL: i32 = 7;
static mut CNT: i32 = 0;
trait T { fn tv(&self) -> i32; fn ts(&self) -> &'static str; }
mod alpha {
    use super::T;
    pub struct SA(pub i32);
    impl T for SA { fn tv(&self) -> i32 { self.0 } fn ts(&self) -> &'static str { "a" } }
    pub fn fa(x: i32) -> i32 { x.wrapping_mul(3).wrapping_add(1) }
    pub fn fstr() -> &'static str { "alpha-lit" }
}
mod beta {
    use super::T;
    pub struct SB(pub i32);
    impl T for SB { fn tv(&self) -> i32 { self.0 * 2 } fn ts(&self) -> &'static str { "b" } }
    pub fn fb(x: i32) -> i32 { x * x - 4 }
    pub fn fslice() -> &'static [u8] { b"beta-bytes" }
    pub fn generic<X: Copy>(x: X, k: i32) -> i32 { k + 1 }
}
#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> i32 {
    let a = alpha::SA(5);
    let b = beta::SB(6);
    let da: &dyn T = &a;
    let db: &dyn T = &b;
    let mut acc = 0i32;
    let mut i = 0;
    while i < 4 { acc += alpha::fa(i); acc += beta::fb(i); i += 1; }
    unsafe { CNT += 1; }
    acc + GLOBAL + unsafe { CNT } + da.tv() + db.tv()
        + alpha::fstr().len() as i32
        + beta::fslice().len() as i32
        + beta::generic(9u64, 10)
}
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { loop {} }
"#
    .to_string()
}

/// M5（WA-38 残余关闭）符号稳定应力源：多 trait×struct 的 dyn vtable +
/// 大量 str/int-slice 字面量 + 泛型单态化，制造 `-Z threads` 真并行下
/// rustc 全局 AllocId（AtomicU64 首次请求序）的调度竞争——旧 alloc_id
/// 命名（`__slice_alloc{N}` / `__vtable_alloc{N}`）在此源上跨并行运行
/// 漂移（实测 6/6 运行符号表互异）。
fn data_sym_stress_source() -> String {
    let mut s = String::from("#![no_std]\n#![no_main]\n");
    for t in 0..6 {
        s.push_str(&format!(
            "trait T{t} {{ fn m{t}(&self) -> i32; fn tag{t}(&self) -> &'static str; }}\n"
        ));
    }
    for i in 0..6 {
        s.push_str(&format!("struct S{i};\n"));
        for t in 0..6 {
            s.push_str(&format!(
                "impl T{t} for S{i} {{\n  fn m{t}(&self) -> i32 {{ {i} }}\n\
                 \x20 fn tag{t}(&self) -> &'static str {{ \"s{i}-t{t}-tag\" }}\n}}\n"
            ));
        }
    }
    let mut idx = 0;
    for i in 0..6 {
        for t in 0..6 {
            idx += 1;
            s.push_str(&format!(
                "fn f{idx}(s: &S{i}) -> i32 {{\n  let d: &dyn T{t} = s;\n\
                 \x20 let a: &str = \"fn{idx}-lit-{i}-{t}\";\n\
                 \x20 let b: &[u8] = b\"fn{idx}-bytes-{t}-{i}\";\n\
                 \x20 let c: &[i32] = &[{i}, {t}, {idx}];\n\
                 \x20 d.m{t}() + a.len() as i32 + b.len() as i32 + c.len() as i32\n}}\n"
            ));
        }
    }
    s.push_str(
        "fn pick<X: Copy>(x: X, k: i32) -> i32 { let z: &[X] = &[x, x]; z.len() as i32 * k + k }\n",
    );
    s.push_str("#[unsafe(no_mangle)]\npub extern \"C\" fn mainCRTStartup() -> i32 {\n");
    s.push_str("    let mut acc = 0i32;\n");
    let mut idx = 0;
    for i in 0..6 {
        for _t in 0..6 {
            idx += 1;
            s.push_str(&format!("    acc += f{idx}(&S{i});\n"));
        }
    }
    s.push_str(
        "    acc += pick(1u64, 3) + pick(7u32, 5) + pick(2.5f64, 7) + pick(0.5f32, 11);\n    acc\n}\n",
    );
    s.push_str("#[panic_handler]\nfn panic(_info: &core::panic::PanicInfo) -> ! { loop {} }\n");
    s
}

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

/// 目录下 forge 后端产出的对象文件（按文件名升序——对象按 CGU 名序稳定）。
/// 单对象模式 = 1 个 `forge_codegen_output.o`；多对象模式 = 每个 CGU 一个
/// `forge_codegen_output.<cgu 名>.o`。
fn forge_objects(dir: &Path) -> Vec<PathBuf> {
    let mut objs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    let n = p
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    n.starts_with("forge_codegen_output") && n.ends_with(".o")
                })
                .collect()
        })
        .unwrap_or_default();
    objs.sort();
    objs
}

/// 对象文件名 → 字节（同配置重复编译必须逐对象逐字节一致）。
fn forge_object_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    forge_objects(dir)
        .into_iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().to_string();
            std::fs::read(&p).ok().map(|b| (name, b))
        })
        .collect()
}

/// 编译并收集产物：源码（`src`）在独立子目录 `subdir` 内编译
///（`-C save-temps` 保留对象），返回 (对象文件字节, exe 路径)。
///
/// 注意：源码写入 **workdir 根共享路径**（`prog_src.rs`）而非子目录——DWARF
/// 的 CU name/行表 file 条目内嵌**源文件绝对路径**：同配置两次编译若各用
/// 不同子目录的源文件，debug 段字节会因路径不同而"漂移"（非真实非确定）。
/// 所有变体共享同一源文件路径后，debuginfo 变体的逐对象字节比较才成立。
fn compile_to_dir(
    workdir: &Path,
    subdir: &str,
    src: &str,
    extra: &[&str],
    envs: &[(&str, &str)],
) -> Result<(Vec<(String, Vec<u8>)>, PathBuf), String> {
    let run = workdir.join(subdir);
    std::fs::create_dir_all(&run).map_err(|e| e.to_string())?;
    let src_path = workdir.join("prog_src.rs");
    std::fs::write(&src_path, src).map_err(|e| e.to_string())?;
    let exe = run.join("prog.exe");
    let mut cmd = Command::new("rustc");
    cmd.arg("-Zcodegen-backend=".to_string() + &backend_dll().display().to_string())
        .args(["-C", "panic=abort", "-C", "overflow-checks=off", "--edition", "2024"])
        .arg("-C")
        .arg("save-temps=yes")
        .args(extra)
        .arg("-C")
        .arg("link-args=/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib")
        .arg(&src_path)
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
                .take(12)
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    let objs = forge_object_bytes(&run);
    Ok((objs, exe))
}

/// 运行产物并取退出码（panic handler 是 loop{}，超时保护）。
fn run_exit(exe: &Path) -> i32 {
    let mut child = Command::new(exe)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn exe");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().expect("wait exe") {
            return status.code().expect("exe exit code");
        }
        assert!(std::time::Instant::now() < deadline, "exe timeout (挂起)");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// 编译 SRC 到独立子目录并收集对象文件（串行/并行变体共用）。
fn compile(
    workdir: &Path,
    name: &str,
    extra: &[&str],
    envs: &[(&str, &str)],
) -> Result<PathBuf, String> {
    compile_to_dir(workdir, name, SRC, extra, envs).map(|(_, exe)| exe)
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
            "instruction sequence differs at #{i} between {label_a} and {label_b} — determinism broken"
        );
    }
}

/// 对象字节表全等断言（同名对象必须逐字节一致）。
fn assert_objects_eq(
    label_a: &str,
    a: &[(String, Vec<u8>)],
    label_b: &str,
    b: &[(String, Vec<u8>)],
) {
    let names_a: Vec<&str> = a.iter().map(|(n, _)| n.as_str()).collect();
    let names_b: Vec<&str> = b.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names_a,
        names_b,
        "object file sets differ between {label_a} ({}) and {label_b} ({}) — CGU 分区/对象命名不稳定",
        names_a.len(),
        names_b.len()
    );
    for ((na, ba), (nb, bb)) in a.iter().zip(b.iter()) {
        assert_eq!(
            ba, bb,
            "object '{na}' bytes differ between {label_a} and {label_b} — 多对象逐字节确定被破坏"
        );
        let _ = nb;
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

/// M6（Stage B）多对象确定性矩阵。
///
/// 应力源 = 多模块（rustc 按模块生成 CGU；小 crate 默认合并成单 CGU 的
/// 情况下用显式 `-C codegen-units` 强制多分区）。
/// 矩阵：
/// - cgu=1 ×2（单 CGU → 单对象，Stage A 回归锚）
/// - cgu=4 ×2（多对象）
/// - cgu=16 ×2（多对象——CGU 数由模块数定，≥2）
/// - cgu=4 + `-Z threads=2` + FORGE_CODEGEN_THREADS=4（真并行池，M4 机制
///   在多对象下——函数任务跨 CGU 上池并行）
/// - cgu=4 + debuginfo=1 ×2、cgu=4 + debuginfo=2 ×2（B-v2 per-CGU CU：
///   debuginfo 与多对象并存——每个带函数的对象各有独立 DWARF CU，
///   逐对象字节稳定）
/// 断言：
/// 1. 同配置两次编译：对象文件**逐字节全等** + 链接产物指令序列全等；
/// 2. 并行与串行（cgu=4）：对象字节全等（par_map 保序 + 归并确定）；
/// 3. 跨配置：退出码恒等（运行行为一致；.o 布局/数量由 CGU 分区定——
///    不再要求指令序列跨配置全等，见文件头口径说明）。
#[test]
fn deterministic_multi_object_bytes_across_runs_and_modes() {
    let workdir = std::env::temp_dir().join(format!("forge_det_m6_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create workdir");
    let src = m6_src();
    let variants: [(&str, &[&str], &[(&str, &str)]); 11] = [
        ("cgu1_a", &["-C", "codegen-units=1"], &[]),
        ("cgu1_b", &["-C", "codegen-units=1"], &[]),
        ("cgu4_a", &["-C", "codegen-units=4"], &[]),
        ("cgu4_b", &["-C", "codegen-units=4"], &[]),
        // M4 真并行池：rustc -Z threads=2 + forge 函数任务 par_map 上池
        (
            "cgu4_par",
            &["-C", "codegen-units=4", "-Z", "threads=2"],
            &[("FORGE_CODEGEN_THREADS", "4")],
        ),
        ("cgu16_a", &["-C", "codegen-units=16"], &[]),
        ("cgu16_b", &["-C", "codegen-units=16"], &[]),
        // B-v2：debuginfo 与多对象并存（per-CGU CU）——逐对象字节稳定
        (
            "cgu4_dbg1_a",
            &["-C", "codegen-units=4", "-C", "debuginfo=1"],
            &[],
        ),
        (
            "cgu4_dbg1_b",
            &["-C", "codegen-units=4", "-C", "debuginfo=1"],
            &[],
        ),
        (
            "cgu4_dbg2_a",
            &["-C", "codegen-units=4", "-C", "debuginfo=2"],
            &[],
        ),
        (
            "cgu4_dbg2_b",
            &["-C", "codegen-units=4", "-C", "debuginfo=2"],
            &[],
        ),
    ];
    let mut outs: Vec<(String, Vec<(String, Vec<u8>)>, Vec<String>, i32)> = Vec::new();
    for (name, extra, envs) in variants {
        let (objs, exe) = compile_to_dir(&workdir, name, &src, extra, envs)
            .map_err(|e| format!("{name}: {e}"))
            .expect("compile variant");
        assert!(!objs.is_empty(), "{name}: no forge objects produced");
        let exit = run_exit(&exe);
        outs.push((name.to_string(), objs, inst_seq(&exe), exit));
    }
    let _ = std::fs::remove_dir_all(&workdir);

    // 跨配置：退出码恒等（单对象/多对象、debuginfo 开/关布局可不同——行为必须一致）
    for (name, _, _, exit) in &outs {
        assert_eq!(
            *exit, 75,
            "{name}: 跨配置行为漂移——exit={exit} ≠ 75（Stage B 不得改变语义）"
        );
    }
    // cgu1 单对象锚：1 个对象（Stage A 形态）
    assert_eq!(outs[0].1.len(), 1, "cgu=1 应为单对象");
    // 多对象锚：cgu4/cgu16/debuginfo 变体至少 2 个对象（多模块 → 多 CGU）
    for idx in [2usize, 5, 7, 9] {
        assert!(
            outs[idx].1.len() >= 2,
            "{} 应为多对象（多模块源），实际 {}",
            outs[idx].0,
            outs[idx].1.len()
        );
    }

    // 同配置重复：逐对象字节全等 + 链接产物指令序列全等
    for (a_idx, b_idx) in [(0usize, 1usize), (2, 3), (5, 6), (7, 8), (9, 10)] {
        let (na, oa, sa, _) = &outs[a_idx];
        let (nb, ob, sb, _) = &outs[b_idx];
        assert_objects_eq(na, oa, nb, ob);
        assert_seq_eq(na, sa, nb, sb);
    }
    // 并行（cgu4_par）与串行（cgu4_a）：对象字节全等
    assert_objects_eq("cgu4_serial", &outs[2].1, "cgu4_par_map", &outs[4].1);
    assert_seq_eq("cgu4_serial", &outs[2].2, "cgu4_par_map", &outs[4].2);

    println!(
        "PASS  deterministic_multi_object objects(cgu1)={} objects(cgu4)={} objects(cgu16)={} \
         objects(cgu4_dbg1)={} objects(cgu4_dbg2)={} variants=11 exit=75 逐对象字节全等（含 -Z threads 真并行 + per-CGU dwarf）",
        outs[0].1.len(),
        outs[2].1.len(),
        outs[5].1.len(),
        outs[7].1.len(),
        outs[9].1.len()
    );
}

/// 编译到独立子目录并收集对象文件中内部数据符号名（`__slice_*`/`__vtable_*`，
/// M5 内容稳定键）。`-C save-temps` 保留对象；Stage B 多对象下聚合全部
/// `forge_codegen_output*.o`（每个数据符号只由 owner CGU 定义一次）。
fn compile_and_data_syms(
    workdir: &Path,
    run_dir: &str,
    src_name: &str,
    extra: &[&str],
    envs: &[(&str, &str)],
) -> Result<Vec<String>, String> {
    let run = workdir.join(run_dir);
    std::fs::create_dir_all(&run).map_err(|e| e.to_string())?;
    let src = workdir.join(format!("{src_name}.rs"));
    let exe = run.join(format!("{src_name}.exe"));
    let mut cmd = Command::new("rustc");
    cmd.arg("-Zcodegen-backend=".to_string() + &backend_dll().display().to_string())
        .args(["-C", "panic=abort", "-C", "overflow-checks=off", "--edition", "2024"])
        .arg("-C")
        .arg("save-temps=yes")
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
    let nm = std::env::var("FORGE_E2E_NM").unwrap_or_else(|_| "llvm-nm".to_string());
    let objs = forge_objects(&run);
    if objs.is_empty() {
        return Err(format!("object not found in: {}", run.display()));
    }
    let mut names: Vec<String> = Vec::new();
    for obj in &objs {
        let out = Command::new(&nm)
            .arg(obj)
            .output()
            .map_err(|e| format!("llvm-nm spawn: {e}"))?;
        names.extend(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| l.contains("__slice_") || l.contains("__vtable_"))
                .filter_map(|l| l.split_whitespace().next_back().map(|s| s.to_string())),
        );
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// M5（WA-38 残余关闭）：内部数据符号名必须**跨运行与调度模式稳定**。
///
/// 旧 alloc_id 命名（`__slice_alloc{N}`/`__vtable_alloc{N}`）的 N 来自
/// rustc 全局 AllocId `AtomicU64`（首次请求序）——-Z threads 真并行
/// （FORGE_CODEGEN_THREADS>1 上 rustc 查询池）下调度序不定，同一内容
/// 跨运行符号名数值漂移。M5 改为内容稳定键（64 位 FNV-1a）。M6 起数据
/// 按 owner CGU 归属跨对象分布——本测试聚合全部对象符号名（跨串/并行、
/// 跨运行逐字一致）。
#[test]
fn stable_data_symbol_names_across_scheduling() {
    let workdir = std::env::temp_dir().join(format!("forge_det_sym_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create workdir");
    std::fs::write(workdir.join("sym.rs"), data_sym_stress_source()).expect("write source");

    let variants: [(&str, &[&str], &[(&str, &str)]); 4] = [
        ("serial", &[], &[]),
        (
            "par_a",
            &["-Z", "threads=4"],
            &[("FORGE_CODEGEN_THREADS", "4")],
        ),
        (
            "par_b",
            &["-Z", "threads=4"],
            &[("FORGE_CODEGEN_THREADS", "4")],
        ),
        (
            "par_c",
            &["-Z", "threads=4"],
            &[("FORGE_CODEGEN_THREADS", "4")],
        ),
    ];
    let mut syms: Vec<(String, Vec<String>)> = Vec::new();
    for (name, extra, envs) in variants {
        let mut names = compile_and_data_syms(&workdir, name, "sym", extra, envs)
            .map_err(|e| format!("{name}: {e}"))
            .expect("compile variant");
        assert!(
            names.len() >= 20,
            "{name}: only {} __slice/__vtable symbols — 应力源未触发数据段",
            names.len()
        );
        names.sort();
        syms.push((name.to_string(), names));
    }
    let _ = std::fs::remove_dir_all(&workdir);

    let (base_name, base) = &syms[0];
    for (name, names) in &syms[1..] {
        assert_eq!(
            base, names,
            "internal data symbol names differ between {base_name} and {name} — \
             M5 内容稳定键被破坏（WA-38 残余回归：alloc_id 调度序重新泄漏进符号名）"
        );
    }
    println!(
        "PASS  stable_data_symbol_names __slice/__vtable symbols={} variants=4 (serial + -Z threads x3)",
        base.len()
    );
}
