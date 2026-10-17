//! 端到端真值测试：用 forge-rustc 作为 rustc codegen backend 编译 no_std 程序，
//! 运行产物并校验退出码（入口函数返回值 = 进程退出码）。
//!
//! 这是本 crate 的**统一测试体系**（P4.5）：
//! - 用例清单即 `CASES` 数组（103 项，含 `known_failure` + `reason` 回归探针）
//! - 旧的 `stage_a.rs` / `run_tests.sh` / `test_runner.sh` 已并入本文件并删除
//! - `rustc_integration_test.ps1` 为简化版 PowerShell 入口（11 个标量用例子集；README 引用）
//! - 未支持清单（global_asm 等）见 WORKAROUNDS.md（[WA-NN] 编号）
//!
//! 前置条件：
//! - nightly 工具链（rust-toolchain.toml 已指定）+ rustc-dev 组件
//! - 先构建 backend：`cargo build -p forge-rustc`
//!
//! 运行：`cargo test -p forge-rustc --test e2e`
//!
//! 说明：每个用例都是独立程序，`mainCRTStartup` 的返回值作为进程退出码。
//! `known_failure = true` 的用例是当前已知限制的显式清单（对应 Phase 2/3 计划），
//! 实现完成后应移除标记使其成为硬断言；`reason` 记录失败模式（SEGV/exit 码），
//! 转正时对照验证（P3.3 回归探针）。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 钉版 nightly 工具链（与 `.github/workflows/ci.yml` 的
/// `toolchain: nightly-2026-09-05` 单点同步；bump 时两处一起改）。
/// `cargo +nightly` 显式 channel 不受 rustup override 影响，必须用钉版
/// 字面量保证 CI/本地一致（rustc_compat.rs 适配层按此版本维护；
/// 1.100 起 `LangItem` 移至 `rustc_hir::attrs::lang_items`，1.99 无法编译。
/// ⚠️ 日期 channel 的 rustc 提交是发布前一日：nightly-2026-09-04 =
/// a69a63265（2026-09-03），**与本机 nightly-2026-09-05 = 0ed41eb41
/// （2026-09-04）不同**——本地验证必须在 09-05 上。务必与本地 rustup 当前 nightly
/// 对齐再钉版！（历史注：曾把 vec 用例的 CI-only AV 归因于该 channel 差异；
/// 2026-09-12 已定性为 **WA-42**（niche tag 写入宽度）——与工具链版本无关，
/// 差异来自 runner 栈残留内容，见计划 §9.7。）
const NIGHTLY: &str = "nightly-2026-09-05";

/// 本机工具链选择：CI 用钉版 NIGHTLY；本机可用 `FORGE_E2E_NIGHTLY`
/// 环境变量覆盖（如 `nightly`——浮动 channel 指向已安装目录，避免
/// 版本化目录缺失/rustup 安装权限限制；本机 .rustup 被 Defender 拦截
/// 时用 `RUSTUP_HOME=target\rustup_home` junction 重定向）。
fn e2e_toolchain() -> String {
    std::env::var("FORGE_E2E_NIGHTLY").unwrap_or_else(|_| NIGHTLY.to_string())
}

/// 用例：程序体（i32 表达式）+ 期望退出码。
/// e2e 模板的 panic 出口码：panic handler 调 `ExitProcess(101)`（Windows 上 Rust 的
/// 约定 abort 码）。**v27 M2 起不再用 `asm!("ud2")`**——内联 asm 在该里程碑被改为
/// **fail-closed**（`[cap:INLINE_ASM]`），而 e2e 模板自己也用 `asm!` 的话，全套用例
/// 会立刻编不过（实测确实如此）。`ExitProcess` 走 FFI（e2e 已链 kernel32）⇒ 无需 asm。
const PANIC_ABORT_CODE: u32 = 101;

/// `expect_trap` 用例的期望码 = panic abort 码（见 [`PANIC_ABORT_CODE`]）。
///
/// 语义：**断言失败边真的分流出去了**（溢出被拦）⇒ 进程以 `101` 退出；
/// 静默回绕会正常退出回绕值（如 `i32::MIN`）⇒ 必红；断言没触发而挂起 ⇒ 超时 ⇒ 也必红。
/// **不要**把期望码写成 `i32::MIN`：`i32::MAX + 1` 的回绕值恰好是它（v27 M1 踩过）。
const TRAP_EXIT: i32 = PANIC_ABORT_CODE as i32;

/// e2e 用例。
struct Case {
    name: &'static str,
    /// 入口函数体（可含辅助 fn 定义）。
    body: &'static str,
    /// 额外 crate 级代码（如 #[link(name = "...")]），插到 #![no_main] 之后。
    extra: &'static str,
    /// 入口符号名："mainCRTStartup"（默认）或 "main"（fn main 形态）。
    entry: &'static str,
    expected: i32,
    /// 期望编译失败（负向用例，A4/B3 门控）：编译必须报错而非产出
    /// 错误结果 exe。断言 stderr 含 `expect_compile_err` 关键词（为空
    /// 则只断言编译失败）。
    expect_compile_fail: bool,
    expect_compile_err: &'static str,
    /// **必须异常终止**（trap 语义，v27 M1）：溢出/越界等 `Assert` 失败边在真机上
    /// 表现为进程**异常终止**（Windows 上 `status.code()` 为 `None`）⇒ 本标记为真时，
    /// "异常终止"算 PASS、"正常退出"算 FAIL。退出码用 [`TRAP_EXIT`] 表达。
    expect_trap: bool,
    /// 已知失败（当前后端限制），仅记录不失败。
    known_failure: bool,
    /// 已知失败的阶段归属（Phase 2 = 调用语义）。
    phase: &'static str,
    /// 已知失败的具体模式（SEGV/exit 码/现象），转正时对照验证（P3.3 回归探针）。
    reason: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "ret_const",
        body: "42",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1",
        reason: "",
    },
    Case {
        name: "add",
        body: "let a = 1i32; let b = 2i32; a + b",
        expected: 3,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1",
        reason: "",
    },
    Case {
        name: "sub",
        body: "5i32 - 3",
        expected: 2,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1",
        reason: "",
    },
    Case {
        name: "mul",
        body: "3i32 * 4",
        expected: 12,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1",
        reason: "",
    },
    Case {
        name: "div",
        body: "10i32 / 3",
        expected: 3,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1",
        reason: "",
    },
    Case {
        name: "rem",
        body: "10i32 % 3",
        expected: 1,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1",
        reason: "",
    },
    Case {
        name: "conditional",
        body: "let x = 5i32; if x > 0 { 1 } else { -1 }",
        expected: 1,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1",
        reason: "",
    },
    Case {
        name: "simple_loop",
        body: "let mut acc = 0i32; let mut i = 0i32; while i < 5 { acc += i; i += 1; } acc",
        expected: 10,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1",
        reason: "",
    },
    Case {
        name: "loop_break",
        body: "let mut acc = 0i32; loop { acc += 1; if acc >= 3 { break; } } acc",
        expected: 3,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1",
        reason: "",
    },
    Case {
        name: "bool_ops",
        body: "let a = true; let b = true; if (a && b) || (!a && !b) { 1 } else { 0 }",
        expected: 1,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1",
        reason: "",
    },
    Case {
        name: "early_return",
        body: "let x = 5i32; if x < 0 { return 0; } x * 2",
        expected: 10,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1",
        reason: "",
    },
    // ── 函数调用语义（Phase 2 已实现：直接调用 + 符号映射 + COFF addend）──
    Case {
        name: "i64_wrapping_add",
        body: "let r = 1000i64.wrapping_add(2000i64); r as i32",
        expected: 3000,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P2",
        reason: "",
    },
    Case {
        name: "fib_recursive",
        body: "fn fib(n: i32) -> i32 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } } fib(10)",
        expected: 55,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P2",
        reason: "",
    },
    Case {
        name: "multi_call_chain",
        body: "fn add2(a: i32, b: i32) -> i32 { a + b } let x = add2(3, 4); let y = add2(x, 5); y",
        expected: 12,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P2",
        reason: "",
    },
    Case {
        name: "nested_calls",
        body: "fn mul(a: i32, b: i32) -> i32 { a * b } mul(mul(2, 3), mul(4, 5))",
        expected: 120,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P2",
        reason: "",
    },
    // ── Windows x64 调用约定（Phase 2）：>4 整数参数走栈、浮点 XMM──
    Case {
        name: "five_args_stack",
        body: "fn f(a: i32, b: i32, c: i32, d: i32, e: i32) -> i32 { a + b + c + d + e } f(1, 2, 3, 4, 5)",
        expected: 15,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P2 abi",
        reason: "",
    },
    Case {
        name: "eight_args_stack",
        body: "fn f(a: i32, b: i32, c: i32, d: i32, e: i32, ff: i32, g: i32, h: i32) -> i32 { a + b + c + d + e + ff + g + h } f(1, 2, 3, 4, 5, 6, 7, 8)",
        expected: 36,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P2 abi",
        reason: "",
    },
    Case {
        name: "float_args",
        body: "fn f(a: f64, b: f64) -> i32 { if a + b > 3.0 { 1 } else { 0 } } f(1.5, 2.0)",
        expected: 1,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        // 已修复（P5.1 验证）：`if a + b > 3.0 { 1 } else { 0 }` 最小用例
        // f(1.5, 2.0) 实测 exit=1 通过——浮点比较/分支链在 Fload/Fstore +
        // fcmp is_fp 分派修复后已正确（比较结果经 Setcc 后分支返回 1）。
        // 2026-08 复验：最小复现 exit=1，known_failure 移除。
        expect_trap: false,
        known_failure: false,
        phase: "P5.1",
        reason: "",
    },
    Case {
        name: "float_return",
        body: "fn f(a: f64, b: f64) -> f64 { a * b } let r = f(3.0, 2.5); if r > 7.0 { 1 } else { 0 }",
        expected: 1,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P5.1",
        reason: "",
    },
    // ── FFI 外部符号（Phase 2）：extern "C" → UNDEF 重定位 + 系统库链接 ──
    Case {
        name: "ffi_exit_process",
        body: "unsafe { ExitProcess(42) }",
        extra: "#[link(name = \"kernel32\")]\nunsafe extern \"C\" { fn ExitProcess(code: u32) -> !; }",
        expected: 42,
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P2 ffi",
        reason: "",
    },
    // ── 聚合类型（Phase 3）：引用/结构体/元组/数组 ──
    Case {
        name: "ref_deref",
        body: "let x = 5i32; let p = &x; *p",
        expected: 5,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P3 agg",
        reason: "",
    },
    Case {
        name: "ref_mut_write",
        body: "let mut x = 5i32; let p = &mut x; *p = 7; x",
        expected: 7,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P3 agg",
        reason: "",
    },
    Case {
        name: "struct_field",
        body: "struct S { a: i32, b: i32 } let s = S { a: 1, b: 2 }; s.a + s.b",
        expected: 3,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P3 agg",
        reason: "",
    },
    Case {
        name: "struct_mut_field",
        body: "struct S { a: i32, b: i32 } let mut s = S { a: 1, b: 2 }; s.a = 10; s.a + s.b",
        expected: 12,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P3 agg",
        reason: "",
    },
    Case {
        name: "tuple_field",
        body: "let t = (1i32, 2i32, 3i32); t.0 + t.1 + t.2",
        expected: 6,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P3 agg",
        reason: "",
    },
    Case {
        name: "nested_struct",
        body: "struct P { x: i32, y: i32 } struct R { p: P, z: i32 } let r = R { p: P { x: 1, y: 2 }, z: 3 }; r.p.x + r.p.y + r.z",
        expected: 6,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P3 agg",
        reason: "",
    },
    Case {
        name: "array_single_index",
        body: "let a = [1i32, 2, 3, 4]; let i = 1usize; a[i]",
        expected: 2,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P3 regalloc",
        reason: "",
    },
    // ── 数组多索引（Phase 5.1 修复：Assert 必须跳转 MIR success target 块，
    //    否则 target 块孤立、局部槽未写 → 0）──
    Case {
        name: "array_multi_index",
        body: "let a = [1i32, 2, 3, 4]; let i = 1usize; a[0] + a[i]",
        expected: 3,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P5.1",
        reason: "",
    },
    // ── 跨 crate / checked 元组（Phase 5.2）──
    Case {
        name: "checked_tuple",
        body: "let (v, _o) = 100i32.overflowing_add(200); v",
        expected: 300,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P5.2",
        reason: "",
    },
    Case {
        name: "multi_match_call",
        body: "fn f(x: i32) -> i32 { match x { 0 => 10, 1 => 20, 2 => 30, _ => 99 } } f(7)",
        expected: 99,
        extra: "",
        // Phase 5.1 翻转：返回值 RAX 传递方向修复（[lower_term.Return] movr8）
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P5.1",
        reason: "",
    },
    // 同一函数内多次索引链/多次调用：静态正确但运行时第二个链结果被破坏
    //（regalloc 深层运行时问题，P5 遗留）
    Case {
        name: "multi_call_same_fn",
        body: "fn f(x: i32) -> i32 { match x { 0 => 10, 1 => 20, 2 => 30, _ => 99 } } f(1) + f(2) + f(7)",
        expected: 149,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P5.1",
        reason: "",
    },
    // ── 枚举（Phase 5.3）：构造 + 判别 + 单 payload + if let ──
    Case {
        name: "enum_basic",
        body: "enum E { A, B, C } let e = E::B; match e { E::A => 1, E::B => 2, E::C => 3 }",
        expected: 2,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P5.3",
        reason: "",
    },
    Case {
        name: "enum_payload",
        body: "enum E { A(i32), B } let e = E::A(42); match e { E::A(x) => x, E::B => 0 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P5.3",
        reason: "",
    },
    Case {
        name: "option_match",
        body: "let o = Some(7i32); match o { Some(x) => x, None => 0 }",
        expected: 7,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P5.3",
        reason: "",
    },
    Case {
        name: "option_none",
        body: "let n: Option<i32> = None; match n { Some(x) => x, None => 9 }",
        expected: 9,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P5.3",
        reason: "",
    },
    Case {
        name: "if_let",
        body: "let o = Some(5i32); let mut v = 0; if let Some(x) = o { v = x * 2; } v",
        expected: 10,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P5.3",
        reason: "",
    },
    // ── 已知限制：整数+浮点混合参数（regalloc 常量 XReg 冲突：a=1 与 b=0.5
    //    的 XReg 被分配同一寄存器 R15，属 forge-codegen regalloc 深层问题）──
    Case {
        name: "mixed_args",
        body: "fn f(a: i32, b: f64, c: i32, d: f64, e: i32) -> i32 { a + c + e } f(1, 0.5, 2, 0.5, 3)",
        expected: 6,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P5.1",
        reason: "",
    },
    // ── alloc 运行时（Phase 6）：__rust_alloc 真实堆分配（VirtualAlloc）+ 裸指针写读 ──
    // CRT mem shim（memcpy/memset/memmove/memcmp/strlen）由后端注入，链接需
    // /DEFAULTLIB:vcruntime.lib（__CxxFrameHandler3）。Box 封装层（Unique/
    // NonNull 聚合）在检查链修复后读回/写回均正确（box_value/box_write）。
    Case {
        name: "alloc_runtime_alloc_write",
        body: "unsafe { let p = __rust_alloc(64, 8); *p = 42u8; *p as i32 }",
        expected: 42,
        extra: "unsafe extern \"C\" { fn __rust_alloc(size: usize, align: usize) -> *mut u8; }",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P6 alloc",
        reason: "",
    },
    Case {
        name: "alloc_runtime_memcpy_shim",
        body: "unsafe { let mut dst: i64 = 0; let src: i64 = 0x00000000006968; memcpy(&mut dst as *mut i64 as *mut u8, &src as *const i64 as *const u8, 2); (dst & 0xff) as i32 }",
        expected: 0x68,
        extra: "unsafe extern \"C\" { fn memcpy(d: *mut u8, s: *const u8, n: usize) -> *mut u8; }",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P6 alloc",
        reason: "",
    },
    // write_bytes 循环路径（P4.4 后补的回归探针）：
    // [WA-14] 已修两层：iconst 插块顺序（曾读未初始化寄存器）+ 块参数
    // 传参寄存器一致性（map_terminator 复用 arg 映射 + pre_allocate 顺序，
    // body 的 addr 计算已正确 `dst+bi`）。2026-08 复验：窄类型宽度修复
    // （主库 Load/Store 用真实内存宽度 mem_opsize——u8 读/写 1 字节不再越界
    // 4 字节 + forge-rustc IntToInt cast 对 u8/u16 无符号源零扩展 mask）
    // 后转正——exit=342。
    Case {
        name: "write_bytes_loop",
        body: "unsafe { let mut buf = [0u8; 8]; core::ptr::write_bytes(buf.as_mut_ptr(), 0xAB, 8); (buf[0] as i32) + (buf[7] as i32) }",
        expected: 0xAB + 0xAB,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P7 intrinsics",
        reason: "",
    },
    // ── Box 封装层（Phase 6）：Unique/NonNull 聚合 + ptr::write + 解引用读回；
    //    依赖 #[global_allocator]（前端语义要求，后端注入 __rust_alloc 无法替代）。──
    Case {
        name: "box_value",
        body: "let b = Box::new(42i32); *b",
        expected: 42,
        extra: "extern crate alloc;\nuse alloc::boxed::Box;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 4096] = [0; 4096];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        base.add(base.align_offset(32))\n    } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P6 alloc",
        reason: "2026-09-12 转正（WA-42 根治：niche 枚举 tag 写入宽度 = tag 标量）——CI run 93927221004
        的 AMD Family 25 Model 1 runner：e2e stage_a 103/103、`[SUMMARY] known=[]`、alloc 探针
        21/21 全 ok（该 runner 在修复前对此类用例 20/20 AV）；本机 hammer：stage_a 5 轮 103/103 +
        5 例各 ×10 全过。以下为历史记录（保留作回归对照）：2026-09-06 回归 known_failure：CI stage_a 偶发 timeout（挂起）——
        Box::new 的 alloc/Unique 链同 vec_push 类 regalloc spill 非确定性残余
        （见 vec_push reason）。转正标准：3 轮 stage_a+parallel 全绿且 exit=42。（2026-09-10 复核：
        stage_a 103/103 × 2 轮无复现；**8 并发负载下出现 4 次瞬态超时**（8 worker × 40 轮窗口内），
        每次均由 harness 的「同产物复跑」转为通过 ⇒ 同 vec_from_slice：宿主侧运行期延迟，
        非错码/错编译；见 WORKAROUNDS WA-40 与计划 §9.2）",
    },
    Case {
        name: "box_write",
        body: "let mut b = Box::new(7i32); *b = 42; *b",
        expected: 42,
        extra: "extern crate alloc;\nuse alloc::boxed::Box;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 4096] = [0; 4096];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        base.add(base.align_offset(32))\n    } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        // 深入定位（六轮）：编译通过但曾 0xC000001D 稳定失败。关键进展与修正：
        // ① 填栈实验有 artifacts：填 0x00/0x55/0xCC/0x1111 到 locals 区都会覆盖"已写
        //    槽"（b 槽 rbp-0x50 等）导致崩——不能证明 locals 未初始化；
        // ② b 槽（local 1, rbp-0x50）写读一致，Box::new 正确；第二个 *b 的
        //    `_4 = load b`（local 4, rbp-0x68）store_place 已生成但执行后 [rbp-0x68]
        //    为 0——store 未生效；
        // ③ "中途 spill 值丢失"假设已实现 pending spill-store 写回验证——证伪并撤销
        //    （def 的 store back 已写槽，pending 写回反而覆盖正确值，five_args 回归）；
        // ④ ASLR 相关确认：/DYNAMICBASE:NO 固定基址下 exit=42 稳定通过，ASLR 必崩；
        // ⑤ 根因实证：movabs 绝对地址 + IMAGE_REL_AMD64_ADDR64——link.exe 的 .reloc
        //    VirtualSize=0x28 只覆盖前 4 个 DIR64，第 5 个 movabs（ALLOC）的重定位在
        //    VirtualSize 外，ASLR 下 loader 不应用 → 访问链接时地址崩溃。已重构
        //    [lower.GlobalAddr] 为 RIP 相对寻址（lea rd,[rip+disp32] + REL32，@lea_rip_rel）
        //    ——movabs 执行流跑偏（旧 0x3752）已消除；
        // ⑥ 【本轮转正】统一聚合框架（Phase 1：槽零初始化 + enum 嵌套投影 field_ty 修复
        //    + copy_agg/pack_sp 统一原语）累积修复了"两次 deref 的 [rbp-0x68] 槽未初始化
        //    垃圾"与 Box 链 enum 解构偏移——ASLR 与固定基址下均稳定 exit=42（3 次运行确认），
        //    从 known_failure 移除。
        expect_trap: false,
        known_failure: false,
        phase: "P6 alloc",
        reason: "",
    },
    // ── Vec/String 完整运行（**2026-09-12 起全部转正：known_failure=false + FLAKY 为空**，
    //    即全部进 CI 门禁）。根因 = WORKAROUNDS **WA-42**（niche 枚举 tag 写入宽度：
    //    `Option<(NonNull<u8>, Layout)>` 的 None 只写 4 字节 → 高 4 字节残留 → 判别误判
    //    `Some(野指针)` → grow 链 copy 解引用 AV）。修复链与全部取证见
    //    docs/archive/forge-rustc-vec_push-plan.md §9.7（`passed=103/103 known=[]` 的 CI run
    //    93927221004 + 本机 hammer 5 轮 + 各 case reason 里的历史记录）。──
    Case {
        name: "vec_push",
        body: "let mut v = alloc::vec::Vec::new(); v.push(1); v.push(2); v.len() as i32",
        expected: 2,
        extra: "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        base.add(base.align_offset(32))\n    } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P2",
        reason: "2026-09-12 转正（WA-42 根治：niche 枚举 tag 写入宽度 = tag 标量）——CI run 93927221004
        的 AMD Family 25 Model 1 runner：e2e stage_a 103/103、`[SUMMARY] known=[]`、alloc 探针
        21/21 全 ok（该 runner 在修复前对此类用例 20/20 AV）；本机 hammer：stage_a 5 轮 103/103 +
        5 例各 ×10 全过。以下为历史记录（保留作回归对照）：2026-09-06 回归 known_failure：CI（e2e Windows）与本机 hammer（3 轮
        stage_a 中 1 轮）均现偶发 exit -1073741819（0xC0000005 AV，本机 439s 挂起/超时
        变体）——alloc/Vec grow 链（RawVec::grow_amortized/finish_grow）高压 spill 下
        sret copy_agg 写 [0]（sret_ptr 值丢失）类 regalloc 非确定性残余（上方注释指向
        forge-codegen regalloc）。2026-09-04 曾本地 3 次转正——spill 决策随编译实例
        变化。转正标准：3 轮 stage_a+parallel 全绿且 exit=2。（2026-09-10 复核：stage_a 103/103 × 2 轮；
        **8 并发负载下 3 个窗口（各 320 次单跑）中出现 1 次瞬态 15 s 超时**，由 harness 的
        「同产物复跑」转为通过 ⇒ 该次是宿主侧运行期延迟、非错码。**2026-09-11 CI 稳定 AV 双证**：
        CI 上 run #25/#26/#27 连续 `exit=-1073741819`（0xC0000005 AV），而该产物的
        `.text` SHA256 `a91b9eef…63fb`（94720 B）**与 CI 逐字节相同**、本机复跑 exit=2
        ⇒ 判为 **CI runner 环境性 AV**（同 cli_tests alloc AV 先例），非错编译；严格口径
        已按 `CI_ENV_AV_CODE` 容忍并留 `CI-ENV-AV` 标记（计划 §9.5）。def-spill 站点经
        FORGE_TRACE_ALLOC 核查全落在可改写字段——失败面已按 fail-closed 收口，见
        WORKAROUNDS WA-40 与计划 §9.5）",
    },
    Case {
        name: "vec_string",
        body: "let s = alloc::string::String::from(\"hi\"); s.len() as i32",
        expected: 2,
        extra: "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        base.add(base.align_offset(32))\n    } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P2",
        reason: "2026-09-12 转正（WA-42 根治：niche 枚举 tag 写入宽度 = tag 标量）——CI run 93927221004
        的 AMD Family 25 Model 1 runner：e2e stage_a 103/103、`[SUMMARY] known=[]`、alloc 探针
        21/21 全 ok（该 runner 在修复前对此类用例 20/20 AV）；本机 hammer：stage_a 5 轮 103/103 +
        5 例各 ×10 全过。以下为历史记录（保留作回归对照）：2026-09-06 回归 known_failure：CI 偶发 timeout（挂起：可能 assert 失败
        进入 panic loop）+ 本地 round 偶发——String::from 的 alloc/grow 链与 vec_push
        同类 regalloc spill 非确定性残余（见 vec_push reason）。转正标准：3 轮
        stage_a+parallel 全绿且 exit=2（2026-09-10 复核：stage_a 103/103 × 2 轮；**8 并发负载下 3 个窗口
        （各 320 次单跑）中出现 1 次瞬态 15 s 超时**，由 harness 的「同产物复跑」转为通过 ⇒
        与文档记录的 timeout 形态一致、属宿主侧运行期延迟（非错码）；def-spill 站点经
        FORGE_TRACE_ALLOC 核查全落在可改写字段——失败面已按 fail-closed 收口，见
        WORKAROUNDS WA-40 与计划 §9.2）",
    },
    Case {
        name: "vec_from_slice",
        body: "let v = alloc::vec::Vec::from(&[1u8, 2, 3][..]); v.len() as i32",
        expected: 3,
        extra: "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        base.add(base.align_offset(32))\n    } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P6 alloc",
        reason: "2026-09-12 转正（WA-42 根治：niche 枚举 tag 写入宽度 = tag 标量）——CI run 93927221004
        的 AMD Family 25 Model 1 runner：e2e stage_a 103/103、`[SUMMARY] known=[]`、alloc 探针
        21/21 全 ok（该 runner 在修复前对此类用例 20/20 AV）；本机 hammer：stage_a 5 轮 103/103 +
        5 例各 ×10 全过。以下为历史记录（保留作回归对照）：2026-09-06 回归 known_failure：CI e2e stage_a 偶发 timeout（挂起：
        Vec::from 的 slice→Vec 拷贝链与 vec_push 同类 regalloc spill 非确定性残余
        （见 vec_push reason；转正原因为 promoted/Unsize 降级正确性，非 alloc 链）。
        转正标准：3 轮 stage_a+parallel 全绿且 exit=3。（2026-09-10 复核：stage_a 103/103 × 2 轮无复现；
        **8 并发负载下复现 1 次为 15 s 超时**——该轮产物独立复跑 20/20 exit=3（91–368 ms），
        且同产物复跑即通过 ⇒ 判为宿主侧运行期延迟，非错码/错编译；harness 已加超时同产物
        复跑（FORGE_E2E_TIMEOUT_SECS 可覆盖阈值），见 WORKAROUNDS WA-40 与计划 §9.2）",
    },
    Case {
        name: "dyn_trait_call",
        body: "let d = Dog; let s: &dyn Speak = &d; s.speak()",
        expected: 7,
        extra: "trait Speak { fn speak(&self) -> i32; }\nstruct Dog;\nimpl Speak for Dog { fn speak(&self) -> i32 { 7 } }",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        // 动态分发（vtable 数据段）已完整实现：
        // ① unsize cast（&Dog → &dyn Speak）生成 vtable 数据段（.data——8 字节指针表：
        //    drop_in_place/size/align/方法指针），方法符号 ADDR64 重定位（.rodata 的
        //    ADDR64 不被 MSVC 链接器应用——实测全 0——必须放 .data 段）；
        // ② vtable 内容来自 tcx.vtable_allocation/vtable_entries（VtblEntry），
        //    self 类型必须传具体类型（&Dog → Dog，否则实例解析 ICE）；
        // ③ 间接调用返回值修复：CALL_RM 后缺 "MOV_RM8_R64 rd, RAX"（与直接调用
        //    [lower.Call] 一致）——结果 XReg 被 regalloc 分配到任意寄存器读到垃圾
        //    （曾返回 vtable+24=0x40003018）。最小用例 exit=7 稳定（3 次运行确认）。
        expect_trap: false,
        known_failure: false,
        phase: "P7 dyn",
        reason: "",
    },
    // ── 标准入口 fn main（Phase 2）：no_main + #[no_mangle] extern "C" fn main
    //    （rustc 前端对无 no_main 的 fn main 硬性要求 std，backend 无法绕过；
    //    MSVC 默认入口 mainCRTStartup 自动转调 main，无需 /ENTRY）──
    Case {
        name: "main_fn_entry",
        body: "let mut acc = 0i32; let mut i = 0; while i < 5 { acc += i; i += 1; } acc",
        expected: 10,
        extra: "",
        entry: "main",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P2 main",
        reason: "",
    },
    // ── 闭包（Phase 4）：AggregateKind::Closure 构造 + FnOnce/FnMut shim 调用
    //    （ICE 修复：item_name 对 closure DefId 无 name，改用 def_path_str）──
    Case {
        name: "closure_capture",
        body: "let x = 5i32; let f = |y: i32| x + y; f(7)",
        expected: 12,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P4 closure",
        reason: "",
    },
    Case {
        name: "closure_fnmut",
        body: "let mut acc = 0i32; let mut add = |v: i32| { acc += v; }; add(3); add(4); acc",
        expected: 7,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P4 closure",
        reason: "",
    },
    // ── static 数据段（Phase 4）：MonoItem::Static → .data/.rodata 符号 +
    //    const{allocN} 指针 → global_addr（G{N} 重定位）。只读与 static mut
    //    写回均验证（常量求值 fallback 修复后检查链正确）。──
    Case {
        name: "static_read",
        body: "COUNT",
        expected: 42,
        extra: "static COUNT: i32 = 42;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P4 static",
        reason: "",
    },
    Case {
        name: "static_mut_counter_overflow_off",
        body: "unsafe { CNT += 1; CNT }",
        expected: 1,
        extra: "static mut CNT: i32 = 0;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P4 static",
        reason: "",
    },
    Case {
        name: "static_two_fields",
        body: "A + B",
        expected: 42,
        extra: "static A: i32 = 7; static B: i32 = 35;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P4 static",
        reason: "",
    },
    // ── Drop glue（Phase 4）：DropInPlace shim 编译 + 作用域末/显式 drop 调用。
    //    副作用经 static mut 写观察（static_mut_counter 已验证该路径）。──
    Case {
        name: "drop_glue",
        body: "let mut d = D { v: 42 }; core::mem::drop(d); let d2 = D { v: 7 }; let r = d2.v; core::mem::drop(d2); r",
        expected: 7,
        extra: "struct D { v: i32 } impl Drop for D { fn drop(&mut self) { self.v = 0; } }",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P4 drop",
        reason: "",
    },
    // ── checked 算术元组解构（Phase 3）：let (v, o) = overflowing_add 的
    //    ScalarPair 返回值（RAX+RDX 双寄存器 + DSL ret_mov 遍历）──
    Case {
        name: "checked_destructure",
        body: "let (v, o) = 100i32.overflowing_add(200); if o { 0 } else { v }",
        expected: 300,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P3 checked",
        reason: "",
    },
    Case {
        name: "checked_destructure_overflow",
        body: "let (v, o) = i32::MAX.overflowing_add(1); if o { 1 } else { v }",
        expected: 1,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P3 checked",
        reason: "",
    },
    // ── Assert 失败路径（Phase 3 附加）：bounds check 通过路径正常返回；
    //    失败路径为 ud2（0xC000001D 非法指令崩溃，探针验证，不入 e2e——
    //    崩溃 exit code 与 bash/win32 表示相关，不稳定）。──
    Case {
        name: "array_bounds_ok",
        body: "let a = [10i32, 20, 30]; let i = core::hint::black_box(1usize); a[i]",
        expected: 20,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P3 checked",
        reason: "",
    },
    // ── 聚合值读写（Phase 1）：16 字节元组/含指针字段结构体的 move/copy ──
    Case {
        name: "agg_tuple64_move",
        body: "let a = (1i64, 2i64); let b = a; b.0 as i32 + b.1 as i32",
        expected: 3,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1 agg",
        reason: "",
    },
    Case {
        name: "agg_ptr_struct_move",
        body: "struct PtrWrap { p: *const i32, cap: i64 } let x = 42i32; let w = PtrWrap { p: &x as *const i32, cap: 1 }; let w2 = w; unsafe { *w2.p + w2.cap as i32 }",
        expected: 43,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1 agg",
        reason: "",
    },
    // 嵌套 niche 判别回归（2026-08 修复，cf5 转正）：
    // CF<Result<usize,u8>, usize> 的 niche_variants=[0]（Continue 是 niche）、
    // untagged=1（Break）——判别读取曾因主库 [lower.Select] 无条件返回 true
    // 臂而误判（Break 当 Continue）——rvalue.rs 改 rustc codegen_get_discr
    // 的算术公式（relative=raw-start；is_niche=relative ule max；discr=算术
    // select）后转正。vec_push 的 grow 错误链（Result/ControlFlow 嵌套 niche）
    // 同源，此用例为回归保护。
    Case {
        name: "nested_enum_break",
        body: "enum CF<T, R> { Continue(R), Break(T) } fn nested(n: usize) -> CF<Result<usize, u8>, usize> { if n > 1000 { CF::Break(Err(5u8)) } else { CF::Continue(n) } } match nested(2000) { CF::Continue(v) => v as i32, CF::Break(_) => 7 }",
        expected: 7,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P9 niche",
        reason: "",
    },
    // ── A3 overflow-checks=on（默认）：普通 + 生成 AddWithOverflow +
    //    Assert（checked 算术已支持）。运行期输入（black_box 防 const
    //    折叠）验证不溢出路径；常量溢出在 const-eval 拦截（编译期错误）。──
    Case {
        name: "overflow_checks_on",
        body: "let a = core::hint::black_box(1000i32); let b = core::hint::black_box(2000i32); a + b",
        expected: 3000,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "A3 overflow",
        reason: "",
    },
    Case {
        name: "overflow_checks_on_mul",
        body: "let a = core::hint::black_box(7i32); let b = core::hint::black_box(6i32); a * b",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "A3 overflow",
        reason: "",
    },
    // ── 聚合 f32 字段构造（B2 回归保护）：数组 [f32; 4] 字面量的逐字段
    //    store 曾走 builder.store（GPR 语义）→ 把地址寄存器低 32 位写进
    //    槽（SIMD3 反汇编实证 movl %r15d,(%r15)）。修复：Aggregate 分支
    //    按字段类型分派 store_ty（f32 → fstore/movss 语义）。──
    Case {
        name: "float_array_construct",
        body: "let a = [1.5f32, 2.5, 3.5, 4.5]; (a[0] as i32) + (a[2] as i32)",
        expected: 4, // 1.5→1、3.5→3、sum=4
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "P1 agg",
        reason: "",
    },
    // ── ≤16B 向量按值 ABI（2026-09 转正，WA-37 D3）：V64/V128（rustc
    //    SimdVector ≤16B）在 Windows x64 按值走 XMM（abi.rs Direct）——
    //    主库 v14 DSL 对 VEC(16) 参数/返回/实参做全宽 128 位 XMM 移动
    //    （MOVAPS；MOVSD/MOVSS 8/4B 静默截断为 D3 前形态，B3 门控已撤）。
    //    lane3/lane1（高半）经 XMM 全宽往返不丢——旧 simd_abi_gated 负向
    //    用例（编译期拒绝"向量 ABI"）已随门控删除。──
    Case {
        name: "simd_v128_local",
        body: "let v = core::simd::Simd::<f32, 4>::from_array([1.0, 2.0, 3.0, 4.5]); (v[0] as i32) * 100 + (v[3] as i32)",
        expected: 104, // 纯局部：构造（Direct XMM0 返回）+ Index 高半 lane3=4.5→4
        extra: "#![feature(portable_simd)]",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "B3 simd",
        reason: "V128 局部按值链路转正（WA-37 D3）：Direct XMM 返回 + 全宽收参",
    },
    Case {
        name: "simd_v128_call",
        body: "let v = core::simd::Simd::<f32, 4>::from_array([1.0, 2.0, 3.0, 4.5]); (helper(v) as i32) * 1000 + (v[3] as i32)",
        expected: 4004, // helper 收 V128 按值参数（XMM 全宽）→ lane3=4.5→4 → 4000+4
        extra: "#![feature(portable_simd)]\nuse core::simd::Simd;\nfn helper(v: Simd<f32, 4>) -> f32 { v[3] }\n",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "B3 simd",
        reason: "V128 按值参数跨函数往返转正（WA-37 D3）：高半 lane3 不截断",
    },
    Case {
        name: "simd_v128_return",
        body: "let r = make(); (r[3] as i32) * 10 + (r[0] as i32)",
        expected: 71, // make 返回 V128（XMM0 全宽）→ lane3=7.5→7、lane0=1 → 71
        extra: "#![feature(portable_simd)]\nuse core::simd::Simd;\nfn make() -> Simd<f32, 4> { core::simd::Simd::from_array([1.0, 2.0, 3.0, 7.5]) }\n",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "B3 simd",
        reason: "V128 按值返回转正（WA-37 D3）：Direct XMM0 返回全宽",
    },
    Case {
        name: "simd_v64_call",
        body: "let v = core::simd::Simd::<f32, 2>::from_array([1.0, 5.5]); (helper(v) as i32) * 100 + (v[1] as i32)",
        expected: 505, // V64 按值参数 lane1=5.5→5（修复前 lane1 槽位错位恒 0）
        extra: "#![feature(portable_simd)]\nuse core::simd::Simd;\nfn helper(v: Simd<f32, 2>) -> f32 { v[1] }\n",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "B3 simd",
        reason: "V64 按值参数/返回转正（WA-37 D3）",
    },
    // ── V256 跨函数 ABI（2026-09 A-S1 转正，WA-37 D2 基建）：>16B 向量
    //    （V256 32B，rustc backend_repr=SimdVector）在 Windows x64 无 YMM
    //    参数寄存器——abi.rs 归类 Indirect（by-ref 传参 + sret 返回），
    //    值全程内存建模（local 槽按 layout_bytes 32B 分配 + copy_agg
    //    整值拷贝），不再物化向量寄存器值。core 侧链：main → from_array
    //    （sret）→ Simd::load（CopyNonOverlapping 32B memcpy，>8B 元素
    //    全宽块拷贝已修）→ 局部 v 槽；v[0]/v[7] 经 portable_simd 的
    //    Index（as_array → &[f32] 指针/标量读，不产生 simd_extract）。
    //    helper 收 V256 参数（Indirect &实参槽 → 收参 copy_agg）。──
    Case {
        name: "simd_v256_call",
        body: "let v = core::simd::Simd::<f32, 8>::from_array([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]); (helper(v) as i32) * 1000 + (v[0] as i32)",
        expected: 1001, // helper(v)=lane0=1.0→1 → 1000*1 + v[0]=1 → 1001
        extra: "#![feature(portable_simd)]\nuse core::simd::Simd;\nfn helper(v: Simd<f32, 8>) -> f32 { v[0] }\n",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "B3 simd",
        reason: "V256 by-ref/sret + 内存值建模转正（WA-37 D2）；全 lane 抽查 87654321 亦绿",
    },
    Case {
        name: "simd_v256_local",
        body: "let v = core::simd::Simd::<f32, 8>::from_array([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]); (v[0] as i32) * 100 + (v[7] as i32)",
        expected: 108, // 纯局部：构造 + 索引
        extra: "#![feature(portable_simd)]",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "B3 simd",
        reason: "V256 向量值基建转正（WA-37 D2）：SimdVector>16B→Indirect + CopyNonOverlapping 全宽",
    },
    // ── D 组 intrinsics（2026-09 转正）：rotate/cttz/ctlz/bitreverse/volatile。
    //    主库修复：lzcnt/tzcnt/popcnt 32 位变体（gpr32 类，否则 64 位
    //    lzcntq 对 u32 返回 63 非 31）；Bitreverse 32 位掩码 SWAR（64 位
    //    掩码反转高位垃圾）。──
    Case {
        name: "intrinsics_bit_ops",
        body: "let mut acc = 0i32; let r = 0x10001u32.rotate_left(4); acc += (r == 0x100010) as i32 * 100; acc += (0x80u32.trailing_zeros() == 7) as i32 * 10; acc += (1u32.leading_zeros() == 31) as i32; acc += (0x1u32.reverse_bits() == 0x8000_0000) as i32 * 1000; acc",
        expected: 1111,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "intrinsics_volatile",
        body: "let mut buf = [0u8; 8]; unsafe { core::ptr::write_volatile(buf.as_mut_ptr(), 42u8) }; let v = unsafe { core::ptr::read_volatile(buf.as_ptr()) }; v as i32 * 10000",
        expected: 420000,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    // 原子 RMW 转正（D 组，2026-08）：主库 AtomicRmw 经 JIT 高压测试
    // 验证（jit.rs test_jit_atomic_rmw_basic/spill_pressure）推翻原
    // "regalloc spill 缺失"假设——解除编译期门控，Xchg/Add/Sub 可用
    // （And/Or/.../Cmpxchg 仍拒绝，见 intrinsics.rs）。
    Case {
        name: "atomic_fetch_add",
        body: "static A: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(5); let old = A.fetch_add(3, core::sync::atomic::Ordering::Relaxed); (if old == 5 { 1000000 } else { 0 }) + A.load(core::sync::atomic::Ordering::Relaxed)",
        expected: 1000008, // old=5（返回旧值）+ 内存=8（新值）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    // i32 快路径哨兵：AtomicRmw {g1}→{out} 后 fetch_add/swap 的 i32 走
    // xadd/xchg 32 位（原 {g1} 恒 64 位池 8 字节越界——v13 单 static 侥幸，
    // 数组相邻必炸）。A[1]/A[2] 哨兵验证 4 字节写不越界。
    Case {
        name: "atomic_i32_fetch_add_arr",
        body: "static A: [core::sync::atomic::AtomicI32; 3] = [core::sync::atomic::AtomicI32::new(5), core::sync::atomic::AtomicI32::new(0x15555555), core::sync::atomic::AtomicI32::new(0x0A0A0A0A)]; let old = A[0].fetch_add(3, core::sync::atomic::Ordering::Relaxed); (if old == 5 { 1000 } else { 0 }) + A[0].load(core::sync::atomic::Ordering::Relaxed) + (A[1].load(core::sync::atomic::Ordering::Relaxed) >> 4) + (A[2].load(core::sync::atomic::Ordering::Relaxed) & 0xF)",
        expected: 1000 + 8 + (0x15555555 >> 4) + (0x0A0A0A0A & 0xF), // 哨兵保留（xadd 32 位不越界）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_i32_swap_arr",
        body: "static A: [core::sync::atomic::AtomicI32; 2] = [core::sync::atomic::AtomicI32::new(77), core::sync::atomic::AtomicI32::new(0x12345678)]; let old = A[0].swap(99, core::sync::atomic::Ordering::SeqCst); (if old == 77 { 100 } else { 0 }) + A[0].load(core::sync::atomic::Ordering::Relaxed) + (A[1].load(core::sync::atomic::Ordering::Relaxed) & 0xFFFF)",
        expected: 100 + 99 + (0x12345678 & 0xFFFF), // old=77、mem=99、A[1] 哨兵保留（xchg 32 位）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_xchg",
        body: "static B: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(100); let old = B.swap(7, core::sync::atomic::Ordering::SeqCst); (if old == 100 { 1000 } else { 0 }) + B.load(core::sync::atomic::Ordering::Relaxed) as i32",
        expected: 1007, // old=100、内存=7
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_fetch_sub",
        body: "static C: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(42); let old = C.fetch_sub(2, core::sync::atomic::Ordering::AcqRel); old - C.load(core::sync::atomic::Ordering::Relaxed)",
        expected: 2, // old=42、内存=40 → 42-40
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_i64",
        body: "static A: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(100); let old = A.fetch_add(50, core::sync::atomic::Ordering::Relaxed); (if old == 100 { 1000 } else { 0 }) + A.load(core::sync::atomic::Ordering::Relaxed) as i32",
        expected: 1150, // old=100、内存=150（i64 原子路径，opsize 64）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    // ── u8/u16/i8/bool 窄原子（v14 字节宽度解锁）：1/2 字节走 CMPXCHG 循环
    //    （xadd/xchg 的 {g1} 恒 64 位池会越界——窄域改用 cmpxchg 循环，
    //    8 位 lockcmpxchg8/16 位 66 前缀）；数组邻元素作越界哨兵。──
    Case {
        name: "atomic_u8_fetch_add_wrap",
        body: "static A: [core::sync::atomic::AtomicU8; 3] = [core::sync::atomic::AtomicU8::new(250), core::sync::atomic::AtomicU8::new(7), core::sync::atomic::AtomicU8::new(9)]; let old = A[0].fetch_add(10, core::sync::atomic::Ordering::Relaxed); (if old == 250 { 1000 } else { 0 }) + (A[0].load(core::sync::atomic::Ordering::Relaxed) as i32) * 10 + A[1].load(core::sync::atomic::Ordering::Relaxed) as i32 + A[2].load(core::sync::atomic::Ordering::Relaxed) as i32",
        expected: 1000 + 40 + 7 + 9, // old=250、250+10 wrap=4（1 字节）、A[1]=7、A[2]=9（越界哨兵）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_u8_fetch_and",
        body: "static A: [core::sync::atomic::AtomicU8; 2] = [core::sync::atomic::AtomicU8::new(0xFC), core::sync::atomic::AtomicU8::new(0x55)]; let old = A[0].fetch_and(0x0F, core::sync::atomic::Ordering::AcqRel); (if old == 0xFC { 100 } else { 0 }) + (A[0].load(core::sync::atomic::Ordering::Relaxed) as i32) + (A[1].load(core::sync::atomic::Ordering::Relaxed) as i32) * 256",
        expected: 100 + 0x0C + 0x55 * 256, // old=0xFC、内存=0x0C、A[1] 哨兵（WA-36 常量 u8 直接比较修复后）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_u8_fetch_xor",
        body: "static A: [core::sync::atomic::AtomicU8; 2] = [core::sync::atomic::AtomicU8::new(0xA5), core::sync::atomic::AtomicU8::new(0x33)]; let old = A[0].fetch_xor(0x0F, core::sync::atomic::Ordering::Relaxed); (if old == 0xA5 { 100 } else { 0 }) + (A[0].load(core::sync::atomic::Ordering::Relaxed) as i32) + (A[1].load(core::sync::atomic::Ordering::Relaxed) as i32) * 256",
        expected: 100 + (0xA5 ^ 0x0F) + 0x33 * 256, // old=0xA5、内存=0xAA、A[1] 哨兵
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_i8_fetch_max",
        body: "static A: core::sync::atomic::AtomicI8 = core::sync::atomic::AtomicI8::new(-50); let old = A.fetch_max(20, core::sync::atomic::Ordering::Relaxed); (if old == -50 { 500 } else { 0 }) + A.load(core::sync::atomic::Ordering::Relaxed) as i32",
        expected: 520, // old=-50、max(-50,20)=20（signed 窄值 WA-36 修复后）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_u16_fetch_or",
        body: "static A: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0x00FF); let old = A.fetch_or(0xFF00, core::sync::atomic::Ordering::Relaxed); (if old == 0xFF { 100 } else { 0 }) + (A.load(core::sync::atomic::Ordering::Relaxed) as i32) * 3",
        expected: 100 + 0xFFFF * 3, // old=0xFF、内存=0xFFFF（16 位）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_i16_fetch_add_neg",
        body: "static A: core::sync::atomic::AtomicI16 = core::sync::atomic::AtomicI16::new(30000); let old = A.fetch_add(-10000, core::sync::atomic::Ordering::Relaxed); (if old == 30000 { 1000 } else { 0 }) + A.load(core::sync::atomic::Ordering::Relaxed) as i32",
        expected: 1000 + 20000, // old=30000、30000-10000=20000（i16 域，无溢出）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_bool_swap_xor",
        body: "static A: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true); let old = A.swap(false, core::sync::atomic::Ordering::Relaxed); let f = A.fetch_xor(true, core::sync::atomic::Ordering::Relaxed); (if old { 100 } else { 0 }) + (if !f { 50 } else { 0 }) + (if A.load(core::sync::atomic::Ordering::Relaxed) { 5 } else { 0 })",
        expected: 100 + 50 + 5, // swap→false(old=true)、xor(true)→true(f=false)、终态 true
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    // ── compare_exchange 双结果（v14 P4）：atomic_cxchg 返回 (T,bool)
    //    tuple——intrinsics 派 2 值（old + icmp 派生 ok），ScalarPair
    //    destination 走 pack_sp 写回；x86 单发 lockcmpxchg 即单次比较。──
    Case {
        name: "atomic_compare_exchange_ok",
        body: "static A: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(42); match A.compare_exchange(42, 100, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Acquire) { Ok(v) => { (if v == 42 { 1000 } else { 1 }) + (A.load(core::sync::atomic::Ordering::Relaxed) as i32) }, Err(_) => 0 }",
        expected: 1100, // Ok(42)（期望命中）+ 内存 100
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_compare_exchange_fail",
        body: "static A: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(42); match A.compare_exchange(7, 100, core::sync::atomic::Ordering::SeqCst, core::sync::atomic::Ordering::SeqCst) { Ok(_) => 0, Err(v) => { (if v == 42 { 200 } else { 2 }) + (A.load(core::sync::atomic::Ordering::Relaxed) as i32) } }",
        expected: 242, // Err(42)（期望未命中，返回当前值）+ 内存保持 42
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "atomic_compare_exchange_weak_u8",
        body: "static A: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0x7F); match A.compare_exchange_weak(0x7F, 0x80, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed) { Ok(v) => { (if v == 0x7F { 300 } else { 3 }) + (A.load(core::sync::atomic::Ordering::Relaxed) as i32) }, Err(_) => 0 }",
        expected: 300 + 0x80, // Ok(0x7F) + 内存 0x80（u8 ScalarPair (u8,bool)）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "static_mut_write",
        body: "static mut X: i32 = 5; unsafe { X += 7; X }",
        expected: 12,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    // ── WA-44：niche 落在聚合 payload **非 0 偏移** ──
    // 24 字节 payload（Memory repr）的 niche 在第 3 个字段（offset 16）；旧实现
    // 读写判别一律用 offset 0 ⇒ 判别读到字段 0 的 `0` ⇒ `Some` 被误判为 `None`。
    Case {
        name: "niche_offset_some_zero_first",
        body: "let o: Option<(usize, usize, core::ptr::NonNull<u8>)> = \
               Some((0, 2, core::ptr::NonNull::dangling())); \
               match o { Some((a, b, _)) => (a + b + 10) as i32, None => 99 }",
        expected: 12, // Some → 0 + 2 + 10；旧行为读 offset 0 见 0 → 误判 None → 99
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "WA-44 niche",
        reason: "niche 偏移一般化（tag_field 派生）的守卫用例",
    },
    Case {
        name: "niche_offset_none_roundtrip",
        body: "let o: Option<(usize, usize, core::ptr::NonNull<u8>)> = None; \
               match o { Some((a, b, _)) => (a + b + 10) as i32, None => 99 }",
        expected: 99, // None 仍必须是 None（写入偏移修正后不能把 niche 写到字段 0）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "WA-44 niche",
        reason: "niche 偏移一般化（tag_field 派生）的守卫用例（None 侧）",
    },
    // ── Range 迭代（WA-20）──
    Case {
        name: "range_next_once",
        body: "let mut r = 0i32..4; let v = r.next().unwrap(); (if v == 0 { 42 } else { 0 })",
        expected: 42, // 单次 next 正确（WA-20 基线：0..4 第一次返回 Some(0)）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "F1 iter",
        reason: "",
    },
    Case {
        name: "range_next_twice",
        body: "let mut r = 0i32..4; let _ = r.next(); let v = r.next().unwrap(); (if v == 1 { 42 } else { 0 })",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "F1 iter",
        reason: "2026-08 转正：WA-23（ScalarPair pack_sp/unpack_sp 按标量宽度读写）——Option<i32> 的 hi 字段 8 字节写越界覆盖相邻 Range.start 槽 → start 不递增；修复后第二次 next 正确返回 Some(1)",
    },
    // ── F1 扩编：迭代器 / 字符串 / 数组 of 结构体 / 聚合传参──
    Case {
        name: "slice_iter_sum",
        body: "let a = [1i32, 2, 3, 4]; let mut s = 0; for x in a.iter() { s += *x; } s",
        expected: 10,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "F1 iter",
        reason: "2026-08 转正：WA-24（switchInt const 判别折叠）——Iter::next 的 `switchInt(UbChecks)` 判别是编译期常量（core::intrinsics::ub_checks 求值 false），折叠后只生成目标分支，不可达的 precondition_check 调用块不再 lower → 无 LNK2019",
    },
    Case {
        name: "vec_iter_enumerate",
        body: "let mut v = alloc::vec::Vec::new(); v.push(10); v.push(20); v.push(30); let mut s = 0; for (i, x) in v.iter().enumerate() { s += i as i32 * x; } s",
        expected: 80, // 0*10 + 1*20 + 2*30
        extra: "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        base.add(base.align_offset(32))\n    } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "F1 iter",
        reason: "2026-09-12 转正（WA-42 根治：niche 枚举 tag 写入宽度 = tag 标量）——CI run 93927221004
        的 AMD Family 25 Model 1 runner：e2e stage_a 103/103、`[SUMMARY] known=[]`、alloc 探针
        21/21 全 ok（该 runner 在修复前对此类用例 20/20 AV）；本机 hammer：stage_a 5 轮 103/103 +
        5 例各 ×10 全过。以下为历史记录（保留作回归对照）：2026-09-06 回归 known_failure：CI 偶发 exit -1073741819（AV，expect 80）
        ——Vec grow + iter 链同 vec_push 类 regalloc spill 非确定性残余（见 vec_push
        reason；WA-29 的 null 解引用曾同类）。转正标准：3 轮 stage_a+parallel 全绿且 exit=80。（2026-09-10 复核：stage_a 103/103 × 2 轮；**8 并发负载下 3 个窗口（各 320 次单跑）中出现 1 次瞬态 15 s 超时**，由 harness 的「同产物复跑」转为通过 ⇒ 该次为宿主侧运行期延迟。**2026-09-11 CI 稳定 AV 双证**：run #25/#26/#27 连续 AV（0xC0000005），而产物 `.text` SHA256 `5f421f08…c732`（103936 B）与 CI 逐字节相同、本机复跑 exit=80 ⇒ 判为 **CI runner 环境性 AV**，非错编译；严格口径已按 `CI_ENV_AV_CODE` 容忍并留 `CI-ENV-AV` 标记（计划 §9.5）。def-spill 站点经 FORGE_TRACE_ALLOC 核查全落在可改写字段——失败面已按 fail-closed 收口，见 WORKAROUNDS WA-40）",
    },
    Case {
        name: "string_concat_len",
        body: "let mut s = alloc::string::String::from(\"ab\"); s.push('c'); s.push('d'); s.len() as i32",
        expected: 4,
        extra: "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        base.add(base.align_offset(32))\n    } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "F1 string",
        reason: "",
    },
    Case {
        name: "array_of_struct",
        body: "struct P { x: i32, y: i32 } let a = [P { x: 1, y: 2 }, P { x: 3, y: 4 }]; a[0].x + a[1].y",
        expected: 5,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "F1 agg",
        reason: "",
    },
    Case {
        name: "struct_array_field_sum",
        body: "struct P { xs: [i32; 3] } let p = P { xs: [10, 20, 30] }; p.xs[0] + p.xs[2]",
        expected: 40,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "F1 agg",
        reason: "",
    },
    Case {
        name: "fn_agg_arg_return",
        body: "struct Pair { a: i32, b: i64 } fn sum(p: Pair) -> i64 { p.a as i64 + p.b } sum(Pair { a: 7, b: 35 }) as i32",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "F1 agg",
        reason: "2026-08 转正：{i32,i64} 是 ScalarPair（2 标量）→ 拆 lo/hi 传参路径正确（旧 reason 的\"16 字节非 ScalarPair\"过时；用例原 body 返回 i64 与入口 i32 不匹配属用例自身错误）",
    },
    Case {
        name: "nested_loop_break_outer",
        body: "let mut total = 0; 'outer: for i in 0..4 { for j in 0..4 { if i * j == 9 { break 'outer; } total += 1; } } total",
        expected: 15, // i=0:4, i=1:4, i=2:4, i=3: j=0..2 累加 3 次后 j=3 遇 3*3=9 break → 4+4+4+3=15
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "F1 control",
        reason: "2026-08 转正：WA-23（pack_sp/unpack_sp 宽度）修复 Range 迭代链后循环正确终止；LLVM 实证真实语义为 15（原 expected=12 注释手算错误：i=3 时 j=0,1,2 三次 3*0/3*1/3*2≠9 均累加，j=3 才 break 'outer）",
    },
    Case {
        name: "match_str_result",
        body: "fn pick(n: i32) -> Result<i32, &'static str> { if n > 0 { Ok(n * 2) } else { Err(\"neg\") } } match pick(21) { Ok(v) => v, Err(_) => 0 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "F1 enum",
        reason: "",
    },
    // ── C1 DebugInfo line-tables-only：`-C debuginfo=1` 编译 + 运行正确
    //    （DWARF 段生成不破坏代码；llvm-objdump 可见 .debug_* 段）。──
    Case {
        name: "debuginfo_line_tables",
        body: "let a = core::hint::black_box(21i32); let b = a * 2; b",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "C1 dwarf",
        reason: "",
    },
    // ── C2 DebugInfo full：`-C debuginfo=2` 编译 + 运行正确（变量/参数
    //    DIE 生成 + fbreg 位置不破坏代码；变量名/类型检查在 dwarf 单测，
    //    此处守护编译与运行路径）。──
    Case {
        name: "debuginfo_full",
        body: "fn add2(a: i32, b: i32) -> i32 { let c = a + b; c } let x = core::hint::black_box(40i32); let y = add2(x, 2); y",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "C2 dwarf",
        reason: "",
    },
    // ── D 组补充：copy_nonoverlapping（rustc 展开成
    //    StatementKind::Intrinsic(CopyNonOverlapping)——曾落入 `_ => {}`
    //    忽略 → 复制不执行；statement.rs 内联循环后转正）。──
    Case {
        name: "copy_nonoverlapping_stmt",
        body: "let mut buf = [0u8; 8]; unsafe { core::ptr::write_volatile(buf.as_mut_ptr(), 42u8) }; unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), buf.as_mut_ptr().add(4), 1) }; buf[4] as i32",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    Case {
        name: "copy_nonoverlapping_multi",
        body: "let mut src = [1u32, 2]; let mut dst = [0u32; 2]; unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), 2) }; dst[0] as i32 + dst[1] as i32",
        expected: 3,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    // ── F2 负向测试：编译失败断言（失败即报错）──
    // ── v27：`core::intrinsics::abort()`（此前落到 `unsupported intrinsic`）──
    //   后端无内联 `asm!` ⇒ 实现为 kernel32 `ExitProcess(3)`（MSVC `abort` 的码；
    //   Rust 对 `abort()` 的退出码未规定）。用例走**真调用**路径，断言退出码 = 3。
    Case {
        name: "intrinsic_abort",
        body: "if core::hint::black_box(true) { unsafe { core::intrinsics::abort() } } 42",
        expected: 3,
        extra: "#![feature(core_intrinsics)]",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 intrinsics",
        reason: "abort() → ExitProcess(3)，进程立即终止（不返回 42）",
    },
    // ── v27 M2：`panic=unwind` **明确拒绝**（`[cap:UNWIND]`）──
    //   后端的拒绝原点是 `codegen_crate` 里的 panic 策略检查：缺 SEH 展开表
    //   （`.pdata`/`.xdata`）与 `__rust_eh_personality`。用例名含 `panic_unwind`
    //   ⇒ harness 不为它加 `-C panic=abort`（否则这条路径走不到）。
    Case {
        name: "neg_panic_unwind",
        body: "42",
        expected: -1,
        // `no_std` + `panic=unwind` 下 rustc **先**要求 `eh_personality` lang item
        // （缺它时分析期就报错、根本走不到后端的 panic 策略检查）⇒ 用例自带一个哑实现，
        // 让编译推进到 codegen，从而真正断言后端那条 `[cap:UNWIND]` 拒绝。
        extra: "#![feature(lang_items)]\n#[lang = \"eh_personality\"]\nextern \"C\" fn eh_personality() {}",
        entry: "mainCRTStartup",
        expect_compile_fail: true,
        expect_compile_err: "cap:UNWIND",
        expect_trap: false,
        known_failure: false,
        phase: "M2 neg",
        reason: "panic=unwind 明确拒绝（缺 SEH 展开表）——v27 M2 加缺口 ID",
    },
    // ── v27 TLS（动态模型）：`#[thread_local]` 真的能用 ──
    //   路径：`const_data_addr` 认出线程局部静态 ⇒ 调 `__forge_tls_addr`
    //   （TlsAlloc/TlsGetValue 惰性初始化 + 首触拷贝初值）。
    //   用例同时钉三件事：① 零初值静态能读写；② **非零初值模板真的被拷贝**
    //   （Y=5 起步，+2 后应为 7）；③ 读回的是本线程的块（X=7）。⇒ 7 + 7 = 14。
    Case {
        name: "thread_local_roundtrip",
        body: "unsafe { X = 7; Y = Y + 2; } unsafe { X + Y }",
        expected: 14,
        extra: "#![feature(thread_local)]\n#[thread_local]\nstatic mut X: i32 = 0;\n#[thread_local]\nstatic mut Y: i32 = 5;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 TLS",
        reason: "动态 TLS：零初值 + 非零初值模板 + 读回（v27 TLS 真实现）",
    },
    // ── v27 M2：内联 `asm!` **明确拒绝**（fail-closed，不再是静默 `unreachable`）──
    //   落地前实测：`asm!("mov {0}, 7", out(reg) x)` **编译无声通过**、运行退出
    //   `0xC000001D`（后端把该处降级成兜底 `unreachable`）⇒ 静默错值。
    //   现在 ⇒ 编译期报 `[cap:INLINE_ASM]`（本用例钉住"必须编译失败 + 报缺口 ID"）。
    // ── v27 M2.5/S1：无操作数的内联 `asm!`（纯文本形态）真的能用 ──
    //   路径：`TerminatorKind::InlineAsm`（纯 String 模板、非 noreturn）
    //   ⇒ IR `InlineAsm`（文本走 `Immediate::String`）⇒ 降级期本 ISA 汇编器解析
    //   ⇒ 机器指令就地插进函数体。`nop` 只验证"编过 + 跑通 + 后续代码照常执行"；
    //   "字节真的被执行"由下面的 `inline_asm_ud2` 证明。
    Case {
        name: "inline_asm_nop",
        body: "unsafe { core::arch::asm!(\"nop\") }; 42",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M2.5 S1",
        reason: "asm!(\"nop\")：文本经 ISA 汇编器编成 0x90 插进函数体，后续代码照常执行",
    },
    //   `ud2` 是非法指令 ⇒ 真执行到就必是 NTSTATUS 异常（0xC000001D）。比 nop 强：
    //   它证明插进去的**字节真的被 CPU 执行了**，而不是被默默丢掉。
    Case {
        name: "inline_asm_ud2",
        body: "unsafe { core::arch::asm!(\"ud2\") }; 42",
        expected: -1073741795, // 0xC000001D = NTSTATUS #UD（非法指令）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: true,
        known_failure: false,
        phase: "v27 M2.5 S1",
        reason: "asm!(\"ud2\") 的字节必须真的被执行 ⇒ NTSTATUS 0xC000001D",
    },

    // ── v27 M2.5/S2a：内联 asm 的**寄存器约束**（in/out/inout + reg 类）──
    //   机制：挑物理寄存器 → 文本占位符换成寄存器名 → 搬入（dst 写死、src 绑 vreg）
    //   → asm 本体 → 搬出（dst 绑 vreg）→ 相关指令带 clobbers。
    //   强断言：不带 asm 时 x 还是 5（exit 5）；asm 生效后 x = 5 + 37 = 42。

    // ── v27 M2.5/S2a：内联 asm 的**寄存器约束**（in/out/inout + reg 类）──
    //   机制：挑物理寄存器 → 占位符换成寄存器名 → 搬入（dst 写死、src 绑 vreg，
    //   字段按角色挑而不是写死 0/1）→ asm 本体 → 搬出（dst 绑 vreg）→ 带 clobbers。
    //   强断言：不带 asm 时 x 还是 5（exit 5）；asm 生效后 x = 5 + 37 = 42。
    Case {
        name: "inline_asm_reg_inout",
        body: "let mut x = 5u64; let y = 37u64; unsafe { core::arch::asm!(\"add {0}, {1}\", inout(reg) x, in(reg) y) }; x as i32",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M2.5 S2a",
        reason: "asm!(\"add {0}, {1}\", inout(reg) x, in(reg) y)：x 从 5 变 42",
    },

    // ── v27 M2.5/S3：内联 asm 的 `const` 占位符 ──
    //   模板里 `{1}` 是 `const 7` ⇒ rustc 侧**就地换成字面文本**"7"
    //   （格式化内核 `global_asm::scalar_text`，与 global_asm! 同一处实现），
    //   只有寄存器占位符进入约束表。x = 35 + 7 = 42。
    Case {
        name: "inline_asm_const",
        body: "let mut x = 35u64; unsafe { core::arch::asm!(\"add {0}, {1}\", inout(reg) x, const 7) }; x as i32",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M2.5 S3",
        reason: "asm!(\"add {0}, {1}\", inout(reg) x, const 7)：const 7 被格式化成字面文本",
    },

    // ── v27 M2.5/S2b：内联 asm 的**显式寄存器名** ──
    //   `out("rax")` 要求槽 0 就用 RAX（rustc 对它的**规范名是 `ax`**，靠谱里的
    //   `aliases` 映射到 64 位组）。rustc 前端**不允许**模板里用 `{0}` 引用显式寄存器
    //   （必须直接写寄存器名）⇒ 模板写 `xor rax, rax` 把 RAX 清零 ⇒ x 必须变 0。
    //   强断言：若实现忽略显式名（挑了别的寄存器），x 仍是 5 ⇒ exit 5 ⇒ 红。
    Case {
        name: "inline_asm_named_reg",
        body: "let mut x = 5u64; unsafe { core::arch::asm!(\"xor rax, rax\", out(\"rax\") x) }; x as i32",
        expected: 0,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M2.5 S2b",
        reason: "out(\"rax\")：显式寄存器名必须真的被用上（否则 x 留在 5）",
    },

    Case {
        name: "neg_inline_asm",
        body: "let mut x: i32 = 0; unsafe { core::arch::asm!(\"mov {0}, 7\", out(reg) x) }; x",
        expected: -1,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: true,
        expect_compile_err: "cap:INLINE_ASM",
        expect_trap: false,
        known_failure: false,
        phase: "M2 neg",
        reason: "内联 asm! 尚未支持 ⇒ 编译期 fail-closed（v27 M2）",
    },
    //   注：`neg_global_asm` 于 **v27 M2 删除**——`global_asm!` 已落地（正向用例
    //   `global_asm_call` 钉住"符号真的可调用"），旧的"必须编译失败"断言随之过时；
    //   不支持的三类形态（`const`/`sym`/寄存器操作数）由生成期的 `[cap:GLOBAL_ASM_*]`
    //   缺口 ID 拒绝，属"定向 fail-closed"，不再需要一个笼统的负向用例。
    Case {
        name: "neg_std_program",
        body: "let s = String::from(\"hi\"); s.len() as i32",
        expected: -1,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: true,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "F2 neg",
        reason: "std 程序（no_std 上下文用 String）→ 编译失败（std 未链接/未声明 extern crate std）",
    },
    // ── M1（v27，2026-10-11）：**运行期溢出必须 trap**（`WA-05` 的真实残余）──
    //   实测取证链：溢出 + checks on ⇒ `Assert` 失败边分流 ⇒ panic handler。
    //   此前 handler 是 `loop {}` ⇒ 表现成"挂起"，而 `run_exe_with_timeout` 又把
    //   "异常终止（无退出码）"与"超时"混在一个分支 ⇒ **trap 被误报成挂起**。
    //   本里程碑两件都修：handler 改 `ud2`（真异常终止）、退出结局分三态
    //   （正常 / 异常终止 / 超时）并加 `expect_trap` 显式断言。
    //   探针口径：静默回绕会**正常退出** `-2147483648`（⇒ FAIL）；trap 必须异常终止。
    Case {
        name: "overflow_runtime_trap",
        body: "let a = core::hint::black_box(2147483647i32); let b = core::hint::black_box(1i32); a + b",
        // 期望**具体机制**：断言失败边分流 ⇒ panic handler ⇒ `ExitProcess(101)`。
        // 静默回绕会正常退出 -2147483648（≠ 101）⇒ 必红；断言不触发而挂起 ⇒ 超时 ⇒ 也必红。
        expected: TRAP_EXIT,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: true,
        known_failure: false,
        phase: "M1 overflow trap",
        reason: "运行期溢出必须 trap（异常终止）；正常退出（回绕）即失败",
    },
    // ── v27 M2：`global_asm!`（`WA-03` 落地）──
    //   文本交给系统上的 LLVM（另起 rustc 子进程，`--emit obj`）汇编成**独立对象**，
    //   由 `CompiledModule::global_asm_object` 交给 rustc 链接（参考 cranelift 后端）。
    //   本用例钉"符号真的可被 Rust 侧 `extern "C"` 调到"：20 + 22 = 42。
    //   Windows x64 前两个整型参数在 ECX/EDX。
    //
    //   v27 M2 收尾：`sym` / `const` 占位符也支持了——下面第二个用例同时用两者：
    //   `lea rax, [rip + {f}]`（`sym` 名字写错 ⇒ **链接期**就报未定义符号）+
    //   `mov eax, {c}`（`c = const 42` ⇒ 退出码可断言）。
    Case {
        name: "global_asm_call",
        body: "unsafe extern \"C\" { fn forge_gas_add(a: i32, b: i32) -> i32; } unsafe { forge_gas_add(20, 22) }",
        extra: "core::arch::global_asm!(\".globl forge_gas_add\", \".p2align 4\", \"forge_gas_add:\", \"mov eax, ecx\", \"add eax, edx\", \"ret\");",
        expected: 42,
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "M2 global_asm",
        reason: "global_asm! 定义符号 + Rust 侧 extern 调用（v27 M2 落地）",
    },
    // ── v27 M2 收尾：`global_asm!` 的 `sym` 与 `const` 占位符 ──
    //   `sym` 走 crate 内同一份 `symbol_name_for_instance`（与函数符号同源）；
    //   `const` 按常量类型的符号性格式化成十进制（有符号 ⇒ i128）。
    Case {
        name: "global_asm_sym_const",
        body: "unsafe extern \"C\" { fn forge_sym_const() -> i32; } unsafe { forge_sym_const() }",
        extra: "#[unsafe(no_mangle)]\npub extern \"C\" fn forge_sym_target() -> i32 { 0 }\ncore::arch::global_asm!(\".globl forge_sym_const\", \"forge_sym_const:\", \"lea rax, [rip + {f}]\", \"mov eax, {c}\", \"ret\", f = sym forge_sym_target, c = const 42);",
        expected: 42,
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "M2 global_asm",
        reason: "global_asm! 的 sym/const 占位符（v27 M2 收尾落地）",
    },
];

/// 生成 no_std 程序源码。
fn make_source(body: &str, extra: &str, entry: &str) -> String {
    format!(
        "#![no_std]\n#![no_main]\n{extra}\n#[unsafe(no_mangle)]\npub extern \"C\" fn {entry}() -> i32 {{\n    {body}\n}}\n\nunsafe extern \"system\" {{\n    #[link_name = \"ExitProcess\"]\n    fn forge_panic_exit(code: u32) -> !;\n}}\n#[panic_handler]\nfn panic(_info: &core::panic::PanicInfo) -> ! {{\n    unsafe {{ forge_panic_exit({PANIC_ABORT_CODE}) }}\n}}\n"
    )
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

fn run_case(case: &Case, workdir: &Path) -> Result<i32, String> {
    run_case_with(case, workdir, &[], &[])
}

/// run_case 的可注入变体（M4：真并行池验证用）：
/// - `extra_rustc_args`：追加到 rustc 命令行（如 `["-Z", "threads=2"]`）；
/// - `extra_envs`：追加到 rustc 子进程环境（如 `FORGE_CODEGEN_THREADS=4`）。
fn run_case_with(
    case: &Case,
    workdir: &Path,
    extra_rustc_args: &[&str],
    extra_envs: &[(&str, &str)],
) -> Result<i32, String> {
    let src = workdir.join(format!("{}.rs", case.name));
    let exe = workdir.join(format!("{}.exe", case.name));
    std::fs::write(&src, make_source(case.body, case.extra, case.entry))
        .map_err(|e| e.to_string())?;

    // 编译：no_std + no_main + panic=abort。overflow-checks 默认 on——checked
    // 算术（AddWithOverflow → (value,bool) 元组 + Assert）已支持（A3 验证：
    // 运行期输入不溢出路径正确），`overflow_checks_on` 用例守护；个别历史
    // 用例依赖 wrapping 语义（如 i64 边界）保留 off 编译。
    // entry=="main"（fn main 形态）：不指定 /ENTRY，MSVC 默认入口自动转调 main
    let entry_args = if case.entry == "main" {
        "/SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib".to_string()
    } else {
        "/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib".to_string()
    };
    // 用例级 panic 策略（v27 M2）：名为 `*panic_unwind*` 的用例**不**加 `-C panic=abort`
    // ——否则后端那条 `[cap:UNWIND]` 拒绝路径永远不会被走到（负向用例就无从断言）。
    let mut rustc_args = vec![
        "-Zcodegen-backend=".to_string() + &backend_dll().display().to_string(),
        "--edition".to_string(),
        "2024".to_string(),
    ];
    if !case.name.contains("panic_unwind") {
        rustc_args.push("-C".to_string());
        rustc_args.push("panic=abort".to_string());
    }
    // 用例级溢出开关：`overflow_off` 用例（需要 wrapping 语义的）显式关闭。
    if case.name.contains("overflow_off") {
        rustc_args.push("-C".to_string());
        rustc_args.push("overflow-checks=off".to_string());
    }
    // C1/C2 DebugInfo：`debuginfo_line_tables` 加 `-C debuginfo=1`
    //（line-tables）；`debuginfo_full` 加 `-C debuginfo=2`（full——变量 DIE）。
    if case.name == "debuginfo_full" {
        rustc_args.push("-C".to_string());
        rustc_args.push("debuginfo=2".to_string());
    } else if case.name.starts_with("debuginfo_") {
        rustc_args.push("-C".to_string());
        rustc_args.push("debuginfo=1".to_string());
    }
    // M4 注入：真并行池编译（-Z threads 开启 rustc 前端并行池 → forge 的
    // par_map 函数任务才会上池，见 backend.rs 门控）。
    rustc_args.extend(extra_rustc_args.iter().map(|s| s.to_string()));
    let mut cmd = Command::new("rustc");
    cmd.args(&rustc_args)
        .arg("-C")
        .arg(format!("link-args={entry_args}"))
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .envs(extra_envs.iter().copied());
    let compile = cmd
        .output()
        .map_err(|e| format!("failed to spawn rustc: {e}"))?;

    if !compile.status.success() {
        let stderr = String::from_utf8_lossy(&compile.stderr);
        // 负向用例（expect_compile_fail）：编译失败是预期结果——校验 stderr
        // 含 expect_compile_err 关键词（A4/B3 门控断言）。
        if case.expect_compile_fail {
            let want = case.expect_compile_err;
            if !want.is_empty() && !stderr.contains(want) {
                return Err(format!(
                    "compile failed but stderr lacks expected error {:?}:\n{}",
                    want,
                    stderr.lines().take(10).collect::<Vec<_>>().join("\n")
                ));
            }
            return Err("__EXPECTED_COMPILE_FAIL__".to_string());
        }
        return Err(format!(
            "compile failed: {}\n{}",
            compile.status,
            stderr.lines().take(10).collect::<Vec<_>>().join("\n")
        ));
    }
    if case.expect_compile_fail {
        return Err(format!(
            "expected compile failure but compilation succeeded (门控失效)"
        ));
    }
    // 诊断：FORGE_E2E_TRACE=1 时编译成功也打印 stderr（FORGE_TRACE_* 输出）
    if std::env::var_os("FORGE_E2E_TRACE").is_some() {
        let stderr = String::from_utf8_lossy(&compile.stderr);
        if !stderr.trim().is_empty() {
            eprintln!("--- {} trace ---\n{stderr}\n--- end ---", case.name);
        }
    }

    // 运行并取退出码（带超时）。v27 M1 起 panic handler 是 `ud2`（异常终止）
    // ⇒ assert 失败不再表现为挂起，而是 `None` 退出码（见 run_exe_with_timeout）。
    let mut result = run_exe_with_timeout(&exe);
    // 超时重试（2026-09-10 取证，见 docs/archive/forge-rustc-vec_push-plan.md §9.2）：
    // 本机 8 并发负载下复现过该假败——首次运行 15 s 未退出，而**同一产物**事后独立
    // 复跑 20/20 正确退出（≈100 ms/次）⇒ 超时多为宿主侧起进程/映像就绪延迟
    // （新写出的 .exe + 负载），不是产物错码。同一产物复跑一次即可区分：
    // 真挂起（产物确定性 panic loop）两次都会超时，仍照常上报。
    if let Err(ref e) = result
        && e.contains(TIMEOUT_MARKER)
    {
        println!(
            "RETRY {:<16} 首次运行超时（疑宿主侧起进程/映像延迟），同产物复跑一次",
            case.name
        );
        record_event("RETRY", case.name, "首次运行超时，同产物复跑一次");
        result = run_exe_with_timeout(&exe);
    }
    // v27 M1（trap 语义）：把"异常终止"与用例期望对齐，并**保留真实 NTSTATUS**
    //   ——`expect_trap` 用例的 `expected` 直接写它要钉的那条 trap 码
    //   （如 `0xC000_001D` = `ud2`），这样"trap 发生了"与"trap 的是**这个**机制"都被断言。
    //   `expect_trap = false` 时异常终止 = **未预期的崩溃**（此前被误报成超时）。
    match result {
        // 正常退出码（含普通非零码；NTSTATUS error 级见下支）
        Ok(c) if !is_trap_code(c) => {
            if !case.expect_trap {
                Ok(c)
            } else if c == TRAP_EXIT {
                // 期望的就是"断言失败边分流 ⇒ panic ⇒ ExitProcess(101)"
                record_event("TRAP", case.name, &format!("panic abort exit={c}（预期）"));
                Ok(c)
            } else {
                Err(format!(
                    "期望 trap（panic abort = {TRAP_EXIT}），实际正常退出 exit={c}——断言失败边没触发"
                ))
            }
        }
        // 异常终止：`expect_trap` ⇒ 成功并返回真实码；否则未预期崩溃
        Ok(c) => {
            if case.expect_trap {
                record_event(
                    "TRAP",
                    case.name,
                    &format!("异常终止 NTSTATUS 0x{c:08X}（预期）"),
                );
                Ok(c)
            } else {
                Err(format!(
                    "未预期的异常终止（NTSTATUS 0x{c:08X}：trap/AV/ud2）——产物崩溃了"
                ))
            }
        }
        Err(ref e) if e.contains(TIMEOUT_MARKER) && case.expect_trap => Err(format!(
            "期望 trap（异常终止）但产物挂起——断言失败边没有触发：{e}"
        )),
        other => other,
    }
}

/// 超时错误标记：`run_exe_with_timeout` 产生、`run_case_with` 据此做一次「同产物复跑」
/// 以剔除宿主侧起进程/映像延迟造成的假败（真挂起会两次都超时，仍上报）。
const TIMEOUT_MARKER: &str = "timeout (挂起";

/// **异常终止**标记（v27 M1）：进程已终止但 `status.code()` 为 `None`（trap/AV/`ud2`）。
/// 与 [`TIMEOUT_MARKER`] 分开是本里程碑的关键——两者此前混在一个分支里，
/// 于是"断言真的拦住了溢出"看起来像"产物挂起"。
const TRAP_MARKER: &str = "trap (异常终止";

/// Windows NTSTATUS 的 severity 位：`0xC0000000` 及以上 = 异常终止（error 级）。
///
/// 实测取值：`ud2` = `0xC000001D`（ILLEGAL_INSTRUCTION）、AV = `0xC0000005`、
/// `__fastfail` = `0xC0000409`、栈溢出 = `0xC00000FD`、除零 = `0xC0000094`。
/// **不要**改用"`status.code()` 为 `None`"来判 trap——Windows 上 `ud2` 有码，
/// 那个假设实测为假（v27 M1 第一次跑就证伪）。
fn is_trap_code(code: i32) -> bool {
    (code as u32) >= 0xC000_0000
}

/// `FORGE_E2E_STRICT_FLAKY=1`：FLAKY 用例**只容忍超时 + CI 运行环境 AV**（两者均已
/// 定性为宿主侧环境性、且有同产物复跑兜底），**其余错码改按 FAIL 硬失败**。
/// 用于「5 例的正确性是否已可进门禁」的定向验证与转正就绪度评估；默认关闭。
/// 2026-09-12 起 [`FLAKY`] 为空（5 例已转正）⇒ 本开关与 `CI-ENV-AV` 容忍路径
/// 当前**不生效**（等价于全量门禁），保留以备将来出现新的机器相关偶发。
fn strict_flaky() -> bool {
    std::env::var_os("FORGE_E2E_STRICT_FLAKY").is_some()
}

/// 事件留痕文件：libtest **默认捕获通过的测试输出**（失败才回放），所以"被容忍的"
/// `KNOWN` / `CI-ENV-AV` / `RETRY` 在 CI 日志里原本是**看不见的**——那样就没法回答
/// "本轮到底有没有踩到环境性 AV/超时"。设 `FORGE_E2E_EVENTS=<路径>` 时把关键事件与
/// 汇总追加到该文件，CI 用 `if: always()` 步骤打印出来（见 .github/workflows/ci.yml）。
fn record_event(kind: &str, case: &str, detail: &str) {
    // 落盘实现只有一份（v26 M1 下沉）：本函数只决定格式（`[<事件>] <用例> <细节>`，CI 依赖）。
    code_forge::forge_isa_runtime::diagnostics::emit_text(
        "e2e",
        &format!("[{kind}] {case} {detail}"),
    );
}

/// CI 运行环境 AV 签名：`0xC0000005` = `-1073741819`。
///
/// 2026-09-11 双证（见计划 §9.5）：`vec_push` / `vec_iter_enumerate` 在 CI 上
/// **稳定 AV**（run #25/#26/#27），而两例的 `.text` SHA256 与大小**与本机逐字节
/// 相同**（`a91b9eef…63fb` / `5f421f08…c732`）且本机复跑正确（exit=2/80）
/// ⇒ 同一份机器码只在 CI runner 上崩 ⇒ 环境性（同 `cli_tests` alloc AV 先例）。
/// 严格模式下该签名按 KNOWN 容忍并打印 `CI-ENV-AV` 标记（不静默），其余错码仍硬失败。
///
/// **2026-09-12 收尾**：双证结论已升级为**已定位的 codegen 缺陷**（WA-42：niche tag
/// 写入宽度——AMD runner 栈残留高位恒非零 ⇒ 必然 AV；`.text` 相同而结论不同正说明
/// 变量是**栈残留内容**而非机器码）。5 例转正后不再有容忍对象，本常量仅作留痕/回归
/// 对照保留（勿据此把新的 AV 判为"环境性"——先查 WA-42 类宽度/判空路径）。
const CI_ENV_AV_CODE: i32 = -1073741819;

/// 运行产物并取退出码。三种结局分开返回（v27 M1）：
/// - 正常退出 ⇒ `Ok(码)`；
/// - **异常终止**（trap/AV/`ud2`，`status.code()` 为 `None`）⇒ 含 [`TRAP_MARKER`] 的错误；
/// - 超时（默认 15 s，`FORGE_E2E_TIMEOUT_SECS` 可覆盖，用于区分「真挂起」与「负载下起得慢」）
///   ⇒ 杀掉子进程并返回含 [`TIMEOUT_MARKER`] 的错误。
fn run_exe_with_timeout(exe: &Path) -> Result<i32, String> {
    let secs: u64 = std::env::var("FORGE_E2E_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15);
    let mut child = Command::new(exe)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", exe.display()))?;
    let mut settled: Option<Option<i32>> = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            settled = Some(status.code());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    match settled {
        // **实测（v27 M1）**：Windows 上 `ud2` 走的是**有码**退出
        // （`0xC000001D` = STATUS_ILLEGAL_INSTRUCTION，`status.code()` = `Some(-1073741795)`）
        // ⇒ 本函数不在这里分类"正常/trap"，把码原样交给 `run_case_with`
        // （它按 [`is_trap_code`] 的 NTSTATUS 语义判，并被 `expect_trap` 归一）。
        // 此前这里把"异常终止"与"超时"混在一个分支 ⇒ **trap 被误报成挂起**。
        Some(Some(c)) => Ok(c),
        // 已终止但拿不到码 = 被宿主强行结束（也是异常终止）⇒ 用通用 trap 码哨兵。
        Some(None) => Ok(TRAP_EXIT),
        None => {
            let _ = child.kill();
            Err(format!(
                "{TIMEOUT_MARKER}：{secs} s 内未退出——产物真挂起（无 trap、无退出）"
            ))
        }
    }
}

/// **M1 不变量守卫（v27）**：把 trap 语义与"零容忍名单"钉死在源码上。
///
/// 为什么需要它：本文件里的这三件事**改坏了不会红**——
/// ① panic handler 退回 `loop {}` ⇒ `expect_trap` 用例从"异常终止"变回"超时"，只是慢一点；
/// ② 新加用例又写 `known_failure: true` ⇒ 套件照绿，容忍名单悄悄长回来；
/// ③ `expect_trap` 用例的 `expected` 不是 trap 码（如又被写成 `i32::MIN`）⇒ 与"静默回绕"
///    撞车，用例形同虚设（v27 M1 第一次就踩过这个坑）。
#[test]
fn e2e_trap_semantics_invariants() {
    let src = include_str!("e2e.rs");
    let code_lines: Vec<&str> = src
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect();

    // ① **只扫模板切片**（`make_source` 的 `format!` 串）：既避免"针写在守卫自己身上"
    //    的自我命中（本轮第三次踩），也比扫全文件精确——校验的就是模板契约本身。
    let tpl_start = src.find("fn make_source").expect("make_source 存在");
    // 切片 = `make_source` 起、到下一个顶层函数（`fn backend_dll`）之前；
    // 不要按 `\n}\n` 找结尾——模板函数的收尾是缩进的 `    }`。
    let tpl_end = src[tpl_start..]
        .find("\nfn ")
        .map(|i| tpl_start + i)
        .expect("模板函数后还有下一个函数");
    let tpl = &src[tpl_start..tpl_end];
    // ①a panic 出口必须是**确定性 abort**（`forge_panic_exit(PANIC_ABORT_CODE)`）。
    assert!(
        tpl.contains("forge_panic_exit({PANIC_ABORT_CODE})"),
        "模板的 panic 出口不是确定性 abort ⇒ expect_trap 用例会退化成超时"
    );
    // ①b 模板自身**不许用内联 asm**：v27 M2 起内联 asm 是 fail-closed（`[cap:INLINE_ASM]`），
    //     模板一旦用它，全套用例会立刻编不过（M1→M2 的实测教训）。
    assert!(
        !tpl.contains("asm!"),
        "模板不许用内联 asm（M2 起 fail-closed）——panic 出口请用 forge_panic_exit"
    );

    // ② 不许有待转正的 known_failure（注释里的历史说明不算）
    //
    // **自我命中陷阱**（与 v26 的事件守卫同源）：本守卫自己在 `include_str!` 的扫描面里，
    // 所以"要找的字面量"不能用连续字符串写出来（否则守卫永远报自己）。用 `concat!`
    // 在**编译期**拼出针，源码里就不存在那个连续形态。
    let needle = concat!("known_failure", ":", " true");
    let still_known: Vec<&&str> = code_lines.iter().filter(|l| l.contains(needle)).collect();
    assert!(
        still_known.is_empty(),
        "e2e 里还有 {} 命中（{} 处）——转正必须翻标记：{still_known:?}",
        needle,
        still_known.len()
    );

    let code = code_lines.join("\n");

    // ③ 每个 expect_trap 用例必须在同块内钉一个真实 trap 码（NTSTATUS ≥ 0xC0000000）
    let lines: Vec<&str> = code.lines().collect();
    let mut trap_cases = 0usize;
    // 同样用 `concat!` 拼针（否则会命中本守卫自己那一行）
    let trap_needle = concat!("expect_trap", ":", " true");
    for (i, l) in lines.iter().enumerate() {
        if !l.contains(trap_needle) {
            continue;
        }
        trap_cases += 1;
        let lo = i.saturating_sub(5);
        let hi = (i + 5).min(lines.len());
        let want = lines[lo..hi]
            .iter()
            .find(|w| w.trim_start().starts_with("expected:"))
            .unwrap_or_else(|| panic!("expect_trap 用例缺 expected 行（第 {} 行附近）", i + 1));
        // 期望码两种合法形态：`TRAP_EXIT`（panic abort 码）或显式 NTSTATUS 字面量
        // `0x… as i32`（硬件异常终止）。
        let code_v = if want.contains("TRAP_EXIT") {
            TRAP_EXIT
        } else {
            let hex = want.split("0x").nth(1).unwrap_or_else(|| {
                panic!("expect_trap 的 expected 必须是 `TRAP_EXIT` 或 `0x… as i32`：{want}")
            });
            let digits: String = hex
                .chars()
                .take_while(|c| c.is_ascii_hexdigit() || *c == '_')
                .collect();
            i64::from_str_radix(&digits.replace('_', ""), 16)
                .unwrap_or_else(|e| panic!("解析 trap 码失败 {digits:?}: {e}")) as i32
        };
        assert!(
            code_v == TRAP_EXIT || is_trap_code(code_v),
            "expect_trap 的 expected（{code_v}）既不是 panic abort 码（{TRAP_EXIT}）\
             也不是 NTSTATUS trap 码（≥ 0xC0000000）——会与正常退出（含溢出回绕）撞车：{want}"
        );
    }
    assert!(
        trap_cases > 0,
        "一个 expect_trap 用例都没有——trap 语义没有真值覆盖"
    );
    println!("OK  e2e_trap_semantics_invariants：trap 用例 {trap_cases} 个、known_failure 0 个");
}

/// 双向 flake 用例名单（**2026-09-12 起为空 = 全量门禁**）。
///
/// 用途（历史）：名单内 `known_failure:true` 的用例"时过时败"——偶发 PASS 会触发
/// 反断言（防 stale），偶发 AV 又会红 CI，故按双向容忍留痕（PASS 计数、FAIL 打印
/// `KNOWN` 不致命）。转正 = 移出名单 + 翻 `known_failure=false`。
///
/// **2026-09-12 转正收官**：原 5 例（vec_push / vec_string / vec_iter_enumerate /
/// vec_from_slice / box_value）的 CI 机型相关 AV 与 WA-41 同源，根因 = **WA-42**
/// （niche 枚举 tag 写入宽度）→ 全部移出名单并翻标记。CI run 93927221004 的
/// AMD Family 25 Model 1 runner 上 `passed=103/103 known=[]`、alloc 探针 21/21 全 ok
/// （该 runner 修复前 20/20 AV），本机 hammer 5 轮 103/103 + 5 例各 ×10 全过
/// （计划 §9.7 / WORKAROUNDS WA-42）。
///
/// 机制保留：将来若再出现**机器相关**的偶发（而非错码），把用例名放回本名单即可
/// 重新容忍；`FORGE_E2E_STRICT_FLAKY` 亦只在名单非空时有意义。
const FLAKY: &[&str] = &[];

#[test]
fn e2e_stage_a_scalar_cases() {
    let dll = backend_dll();
    let workdir = std::env::temp_dir().join(format!("forge_rustc_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create workdir");
    println!("backend: {}", dll.display());
    println!("workdir: {}", workdir.display());

    let mut passed = 0;
    let mut known_failures = Vec::new();
    let mut unexpected_failures = Vec::new();
    // 失败案件名（用于 FORGE_E2E_KEEP 保留工作目录取证，见 §9.1）
    let mut failed_cases: Vec<&str> = Vec::new();
    // 单用例调试：FORGE_E2E_ONLY=用例名 只跑一个（配合 FORGE_TRACE_*）
    let only = std::env::var("FORGE_E2E_ONLY").ok();

    for case in CASES {
        if let Some(ref only_name) = only
            && only_name != case.name
        {
            continue;
        }
        match run_case(case, &workdir) {
            Ok(code) if code == case.expected => {
                // P0-20：known_failure 转正必须显式翻转标记——否则 bug 修复
                // 后无人更新（known_failure 永不 assert，CI 照绿）。
                if case.known_failure {
                    if FLAKY.contains(&case.name) {
                        // 双向 flake：本轮偶发通过——按 PASS 计数不触发反断言
                        println!(
                            "FLAKY-PASS {:<12} exit={}（已知偶发；reason 见下）",
                            case.name, code
                        );
                        passed += 1;
                        continue;
                    }
                    panic!(
                        "KNOWN-FAILURE TURNED PASS: `{}` now exits {} (expected)——请移除 known_failure 标记并更新 reason",
                        case.name, code
                    );
                }
                println!("PASS  {:<16} exit={}", case.name, code);
                passed += 1;
            }
            // 负向用例（A4/B3）：编译失败哨兵 = PASS
            Err(e) if e == "__EXPECTED_COMPILE_FAIL__" => {
                println!("PASS  {:<16} compile-fail (gated)", case.name);
                passed += 1;
            }
            Ok(code) => {
                let msg = format!("exit={code} (want {})", case.expected);
                // 严格模式（FORGE_E2E_STRICT_FLAKY=1）：FLAKY 用例只容忍**超时**与
                // **CI 运行环境 AV**（两者都已有双证/复跑兜底），其余错码按 FAIL
                // 上报——错码是错编译的 signature，用它做转正就绪度的定向验证。
                // 默认关闭 ⇒ CI 现有行为不变。
                let flaky_strict = strict_flaky() && FLAKY.contains(&case.name);
                let env_av = flaky_strict && code == CI_ENV_AV_CODE;
                if case.known_failure && (!flaky_strict || env_av) {
                    if env_av {
                        // 显式标记：不静默（同产物 .text 与本机逐字节一致，见 §9.5）
                        println!(
                            "KNOWN {:<16} {msg} [{}] CI-ENV-AV: CI runner 环境性 AV（产物 .text 与本机逐字节一致，见计划 §9.5）",
                            case.name, case.phase
                        );
                        record_event("CI-ENV-AV", case.name, &msg);
                    } else {
                        // 回归探针（P3.3）：输出失败模式（reason），转正时对照验证
                        println!(
                            "KNOWN {:<16} {msg} [{}] reason: {}",
                            case.name, case.phase, case.reason
                        );
                        record_event("KNOWN-WRONGCODE", case.name, &msg);
                    }
                    known_failures.push(case.name);
                    failed_cases.push(case.name);
                } else {
                    println!("FAIL  {:<16} {msg}", case.name);
                    record_event("FAIL", case.name, &msg);
                    unexpected_failures.push(format!("{}: {msg}", case.name));
                    failed_cases.push(case.name);
                }
            }
            Err(e) => {
                let msg = format!("error: {e}");
                if case.known_failure {
                    println!(
                        "KNOWN {:<16} {msg} [{}] reason: {}",
                        case.name, case.phase, case.reason
                    );
                    record_event(
                        if e.contains(TIMEOUT_MARKER) {
                            "KNOWN-TIMEOUT"
                        } else {
                            "KNOWN-ERR"
                        },
                        case.name,
                        &msg,
                    );
                    known_failures.push(case.name);
                    failed_cases.push(case.name);
                } else {
                    println!("FAIL  {:<16} {msg}", case.name);
                    record_event("FAIL", case.name, &msg);
                    unexpected_failures.push(format!("{}: {msg}", case.name));
                    failed_cases.push(case.name);
                }
            }
        }
    }

    // 失败取证：FORGE_E2E_KEEP=1 时**保留工作目录**（否则产物在清理时丢失，
    // 无法做 §9.1 的"CI/本机产物字节对照"）。目录内每用例留下 `<name>.rs` 与
    // `<name>.exe`（配合 FORGE_TRACE_* / FORGE_E2E_TRACE 的 stderr 即完整证据）。
    if failed_cases.is_empty() || std::env::var_os("FORGE_E2E_KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&workdir);
    } else {
        println!(
            "\n[keep] 失败用例 {:?} —— 工作目录保留：{}（.rs/.exe 可复跑对照）",
            failed_cases,
            workdir.display()
        );
    }

    println!("\n=== e2e 汇总 ===");
    println!("passed: {passed}/{}", CASES.len());
    record_event(
        "SUMMARY",
        "stage_a",
        &format!(
            "passed={passed}/{} known={:?} unexpected={}",
            CASES.len(),
            known_failures,
            unexpected_failures.len()
        ),
    );
    if !known_failures.is_empty() {
        println!("known failures (待 Phase 2/3): {:?}", known_failures);
    }
    if !unexpected_failures.is_empty() {
        panic!("unexpected failures:\n{}", unexpected_failures.join("\n"));
    }
    assert!(passed > 0, "no cases passed — backend broken");
}

/// 分配路径逐步探针（**诊断用，不做断言**；2026-09-11 加）。
///
/// 背景：CI 上 `vec_push` / `vec_iter_enumerate` 在**某些 runner 实例**上
/// 20/20 稳定 AV（`0xC0000005`），而 `.text` 与本机逐字节相同、本机 0 AV
/// （见 docs/archive/forge-rustc-vec_push-plan.md §9.5）。WER 在该 runner 上不留记录，
/// 所以改用"逐步裁剪 + 对照变体"定位崩溃点：
///
/// - `probe_new_only`：只 `Vec::new()`（不分配）；
/// - `probe_one_push`：首次 push（触发 0→4 的 grow = 一次 alloc）；
/// - `probe_two_push` / `probe_six_push`：多次 push（后续 grow，多次 alloc/realloc）；
/// - `probe_iter_enum`：push×3 + 迭代（= `vec_iter_enumerate`）；
/// - `*_bump`：同样程序但分配器**每次返回不同地址**（bump），用来验证
///   "同一地址反复分配（naive 分配器）是否就是触发条件"。
///
/// 本机预期全部 exit 正确；受影响的 runner 上会看到某一步 AV —— 哪一步先崩，
/// 崩溃点就落在哪条路径上。
///
/// **2026-09-12 结案（WA-42）**：本探针已把崩溃点收敛到 grow 链（`probe_one_push`
/// 即崩 ⇒ 首次 alloc 的 `finish_grow → grow_impl_runtime` 路径），根因是 niche
/// 枚举 `None` 的 tag 只写 4 字节、判别读 8 字节（`current_memory` 误返 `Some`）。
/// 修复后同一 AMD runner（Family 25 Model 1）上 **21/21 变体全部 ok**
/// （此前其中 8 个 20/20 AV）——见计划 §9.7。探针保留：它是"机器相关崩溃面"的
/// 现成指纹，将来换机型时可一眼看出是否同类。
#[test]
fn e2e_alloc_step_probe() {
    let workdir =
        std::env::temp_dir().join(format!("forge_rustc_alloc_probe_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create workdir");
    println!("workdir: {}", workdir.display());

    // 与 vec_push 等用例完全一致的 naive 分配器：**每次 alloc 都返回同一地址**。
    const NAIVE_ALLOC: &str = "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        base.add(base.align_offset(32))\n    } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;";
    // bump 分配器：每次 alloc 前进游标、返回**不同**地址（dealloc 仍是 no-op）。
    const BUMP_ALLOC: &str = "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstatic mut CUR: usize = 0;\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, l: Layout) -> *mut u8 { unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        let cur = core::ptr::addr_of_mut!(CUR).read();\n        let align = if l.align() < 16 { 16 } else { l.align() };\n        let off = (cur + align - 1) & !(align - 1);\n        let next = off + l.size();\n        if next > 8192 { return core::ptr::null_mut(); }\n        core::ptr::addr_of_mut!(CUR).write(next);\n        base.add(off)\n    } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;";

    const NEW_ONLY: &str = "let v: alloc::vec::Vec<u8> = alloc::vec::Vec::new(); v.len() as i32";
    const ONE_PUSH: &str = "let mut v = alloc::vec::Vec::new(); v.push(1); v.len() as i32";
    const TWO_PUSH: &str =
        "let mut v = alloc::vec::Vec::new(); v.push(1); v.push(2); v.len() as i32";
    const SIX_PUSH: &str = "let mut v = alloc::vec::Vec::new(); let mut i = 0; while i < 6 { v.push(i); i += 1; } v.len() as i32";
    const ITER_ENUM: &str = "let mut v = alloc::vec::Vec::new(); v.push(10); v.push(20); v.push(30); let mut s = 0; for (i, x) in v.iter().enumerate() { s += i as i32 * x; } s";

    // 「审计分配器」：记录每次 alloc 的请求尺寸，并在返回块之后**投毒** 32 字节，
    // 供用例回读校验。用途：在**本机**确定性地检测两件本来只在 CI 机型上暴露的事——
    // ①生成代码是否向分配器要了**错误的尺寸/对齐**（尺寸错了，naive 分配器也照样"成功"，
    //   随后按错误尺寸写/拷 → 在别的内存布局上就是 AV / 0xC0000409 栈缓冲区破坏）；
    // ②push 路径是否越界写到分配块之外（投毒字节被改写）。
    // 记录数组：ASIZE[0..8] 尺寸、AALIGN[0..8] 对齐、APTR0 首次返回的指针、AN 次数。
    const AUDIT_ALLOC: &str = "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstatic mut CUR: usize = 0;\nstatic mut ASIZE: [usize; 8] = [0; 8];\nstatic mut AALIGN: [usize; 8] = [0; 8];\nstatic mut APTR: [*mut u8; 8] = [core::ptr::null_mut(); 8];\nstatic mut AN: usize = 0;\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, l: Layout) -> *mut u8 { unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        let cur = core::ptr::addr_of_mut!(CUR).read();\n        let align = if l.align() < 16 { 16 } else { l.align() };\n        let off = (cur + align - 1) & !(align - 1);\n        let size = l.size();\n        if off + size + 32 > 8192 { return core::ptr::null_mut(); }\n        let n = core::ptr::addr_of_mut!(AN).read();\n        if n < 8 {\n            (*core::ptr::addr_of_mut!(ASIZE))[n] = size;\n            (*core::ptr::addr_of_mut!(AALIGN))[n] = l.align();\n        }\n        core::ptr::addr_of_mut!(AN).write(n + 1);\n        let p = base.add(off);\n        if n < 8 { (*core::ptr::addr_of_mut!(APTR))[n] = p; }\n        let mut i = 0;\n        while i < 32 { p.add(size + i).write_volatile(0xA5); i += 1; }\n        core::ptr::addr_of_mut!(CUR).write(off + size + 32);\n        p\n    } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;";

    // 首次 push（空 Vec grow-from-empty）实际向分配器要多少字节？Vec<i32> 期望
    // cap=4 × 4B = 16；Vec<u8> 期望 cap=8 × 1B = 8；第五次 push 时 4→8 应要 32。
    const AUDIT_ONE_I32: &str = "let mut v: alloc::vec::Vec<i32> = alloc::vec::Vec::new(); v.push(1); unsafe { (*core::ptr::addr_of!(ASIZE))[0] as i32 }";
    const AUDIT_ONE_U8: &str = "let mut v: alloc::vec::Vec<u8> = alloc::vec::Vec::new(); v.push(1); unsafe { (*core::ptr::addr_of!(ASIZE))[0] as i32 }";
    const AUDIT_GROW_2: &str = "let mut v: alloc::vec::Vec<i32> = alloc::vec::Vec::new(); let mut i = 0; while i < 5 { v.push(i); i += 1; } unsafe { (*core::ptr::addr_of!(ASIZE))[1] as i32 }";
    const AUDIT_ALIGN: &str = "let mut v: alloc::vec::Vec<i32> = alloc::vec::Vec::new(); v.push(1); unsafe { (*core::ptr::addr_of!(AALIGN))[0] as i32 }";
    // 投毒校验：push 若干次后回读每个分配块之后的 32 字节；全为 0xA5 ⇒ 返回 1，
    // 否则返回 1000+首个被改写的偏移（本机若出现 1000+ ⇒ 生成代码越界写）。
    const AUDIT_POISON: &str = "let mut v: alloc::vec::Vec<i32> = alloc::vec::Vec::new(); let mut i = 0; while i < 5 { v.push(i); i += 1; } unsafe {\n        let n = core::ptr::addr_of!(AN).read();\n        let mut k = 0; let mut bad = -1;\n        while k < n && k < 8 {\n            let p = (*core::ptr::addr_of!(APTR))[k] as *const u8;\n            let sz = (*core::ptr::addr_of!(ASIZE))[k];\n            let mut j = 0;\n            while j < 32 {\n                if p.add(sz + j).read_volatile() != 0xA5 { bad = (k * 100 + j) as i32; break; }\n                j += 1;\n            }\n            if bad >= 0 { break; }\n            k += 1;\n        }\n        if bad < 0 { 1 } else { 1000 + bad }\n    }";
    // 仅回读指针数组第 0 项与 HEAP 起点的差（期望 0）：检验"指针写入 static mut 数组再读回"。
    const AUDIT_PTR_RT: &str = "let mut v: alloc::vec::Vec<i32> = alloc::vec::Vec::new(); v.push(1); unsafe {\n        let p = (*core::ptr::addr_of!(APTR))[0] as usize;\n        let base = core::ptr::addr_of!(HEAP) as usize;\n        p.wrapping_sub(base) as i32\n    }";
    // 只做一次"计算地址读取"：读首个分配块末尾（毒区首字节，期望 0xA5=165）。
    const AUDIT_READ_AT_PTR: &str = "let mut v: alloc::vec::Vec<i32> = alloc::vec::Vec::new(); v.push(1); unsafe {\n        let p = (*core::ptr::addr_of!(APTR))[0] as *const u8;\n        let sz = (*core::ptr::addr_of!(ASIZE))[0];\n        p.add(sz).read_volatile() as i32\n    }";
    // 同上但**不读内存**：只算地址并返回相对偏移（期望 16）——分离"地址计算"与"载入"。
    const AUDIT_ADDR_CALC: &str = "let mut v: alloc::vec::Vec<i32> = alloc::vec::Vec::new(); v.push(1); unsafe {\n        let p = (*core::ptr::addr_of!(APTR))[0] as *const u8;\n        let sz = (*core::ptr::addr_of!(ASIZE))[0];\n        let base = core::ptr::addr_of!(HEAP) as usize;\n        (p as usize).wrapping_add(sz).wrapping_sub(base) as i32\n    }";
    // 读同一字节但**把值归一化成常量**再返回（期望 165）——分离"载入成功"与"值回传"。
    const AUDIT_READ_ONLY: &str = "let mut v: alloc::vec::Vec<i32> = alloc::vec::Vec::new(); v.push(1); unsafe {\n        let p = (*core::ptr::addr_of!(APTR))[0] as *const u8;\n        let sz = (*core::ptr::addr_of!(ASIZE))[0];\n        let b = p.add(sz).read_volatile();\n        if b == 0xA5 { 165 } else { b as i32 }\n    }";
    // 裸分配（不经过 Vec）：请求 size 字节、写一个字节再返回 1；分配失败返回 -1。
    const RAW4: &str = "unsafe {\n        let l = core::alloc::Layout::from_size_align(4, 4).unwrap();\n        let p = alloc::alloc::alloc(l);\n        if p.is_null() { return -1; }\n        p.write(7);\n        1\n    }";
    const RAW16: &str = "unsafe {\n        let l = core::alloc::Layout::from_size_align(16, 4).unwrap();\n        let p = alloc::alloc::alloc(l);\n        if p.is_null() { return -1; }\n        p.write(7);\n        1\n    }";
    const RAW64: &str = "unsafe {\n        let l = core::alloc::Layout::from_size_align(64, 4).unwrap();\n        let p = alloc::alloc::alloc(l);\n        if p.is_null() { return -1; }\n        p.write(7);\n        1\n    }";
    // 完全不经过分配器：直接写 static HEAP 前 64 字节（隔离"写 HEAP"本身）。
    const HEAP_WRITE: &str = "unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        let mut i = 0;\n        while i < 64 { base.add(i).write_volatile(0x5A); i += 1; }\n        1\n    }";
    // 在**出问题的机器上**取 faulting RIP：装 VEH，异常时以 0xEE0xxxxx|RIP低20位 退出。
    // （本机已验证该基础设施对正常路径无影响。）
    const VEH_ALLOC: &str = "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe {\n        let base = core::ptr::addr_of_mut!(HEAP) as *mut u8;\n        base.add(base.align_offset(32))\n    } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;\n#[repr(C)]\nstruct Er { code: u32, flags: u32, record: *mut Er, address: *mut core::ffi::c_void, num: u32, pad: u32, info: [u64; 15] }\n#[repr(C)]\nstruct Ep { er: *mut Er, ctx: *mut u8 }\n#[link(name = \"kernel32\")]\nunsafe extern \"system\" {\n    fn AddVectoredExceptionHandler(first: u32, handler: unsafe extern \"system\" fn(*mut Ep) -> i32) -> *mut core::ffi::c_void;\n    fn ExitProcess(code: u32) -> !;\n}\nunsafe extern \"system\" fn veh(info: *mut Ep) -> i32 {\n    unsafe {\n        let ctx = (*info).ctx;\n        let rip = *(ctx.add(0xF8) as *const u64);\n        ExitProcess(0xEE00_0000u32 | ((rip & 0x000F_FFFF) as u32))\n    }\n}\n#[inline(never)]\nunsafe fn install_veh() { unsafe { AddVectoredExceptionHandler(1, veh); } }";
    const VEH_ONE_PUSH: &str = "unsafe { install_veh(); }\n    let mut v: alloc::vec::Vec<i32> = alloc::vec::Vec::new();\n    v.push(1);\n    v.len() as i32";
    // 只校验第 0 个块（不跨块、不用 APTR[1]）。
    const AUDIT_GUARD_K0: &str = "let mut v: alloc::vec::Vec<i32> = alloc::vec::Vec::new(); v.push(1); unsafe {\n        let p = (*core::ptr::addr_of!(APTR))[0] as *const u8;\n        let sz = (*core::ptr::addr_of!(ASIZE))[0];\n        let mut j = 0; let mut bad = -1;\n        while j < 32 {\n            if p.add(sz + j).read_volatile() != 0xA5 { bad = j as i32; break; }\n            j += 1;\n        }\n        if bad < 0 { 1 } else { 1000 + bad }\n    }";

    let variants: [(&str, &str, &str, i32); 22] = [
        ("probe_new_only", NEW_ONLY, NAIVE_ALLOC, 0),
        ("probe_one_push", ONE_PUSH, NAIVE_ALLOC, 1),
        ("probe_two_push", TWO_PUSH, NAIVE_ALLOC, 2),
        ("probe_six_push", SIX_PUSH, NAIVE_ALLOC, 6),
        ("probe_iter_enum", ITER_ENUM, NAIVE_ALLOC, 80),
        ("probe_two_push_bump", TWO_PUSH, BUMP_ALLOC, 2),
        ("probe_six_push_bump", SIX_PUSH, BUMP_ALLOC, 6),
        ("probe_audit_one_i32", AUDIT_ONE_I32, AUDIT_ALLOC, 16),
        ("probe_audit_one_u8", AUDIT_ONE_U8, AUDIT_ALLOC, 8),
        ("probe_audit_grow2", AUDIT_GROW_2, AUDIT_ALLOC, 32),
        ("probe_audit_align4", AUDIT_ALIGN, AUDIT_ALLOC, 4),
        ("probe_oob_guard", AUDIT_POISON, AUDIT_ALLOC, 1),
        // ↓ 隔离 `probe_oob_guard` 在本机就 AV 的原因（指针数组回读 / 计算地址读取）
        ("probe_ptr_roundtrip", AUDIT_PTR_RT, AUDIT_ALLOC, 0),
        ("probe_read_at_ptr", AUDIT_READ_AT_PTR, AUDIT_ALLOC, 165),
        ("probe_guard_k0", AUDIT_GUARD_K0, AUDIT_ALLOC, 1),
        // ↓ 继续钉住 probe_read_at_ptr 的 AV：地址计算 vs 载入值流
        ("probe_addr_calc", AUDIT_ADDR_CALC, AUDIT_ALLOC, 16),
        ("probe_read_only", AUDIT_READ_ONLY, AUDIT_ALLOC, 165),
        // ↓ 裸分配 / 直接写 HEAP：把"分配路径"从"Vec 机制"里剥出来（CI 上 AMD 机型
        //   连 `audit_one_i32` 都崩，说明触发面比 Vec 更宽）。
        ("probe_raw_alloc4", RAW4, NAIVE_ALLOC, 1),
        ("probe_raw_alloc16", RAW16, NAIVE_ALLOC, 1),
        ("probe_raw_alloc64", RAW64, NAIVE_ALLOC, 1),
        ("probe_heap_write", HEAP_WRITE, NAIVE_ALLOC, 1),
        // VEH 版：在崩的机器上把 faulting RIP 带回来（0xEE0xxxxx|低20位 ⇒ 见 CI 日志）
        ("probe_veh_one_push", VEH_ONE_PUSH, VEH_ALLOC, 1),
    ];

    for (name, body, extra, want) in variants {
        let case = Case {
            name,
            body,
            extra,
            entry: "mainCRTStartup",
            expected: want,
            expect_compile_fail: false,
            expect_compile_err: "",
            expect_trap: false,
            known_failure: false,
            phase: "probe",
            reason: "分配路径逐步探针（诊断用，不参与门禁）",
        };
        // 诊断优先：任何结果都只打印，不让本测试红（真实门禁在 stage_a）
        // v27 M1：trap（含 AV）现在由 `Err(TRAP_MARKER…)` 表达（进程异常终止），
        // 不再落进 `Ok(码)`——所以 AV 从错误文本里认（消息里带 `0x…` NTSTATUS）。
        let (verdict, detail) = match run_case(&case, &workdir) {
            Ok(code) if code == want => ("ok", format!("exit={code}")),
            Ok(code) => ("WRONG", format!("exit={code} (want {want})")),
            Err(e) if e.contains("0xC0000005") => ("AV", format!("0xC0000005：{e}")),
            Err(e) if e.contains(TRAP_MARKER) => ("TRAP", e),
            Err(e) => ("ERR", format!("error: {e}")),
        };
        println!("PROBE {name:<22} {verdict:<5} {detail}");
    }

    // 对照组（**已撤**）：曾想用"普通 rustc 编同一份源码"做后台无关对照，但
    // rustc 对 `#![no_main]` 不把 `#[no_mangle] mainCRTStartup` 当入口（`--emit=link`
    // 只产出 ~1.5 KB 空桩、退出码恒 0），对照组不成立，故不保留该路径。
    let _ = std::fs::remove_dir_all(&workdir);
}

/// WA-42 回归（2026-09-12 修复）：**宽聚合 payload 的 niche 枚举 `None` 构造
/// 必须按 tag 标量宽度写入**。
///
/// 根因（本机确定性复现 WA-41 的定位结论，见 WORKAROUNDS WA-41/WA-42）：
/// `Option<(NonNull<u8>, Layout)>` 的布局是 24 字节 `Memory` repr、niche 是
/// offset 0 的 8 字节指针字段。枚举构造降级旧的 tag 宽度推导只覆盖
/// `backend_repr = Scalar / ScalarPair`，`Memory` 落进 `_ => 4` 兜底 → 发
/// `store i32 0`（`movl`）只写低 4 字节，**高 4 字节残留栈上旧值**（本机实测
/// `0x00007ffd00000000`）→ 调用方 `finish_grow` 按 8 字节判空失败 → 误取
/// `Some(野指针)` → `grow_impl_runtime` 的 `copy_nonoverlapping` 解引用 → AV
/// （`Vec::new(); v.push(1)` 就崩，即 CI 上 `vec_push`/`vec_iter_enumerate`
/// 机型相关 AV 的同一族缺陷）。
///
/// 断言取 **IR 级**而非行为级：残留字节取决于栈布局（本机 e2e 的 vec 用例
/// 恰好残留 0 → 行为级断言在本机不复现），IR 里 `store i64` / `store i32`
/// 与布局无关且确定 → 该测试是稳定回归门（修复前必红）。
#[test]
fn niche_wide_payload_none_tag_store_uses_tag_width() {
    let dll = backend_dll();
    let workdir = std::env::temp_dir().join(format!("forge_rustc_wa42_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create workdir");
    let src = workdir.join("wa42_niche.rs");
    // wide_none：24 字节 payload + offset 0 的 8 字节指针 niche（Memory repr）。
    // `#[inline(never)]` 保证它是独立函数、IR dump 可单独检索。
    std::fs::write(
        &src,
        r#"#![no_std]
#![no_main]
use core::ptr::NonNull;
#[inline(never)]
#[unsafe(no_mangle)]
pub fn wide_none() -> Option<(NonNull<u8>, usize, usize)> {
    None
}
#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> i32 {
    match wide_none() {
        None => 42,
        Some(_) => 1,
    }
}
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
"#,
    )
    .expect("write source");
    let exe = workdir.join("wa42_niche.exe");
    let out = Command::new("rustc")
        .arg("-Zcodegen-backend=".to_string() + &dll.display().to_string())
        .args(["-C", "panic=abort", "--edition", "2024"])
        .arg("-C")
        .arg(
            "link-args=/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE \
             /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib",
        )
        // IR dump 走 stderr：本测试直接检索它做与栈布局无关的形状断言。
        .env("FORGE_TRACE_IR", "1")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("rustc spawn");
    assert!(
        out.status.success(),
        "WA-42 用例编译失败:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let status = Command::new(&exe).status().expect("run exe");
    assert_eq!(
        status.code(),
        Some(42),
        "WA-42 用例运行错码（None 判别被栈残留污染）"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    let start = stderr
        .find("=== IR dump: wide_none ===")
        .expect("IR dump 缺 wide_none（FORGE_TRACE_IR 未生效？）");
    // 从 dump 头**之后**找下一个 dump 头：直接对 `rest` 再 find 会命中 0 位置。
    let rest = &stderr[start + "=== IR dump:".len()..];
    let end = rest.find("[forge] === IR dump:").unwrap_or(rest.len());
    let fn_ir = &rest[..end];
    assert!(
        fn_ir.contains("store i64 0, ptr"),
        "WA-42 回归：wide_none 的 None tag 必须按 8 字节 tag 宽度写入（store i64）\n{fn_ir}"
    );
    assert!(
        !fn_ir.contains("store i32 0, ptr"),
        "WA-42 回归：wide_none 出现 4 字节 tag 写入（高 4 字节会残留栈垃圾 → 误判 Some）\n{fn_ir}"
    );
    let _ = std::fs::remove_dir_all(&workdir);
}

/// M4（并行路径 A，根治 WA-38）：forge 函数降级任务提交 **rustc 查询池**
/// （`-Z threads>=2` 开启 rustc 并行前端 → backend.rs 门控
/// `FORGE_CODEGEN_THREADS>1 && jobs.frontend.is_some() && 函数数>1` 后以
/// `rustc_data_structures::sync::par_map` 上池并行，见 WORKAROUNDS WA-38）。
/// 这里抽用例逐一以两种方式编译并运行：
/// - 无参编译（无 -Z threads → 旧 T=1 串行路径）；
/// - `-Z threads=2` + `FORGE_CODEGEN_THREADS=4`（真并行池路径）。
/// 两路径退出码必须一致且等于期望值（产物指令序列一致性由 determinism.rs
/// 的 -Z threads 矩阵守护）。
/// ⚠️ 名单不含 alloc/Vec grow 与 V256 用例：2026-09-06 起它们在本测试出现
/// 非确定性残余（vec AV / par 编译偶发 timeout 挂起——见 vec_push reason 与
/// FLAKY 名单），其编译正确性由 stage_a/并行 case 清单与 jit 矩阵覆盖；
/// **2026-09-12 更新**：vec AV 半边已由 WA-42 根治（5 例转正进门禁，见计划 §9.7），
/// 但"par 编译偶发 timeout 挂起"从未定性，且 `-Z threads=2` 下用例会成倍放大编译
/// 负载——故暂仍不并入本名单（避免用未定性的形态换覆盖率）；要并入需先拿该形态的
/// 独立证据（本机 hammer 的 parallel 变体扩名单跑通多轮）后再改。
#[test]
fn e2e_parallel_pool_threads() {
    let workdir = std::env::temp_dir().join(format!("forge_rustc_e2e_par_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create workdir");
    println!("workdir: {}", workdir.display());
    let names = [
        "fib_recursive",
        "simd_v128_call",
        "debuginfo_line_tables",
        "debuginfo_full",
    ];
    for name in names {
        let case = CASES.iter().find(|c| c.name == name).expect("case exists");
        let plain = run_case(case, &workdir)
            .map_err(|e| format!("{name} (T=1): {e}"))
            .expect("plain compile/run");
        let par = run_case_with(
            case,
            &workdir,
            &["-Z", "threads=2"],
            &[("FORGE_CODEGEN_THREADS", "4")],
        )
        .map_err(|e| format!("{name} (-Z threads=2 par_map): {e}"))
        .expect("parallel compile/run");
        assert_eq!(
            plain, case.expected,
            "{name}: T=1 退出码 {plain} ≠ 期望 {}",
            case.expected
        );
        assert_eq!(
            par, plain,
            "{name}: 并行池退出码 {par} ≠ T=1 {plain}——par_map 路径产物行为漂移"
        );
        println!(
            "PASS  parallel-pool {:<16} exit={} (T=1 == -Z threads=2)",
            name, par
        );
    }
    let _ = std::fs::remove_dir_all(&workdir);
}

/// cargo 工作流集成测试：用 rustc wrapper + build-std 编译模板工程并运行产物。
/// 验证从 rust 工具链到 forge backend 的完整路径（cargo → rustc wrapper →
/// build-std core(LLVM) + 用户 crate(forge) → 链接 → 运行）。
#[test]
fn e2e_cargo_template_workflow() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let workspace = Path::new(manifest).join("..").join("..").join("..");
    let hello_dir = workspace.join("examples").join("forge-rustc-hello");
    assert!(
        hello_dir.join("Cargo.toml").exists(),
        "template project missing: {}",
        hello_dir.display()
    );

    let dll = backend_dll();
    let wrapper = workspace
        .join("tools")
        .join("forge-rustc-wrapper")
        .join("target")
        .join("debug")
        .join("forge-rustc-wrapper.exe");
    assert!(
        wrapper.exists(),
        "wrapper not built — run `cargo build --manifest-path tools/forge-rustc-wrapper/Cargo.toml` first"
    );

    let target_dir =
        std::env::temp_dir().join(format!("forge_cargo_target_{}", std::process::id()));
    // M3/M4（并行 CGU Stage A）：`-C codegen-units=4` 验证产物与 CGU 划分
    // 无关；**M4 起 rustc 并行前端（RUSTFLAGS 注入 `-Z threads=2`）+
    // FORGE_CODEGEN_THREADS=4** 使 forge 函数降级任务提交 rustc 查询池
    // （par_map——WA-38 根治，无 -Z threads 时自动串行产物不变）。
    // ——cargo/build-std 端到端（core 由 LLVM 编译，用户 crate 经 forge）。
    let rustflags = format!(
        "-Zcodegen-backend={} -Z threads=2 -C panic=abort -C overflow-checks=off -C codegen-units=4 -C link-arg=/SUBSYSTEM:CONSOLE -C link-arg=/DEFAULTLIB:kernel32.lib -C link-arg=/DEFAULTLIB:vcruntime.lib",
        dll.display()
    );

    // 编译模板工程（build-std=core：系统 crate 用 LLVM，用户 crate 用 forge）
    // P0-22：工具链钉版与 CI 一致（.github/workflows/ci.yml 单点同步）——
    // `+nightly` 显式 channel 不受 rustup override 影响，必须用钉版字面量；
    // 本机可用 FORGE_E2E_NIGHTLY 覆盖（浮动 channel，见 e2e_toolchain）。
    let toolchain = format!("+{}", e2e_toolchain());
    let build = Command::new("cargo")
        .current_dir(&hello_dir)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("RUSTC_WRAPPER", &wrapper)
        .env("RUSTFLAGS", &rustflags)
        .env("FORGE_CODEGEN_THREADS", "4")
        .args([toolchain.as_str(), "-Z", "build-std=core", "build"])
        .output()
        .map_err(|e| format!("failed to spawn cargo: {e}"))
        .expect("cargo spawn");
    if !build.status.success() {
        // build-std 需要下载 std 的注册表依赖（wasip3 等）——本机无网络/
        // 凭证（schannel SSL SEC_E_NO_CREDENTIALS）或镜像缺版本时 cargo
        // 构建必然失败，但这是环境限制而非后端缺陷：检测到网络特征则
        // SKIP（CI 有网仍会真跑），其他失败仍硬断言。
        let err = String::from_utf8_lossy(&build.stderr);
        let net_issue = err.contains("SSL")
            || err.contains("network error")
            || err.contains("spurious")
            || err.contains("failed to get")
            || err.contains("failed to load source")
            || err.contains("download of")
            || err.contains("failed to fetch")
            || err.contains("tries remaining");
        if net_issue {
            let _ = std::fs::remove_dir_all(&target_dir);
            println!(
                "SKIP cargo_template_workflow: network unavailable for build-std deps\n{}",
                err.lines().take(4).collect::<Vec<_>>().join("\n")
            );
            return;
        }
        panic!("cargo build failed:\n{err}");
    }

    // 运行产物并校验退出码（模板程序 while 循环累加 0..9 = 45）
    let exe = target_dir.join("debug").join("forge-rustc-hello.exe");
    let status = Command::new(&exe)
        .status()
        .map_err(|e| format!("failed to run {}: {e}", exe.display()))
        .expect("run exe");
    assert_eq!(
        status.code(),
        Some(45),
        "expected exit 45, got {:?}",
        status.code()
    );

    let _ = std::fs::remove_dir_all(&target_dir);
    println!("PASS  cargo_template_workflow exit=45");
}

/// M6（Stage B，每 CGU 独立对象文件）端到端：多模块 no_std crate +
/// `-C codegen-units=4` → rustc 分区出多 CGU → forge 每 CGU 一个 .o
///（链接器多 .obj）。验收：
/// 1. 编译成功（无 LNK2005 重复强符号——数据 owner 归并 + 实例首 CGU 定义）；
/// 2. 产物目录出现 ≥2 个 `forge_codegen_output.<cgu>.o`；
/// 3. 跨对象引用（函数 call / vtable ADDR64 / promoted rodata）经链接器
///    UNDEF 解析，运行退出码正确；
/// 4. 串行（T=1）与真并行（-Z threads=2 + FORGE_CODEGEN_THREADS=4）退出码一致。
///    （多模块是必要的：rustc 非增量默认会把小 CGU 合并成单 CGU——
///    显式 codegen-units + 多模块才产生多分区。）
#[test]
fn e2e_multi_object_cgu_units() {
    let workdir = std::env::temp_dir().join(format!("forge_rustc_e2e_m6_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create workdir");
    println!("workdir: {}", workdir.display());

    // 多模块 + trait dyn + slice/str 数据 + 跨模块调用（mod alpha/beta/main 各成 CGU）
    let extra = "mod alpha {\n\
         pub fn fa(x: i32) -> i32 { x * 2 + 1 }\n\
         pub fn fstr() -> &'static str { \"alpha-lit\" }\n\
         }\n\
         mod beta {\n\
         pub struct SX(pub i32);\n\
         impl SX { pub fn v(&self) -> i32 { self.0 + 2 } }\n\
         pub fn fslice() -> &'static [u8] { b\"bb\" }\n\
         }\n\
         trait Speak { fn sp(&self) -> i32; }\n\
         struct Dog;\n\
         impl Speak for Dog { fn sp(&self) -> i32 { 8 } }\n";
    let body = "let mut acc = 0i32; let mut i = 0; while i < 3 { acc += alpha::fa(i); i += 1; }\n\
         let d = Dog; let sd: &dyn Speak = &d;\n\
         acc + beta::SX(5).v() + alpha::fstr().len() as i32 + beta::fslice().len() as i32 + sd.sp()";
    let src = make_source(body, extra, "mainCRTStartup");
    let src_path = workdir.join("m6_multi.rs");
    std::fs::write(&src_path, &src).expect("write source");
    // alpha::fa(0..3)=1+3+5=9；SX(5).v()=7；fstr len=9；fslice len=2；sd.sp()=8 → 35
    let expected = 35;

    let compile_run = |rustc_args: &[String], envs: &[(&str, &str)]| -> Result<i32, String> {
        let out_exe = workdir.join("m6_multi_run.exe");
        let mut cmd = Command::new("rustc");
        cmd.args(rustc_args)
            .arg("-C")
            .arg(format!(
                "link-args=/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib"
            ))
            .arg(&src_path)
            .arg("-o")
            .arg(&out_exe)
            .envs(envs.iter().copied());
        let compile = cmd
            .output()
            .map_err(|e| format!("failed to spawn rustc: {e}"))?;
        if !compile.status.success() {
            let stderr = String::from_utf8_lossy(&compile.stderr);
            return Err(format!(
                "compile failed (LNK2005/多对象链接错误): {}\n{}",
                compile.status,
                stderr.lines().take(12).collect::<Vec<_>>().join("\n")
            ));
        }
        // 运行取退出码
        let status = Command::new(&out_exe)
            .status()
            .map_err(|e| format!("failed to run: {e}"))?;
        Ok(status.code().unwrap_or(-1))
    };

    let base_args = vec![
        "-Zcodegen-backend=".to_string() + &backend_dll().display().to_string(),
        "-C".to_string(),
        "panic=abort".to_string(),
        "-C".to_string(),
        "overflow-checks=off".to_string(),
        "--edition".to_string(),
        "2024".to_string(),
        "-C".to_string(),
        "codegen-units=4".to_string(),
        "-C".to_string(),
        "save-temps=yes".to_string(),
    ];

    // 串行 T=1
    let serial_exit = compile_run(&base_args, &[])
        .map_err(|e| format!("m6 multi-object (serial): {e}"))
        .expect("serial multi-object compile/run");
    assert_eq!(
        serial_exit, expected,
        "m6 multi-object serial exit={serial_exit} ≠ {expected}"
    );

    // 真并行（rustc 池 -Z threads=2 + forge 函数任务 par_map）
    let mut par_args = base_args.clone();
    par_args.push("-Z".to_string());
    par_args.push("threads=2".to_string());
    let par_exit = compile_run(&par_args, &[("FORGE_CODEGEN_THREADS", "4")])
        .map_err(|e| format!("m6 multi-object (parallel): {e}"))
        .expect("parallel multi-object compile/run");
    assert_eq!(
        par_exit, expected,
        "m6 multi-object parallel exit={par_exit} ≠ {expected}（par_map 路径多对象漂移）"
    );

    // 多对象产物断言：≥2 个 forge_codegen_output.<cgu>.o（rustc 分区出多 CGU）
    let objs: Vec<String> = std::fs::read_dir(&workdir)
        .expect("read workdir")
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            (n.starts_with("forge_codegen_output") && n.ends_with(".o")).then_some(n)
        })
        .collect();
    assert!(
        objs.len() >= 2,
        "m6 多对象预期 ≥2 个 .o（多模块 + codegen-units=4），实际 {}: {:?}",
        objs.len(),
        objs
    );

    let _ = std::fs::remove_dir_all(&workdir);
    println!(
        "PASS  e2e_multi_object_cgu_units exit={} (serial == -Z threads=2) objects={}",
        serial_exit,
        objs.len()
    );
}

/// M6 B-v2（debuginfo per-CGU CU）：`-C debuginfo>=1` 在多对象模式下不再
/// 回退单对象——**每个有函数的 CGU 对象自带独立 DWARF CU**（CU 内 reloc
/// 只引用本对象已定义函数符号，满足 add_dwarf 同文件约束）。断言：
/// 1. debuginfo=1 / debuginfo=2 + `-C codegen-units=4`（多模块源）编译链接
///    成功、运行退出码正确（35）；
/// 2. 产物 ≥2 个对象，且其中 ≥2 个含 `.debug_info` 段（per-CGU CU——
///    单 CU 摊平形态下只有一个对象带 debug 段）；
/// 3. debuginfo=2（full：变量/参数 DIE）同路径同样成立（debuginfo_full 单
///    模块回退单对象路径由 e2e debuginfo_full 用例守护）。
#[test]
fn e2e_multi_object_debuginfo_per_cgu_cu() {
    let workdir =
        std::env::temp_dir().join(format!("forge_rustc_e2e_dbg_m6_{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create workdir");
    println!("workdir: {}", workdir.display());

    // 多模块 + trait dyn + slice/str 数据 + 跨模块调用（mod alpha/beta + root 各成 CGU）
    let extra = "mod alpha {\n\
         pub fn fa(x: i32) -> i32 { x * 2 + 1 }\n\
         pub fn fstr() -> &'static str { \"alpha-lit\" }\n\
         }\n\
         mod beta {\n\
         pub struct SX(pub i32);\n\
         impl SX { pub fn v(&self) -> i32 { self.0 + 2 } }\n\
         pub fn fslice() -> &'static [u8] { b\"bb\" }\n\
         }\n\
         trait Speak { fn sp(&self) -> i32; }\n\
         struct Dog;\n\
         impl Speak for Dog { fn sp(&self) -> i32 { 8 } }\n";
    let body = "let mut acc = 0i32; let mut i = 0; while i < 3 { acc += alpha::fa(i); i += 1; }\n\
         let d = Dog; let sd: &dyn Speak = &d;\n\
         acc + beta::SX(5).v() + alpha::fstr().len() as i32 + beta::fslice().len() as i32 + sd.sp()";
    let src_path = workdir.join("m6dbg.rs");
    std::fs::write(&src_path, make_source(body, extra, "mainCRTStartup")).expect("write source");
    let expected = 35; // fa(0..3)=1+3+5=9；SX.v()=7；fstr len=9；fslice len=2；sp()=8

    for (level, label) in [("1", "debuginfo=1"), ("2", "debuginfo=2")] {
        let out_exe = workdir.join(format!("m6dbg_{level}.exe"));
        let mut cmd = Command::new("rustc");
        cmd.arg("-Zcodegen-backend=".to_string() + &backend_dll().display().to_string())
            .args([
                "-C", "panic=abort", "-C", "overflow-checks=off", "--edition", "2024",
                "-C", "codegen-units=4",
            ])
            .arg("-C")
            .arg(format!("debuginfo={level}"))
            .arg("-C")
            .arg("save-temps=yes")
            .arg("-C")
            .arg("link-args=/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib")
            .arg(&src_path)
            .arg("-o")
            .arg(&out_exe);
        let compile = cmd.output().expect("rustc spawn");
        if !compile.status.success() {
            panic!(
                "{label}: compile failed (多对象 + per-CGU dwarf)\n{}",
                String::from_utf8_lossy(&compile.stderr)
                    .lines()
                    .take(15)
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        let status = Command::new(&out_exe).status().expect("run exe");
        assert_eq!(
            status.code(),
            Some(expected),
            "{label}: exit {:?} ≠ {expected}（多对象 + per-CGU dwarf 语义漂移）",
            status.code()
        );
        // ≥2 个对象含 .debug_info（per-CGU CU）
        let objs: Vec<std::path::PathBuf> = std::fs::read_dir(&workdir)
            .expect("read workdir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let n = p
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                // 两次 level 编译同源同 cgu → 对象名相同，只统计含 debug 段的对象
                n.starts_with("forge_codegen_output") && n.ends_with(".o")
            })
            .collect();
        let mut with_dwarf = 0;
        for obj in &objs {
            let out = Command::new("llvm-objdump")
                .arg("-h")
                .arg(obj)
                .output()
                .expect("llvm-objdump spawn");
            let has_dwarf = String::from_utf8_lossy(&out.stdout).contains(".debug_info");
            if has_dwarf {
                with_dwarf += 1;
            }
        }
        assert!(
            with_dwarf >= 2,
            "{label}: per-CGU CU 预期 ≥2 个对象含 .debug_info，实际 {with_dwarf}/{}（{objs:?}）",
            objs.len(),
        );
        let _ = std::fs::remove_file(&out_exe);
        let _ = std::fs::remove_file(workdir.join(format!("m6dbg_{level}.o")));
        println!(
            "PASS  e2e_multi_object_debuginfo {label} exit={expected} objects_with_debug_info={with_dwarf}"
        );
    }
    let _ = std::fs::remove_dir_all(&workdir);
}

/// M6 增量编译端到端（-C incremental）：rustc 增量会话 + 多 CGU 多对象下
/// 二次编译（含源码变更）不 ICE、CGU 级 WorkProduct 正常读写、链接运行
/// 退出码正确。
#[test]
fn e2e_incremental_rebuild_cgu() {
    let workdir = std::env::temp_dir().join(format!("forge_rustc_e2e_incr_{}", std::process::id()));
    let incr_dir = workdir.join("incr");
    std::fs::create_dir_all(&workdir).expect("create workdir");
    println!("workdir: {}", workdir.display());

    // v1：多模块（alpha/beta/root）——fa(0..3)=1+3+5=9；body 见 body_of(false)
    let v1 = "mod alpha { pub fn fa(x: i32) -> i32 { x * 2 + 1 } pub fn fstr() -> &'static str { \"a\" } }\n\
         mod beta { pub struct SX(pub i32); impl SX { pub fn v(&self) -> i32 { self.0 + 2 } } }\n\
         trait Speak { fn sp(&self) -> i32; }\n\
         struct Dog;\n\
         impl Speak for Dog { fn sp(&self) -> i32 { 8 } }\n";
    // v2：新增 mod gamma（新 CGU）+ 改变 beta::SX.v 实现（CGU 失效）
    let v2 = "mod alpha { pub fn fa(x: i32) -> i32 { x * 2 + 1 } pub fn fstr() -> &'static str { \"a\" } }\n\
         mod beta { pub struct SX(pub i32); impl SX { pub fn v(&self) -> i32 { self.0 + 3 } } }\n\
         mod gamma { pub fn fg(x: i32) -> i32 { x * 5 } }\n\
         trait Speak { fn sp(&self) -> i32; }\n\
         struct Dog;\n\
         impl Speak for Dog { fn sp(&self) -> i32 { 8 } }\n";
    let body_of = |use_gamma: bool| {
        format!(
            "let mut acc = 0i32; let mut i = 0; while i < 3 {{ acc += alpha::fa(i); i += 1; }}\n\
             let d = Dog; let sd: &dyn Speak = &d;\n\
             acc + beta::SX(5).v() + alpha::fstr().len() as i32 + sd.sp(){}",
            if use_gamma { " + gamma::fg(2)" } else { "" }
        )
    };
    // v1：9 + 7 + 1 + 8 = 25；v2：9 + (5+3) + 1 + 8 + 10 = 36
    let (exit1, exit2) = (25i32, 36i32);

    let compile = |extra: &str, body: &str, out: &str| -> i32 {
        // 增量会话要求**同一输入文件/同 crate 名**（dep-graph/crate hash 匹配）
        // ——不同 pass 复用同名源文件，源码内容变更使对应 CGU 失效。
        let src_path = workdir.join("incr_prog.rs");
        std::fs::write(&src_path, make_source(body, extra, "mainCRTStartup")).expect("write");
        let exe = workdir.join(format!("{out}.exe"));
        let status = Command::new("rustc")
            .arg("-Zcodegen-backend=".to_string() + &backend_dll().display().to_string())
            .args([
                "-C", "panic=abort", "-C", "overflow-checks=off", "--edition", "2024",
                "-C", "codegen-units=4",
            ])
            .arg("-C")
            .arg(format!("incremental={}", incr_dir.display()))
            .arg("-C")
            .arg("link-args=/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib")
            .arg(&src_path)
            .arg("-o")
            .arg(&exe)
            .status()
            .expect("rustc spawn");
        assert!(status.success(), "{out}: 增量编译失败");
        Command::new(&exe)
            .status()
            .expect("run exe")
            .code()
            .expect("exit code")
    };

    let e1 = compile(v1, &body_of(false), "incr_pass1");
    assert_eq!(e1, exit1, "增量 pass1 exit={e1} ≠ {exit1}");
    let e2 = compile(v2, &body_of(true), "incr_pass2");
    assert_eq!(
        e2, exit2,
        "增量 pass2（源码变更 + 新 CGU）exit={e2} ≠ {exit2}"
    );
    // pass2 无变更再编（同会话目录重复编译——WP 重写路径）
    let e2b = compile(v2, &body_of(true), "incr_pass2b");
    assert_eq!(e2b, exit2, "增量 pass2 重复 exit={e2b} ≠ {exit2}");

    let _ = std::fs::remove_dir_all(&workdir);
    println!(
        "PASS  e2e_incremental_rebuild_cgu exit={exit1}→{exit2}（-C incremental 3 次编译 + 多 CGU WorkProduct）"
    );
}
