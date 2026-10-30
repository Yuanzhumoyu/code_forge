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
        expected: 3, // 1 + 2（上一轮脚本误改成 0，已修回）
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
    Case {
        name: "float_param_live_across_call",
        // **KD-11**：f32 形参在函数内有**调用**时必须跨调用存活。x86_64/arm64 的
        // 浮点寄存器都是 caller-saved ⇒ regalloc 必然把形参 spill；收参阶段必须把
        // ABI 的 FPR 落点写进该形参的 spill 槽。旧生成器只对 GPR 形参做这件事
        // （`!class.is_fp()` 条件）⇒ 浮点形参的槽**从未写入**，reload 读未初始化值
        // ⇒ `f(1.0) = 1.0 + 9.0` 得 9.0（期望 10.0）。helper 必须 `#[inline(never)]`
        // 才能让"调用"真的存在（否则整个函数被折叠，缺陷被掩盖）。
        body: "let x = core::hint::black_box(1.0f32); if f(x) == 10.0 { 10 } else { 9 }",
        expected: 10,
        extra: "#[inline(never)]\nfn helper() -> f32 { 9.0 }\n#[inline(never)]\nfn f(x: f32) -> f32 { let y = helper(); x + y }",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 KD-11",
        reason: "f32 形参跨调用 spill 后收参不得丢失（ABI FPR → spill 槽）",
    },
    Case {
        // 同一形状的 **-O2** 档：除 KD-11 外还钉住 SCCP 不得把已有 `Fconst` 走
        // 整数 Big 通道重写（f32 位型 0x41200000 被当整数 1092616192 ⇒ arm64 材料化
        // 成 f64(1092616192) 的位型、取低 32 位得 0，比较变成 `== 0.0`）。实测修复前
        // 本用例返回 9、修复后 10。
        name: "opt_float_param_live_across_call",
        body: "let x = core::hint::black_box(1.0f32); if f(x) == 10.0 { 10 } else { 9 }",
        expected: 10,
        extra: "#[inline(never)]\nfn helper() -> f32 { 9.0 }\n#[inline(never)]\nfn f(x: f32) -> f32 { let y = helper(); x + y }",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 KD-11",
        reason: "-O2 下 f32 形参跨调用 spill 收参 + f32 常量位型都必须正确",
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
    // v27 M6-139：`core::simd` 逐元素算术经 simd_add/sub/mul/div/neg 降级到 IR 的
    // Vadd/Vsub/Vmul/Vdiv/Vneg（此前这些 intrinsic 落到 "unsupported intrinsic"，
    // 向量算术在 forge 后端根本编不过）。断言用 as i32 累加，避免浮点近似掩盖错值。
    Case {
        name: "simd_v128_arith",
        body: "let a = core::simd::Simd::<f32, 4>::from_array([1.0, 2.0, 3.0, 4.0]); let b = core::simd::Simd::<f32, 4>::splat(10.0); let c = a + b; let d = c * b; let e = d - b; let f = e / b; let g = -f; (f[0] as i32) * 1000 + (f[1] as i32) * 100 + (f[2] as i32) * 10 + (f[3] as i32) + (g[0] as i32 == -10) as i32",
        expected: 11234, // f=(10,11,12,13) → 10*1000+11*100+12*10+13 = 11233；g[0]=-10 → +1
        extra: "#![feature(portable_simd)]\nuse core::simd::Simd;\n",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "B3 simd",
        reason: "v27 M6-139：向量逐元素 add/sub/mul/div/neg 端到端",
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
    // v27 M6-140：同宽标量 transmute（`CastKind::Transmute`）。旧实现落 `_ => Ok(val)`
    // ⇒ 值保持源类型，随后按目标类型取数会静默错值（aarch64 -O0 实测
    // `f32::to_bits()` 恒 0；x86 因值在 GPR/XMM 的布局差异不一定暴露）。
    // 用 `#[inline(never)] one()` 保证值来自运行期，避免常量折叠掩盖缺陷。
    Case {
        name: "float_bits_transmute",
        body: "{ #[inline(never)] fn one() -> i32 { 1 } let x = (one() as f32) + 9.0; let u = x.to_bits(); let y = f32::from_bits(u); if u != 0x4120_0000 || y != 10.0 { 1 } else { 0 } }",
        expected: 0,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "D intrinsics",
        reason: "v27 M6-140：同宽标量 transmute 按位重解释（to_bits/from_bits）",
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
    Case {
        name: "inline_asm_sym",
        // v27 M6-93：`sym` 以**输入寄存器**承载符号地址（函数 + 静态两种）。
        // 强断言：地址必须等于 Rust 侧同一符号的 `as usize`，且静态内容可读。
        body: "let f: fn() -> i64 = sym_target;                                     let mut got: usize;                                                       unsafe { core::arch::asm!(\"mov {0}, {1}\", out(reg) got, sym sym_target) };                       let mut p: usize;                                                         unsafe { core::arch::asm!(\"mov {0}, {1}\", out(reg) p, sym SYM_BYTE) };                          let v = unsafe { *(p as *const u8) };                                     if got == f as usize && v == 7 { 42 } else { 7 }",
        extra: "#[inline(never)]\nfn sym_target() -> i64 { 42 }\nstatic SYM_BYTE: u8 = 7;\n",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M2.5 S3",
        reason: "asm! 的 sym（函数/静态）：地址经隐藏全局符号 + abs64 重定位物化到输入寄存器",
    },
    Case {
        name: "inline_asm_sym_call",
        // `sym` 用在 `call {0}`：同时覆盖"池寄存器避开显式名"（`out(\"rax\")` 与
        // `clobber_abi` 都点名 RAX，sym 的池选择不能再挑 RAX）。
        body: "let r: i64; unsafe { core::arch::asm!(\"call {0}\", sym sym_target, out(\"rax\") r, clobber_abi(\"C\")) }; if r == 42 { 42 } else { 7 }",
        extra: "#[inline(never)]\nfn sym_target() -> i64 { 42 }\n",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M2.5 S3",
        reason: "sym 作为 call 目标（寄存器间接调用），并与显式 out(\"rax\")/clobber_abi 共存",
    },
    Case {
        name: "inline_asm_sym_literal",
        // **语义边界**：把 sym 当**字面符号**写（`[rip + {0}]`）时，替换出来的是寄存器，
        // 本谱汇编器拒绝该文本 ⇒ 编译期明确报错（fail-closed，不静默错码）。
        body: "let mut got: usize; unsafe { core::arch::asm!(\"lea {0}, [rip + {1}]\", out(reg) got, sym sym_target) }; got as i32",
        extra: "#[inline(never)]\nfn sym_target() -> i64 { 42 }\n",
        entry: "mainCRTStartup",
        expected: 0,
        expect_compile_fail: true,
        expect_compile_err: "内联汇编文本无法被汇编器解析",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M2.5 S3",
        reason: "sym 只保证寄存器形态；字面符号写法（[rip + {0}]）必须明确拒绝（fail-closed）",
    },
    Case {
        name: "inline_asm_label",
        // v27 M6-94：`label` 操作数 = 目标基本块。文本里的名字由 ISA 汇编器从**外部标签表**
        // 取值（值 = IR 块下标），编码器的 label 槽走 `use_label_at` ⇒ 与普通分支同一条路。
        // 强断言：jmp 真的落到 label 块的入口（n = 7），不是"编过了但跳错/没跳"。
        body: "let mut n: u64 = 0; unsafe { core::arch::asm!(\"jmp {0}\", label { n = 7; }) }; if n == 7 { 42 } else { 7 }",
        extra: "",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M2.5 S4",
        reason: "asm! 的 label：jmp 到目标块（外部标签 → IR 块下标 → 相对位移）",
    },
    Case {
        name: "inline_asm_label_cond",
        // 条件分支 + label：`cmp 0` 置 ZF，`jcc e` 命中时跳到 label 块（n = 7）。
        body: "let x: u64 = 0; let mut n: u64 = 0; unsafe { core::arch::asm!(\"cmp {0}, 0\", \"jcc e, {1}\", in(reg) x, label { n = 7; }) }; if n == 7 { 42 } else { 7 }",
        extra: "",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M2.5 S4",
        reason: "条件分支 jcc 命中 label（带输入寄存器约束 + 外部标签共存）",
    },
    Case {
        name: "inline_asm_label_cond_skip",
        // 反向：条件不命中 ⇒ label 块**不执行**（n 仍 0），落回 asm 后的正常控制流。
        body: "let x: u64 = 1; let mut n: u64 = 0; unsafe { core::arch::asm!(\"cmp {0}, 0\", \"jcc e, {1}\", in(reg) x, label { n = 7; }) }; if n == 0 { 42 } else { 7 }",
        extra: "",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M2.5 S4",
        reason: "label 未命中时不误入目标块（fallthrough 与目标块是两条路）",
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

    // ── v27 M2.5/S4：`clobber_abi("C")` ──
    //   rustc 在 MIR 构建期把 `clobber_abi` 展开成一批 `Out { place: None }`（纯 clobber）
    //   ⇒ 后端把它们记进 `inst_clobbers`（本 ISA 不认识的寄存器——如 AVX-512 mask `k0`——
    //   跳过无害：我们从不在那些寄存器里放值）。
    Case {
        name: "inline_asm_clobber_abi",
        body: "unsafe { core::arch::asm!(\"nop\", clobber_abi(\"C\")) }; 42",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M2.5 S4",
        reason: "clobber_abi(\"C\") 从 fail-closed 变成可用（纯 clobber 槽）",
    },

    // ── v27 M2.5/S4：`options(noreturn)` ──
    //   此前被一票否决（`[cap:INLINE_ASM]`）。现在支持：汇编不返回 ⇒ 没有 fallthrough
    //   （rustc 侧 `targets` 为空）⇒ 当前块以 `unreachable` 收尾，`ud2` 真 trap。
    Case {
        name: "inline_asm_noreturn",
        body: "unsafe { core::arch::asm!(\"ud2\", options(noreturn)) }",
        expected: -1073741795, // 0xC000001D = NTSTATUS #UD（与 inline_asm_ud2 同口径）
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: true,
        known_failure: false,
        phase: "v27 M2.5 S4",
        reason: "options(noreturn)：汇编不返回 ⇒ 块以 unreachable 收尾，ud2 真 trap",
    },

    // ── v27 M2.5/S3 第二半：`sym` 占位符（**fail-closed，点名原因**）──
    //   `sym` 的符号文本能拼出来，但**本谱的汇编器不接受符号操作数**
    //   （`global_asm!` 能用符号是因为它走 LLVM 汇编器，inline asm 走的是本谱汇编器）。
    //   要支持它，得给 ISA-DSL 的汇编器加"符号立即数 + `reloc abs64`"的解析形态。
    //   这条负向用例钉住"拒绝 + 说清原因"，避免将来有人以为"没实现"而重复踩。
    // ── v27 M5：`#[unsafe(naked)]` **真支持**（v28 起）──
    //   裸函数的函数体只有汇编、没有编译器序/尾声与栈帧。实测落地需要三件事同时到位：
    //   ① IR `FunctionAttributes::NO_FRAME`（发射侧据此跳过序言/尾声）；
    //   ② lowering **不建任何栈槽**（`LowerCtxt::new` 的 `naked` 形参）——否则帧里会有槽，
    //      而 naked 没帧可指；
    //   ③ 无落点的 `InlineAsm` 必须以 `unreachable` 收尾（裸函数的 MIR 是
    //      `options()` 空 + `targets` 空，此前只看 `NORETURN` ⇒ "块没有终结符"的内部错误）。
    //   本用例走完整条链：`add_one(41)` 由裸汇编 `lea rax,[rcx+1]; ret` 算出 42。
    Case {
        name: "naked_fn",
        body: "add_one(41) as i32",
        expected: 42,
        extra: "#[unsafe(naked)]\nextern \"C\" fn add_one(x: i64) -> i64 {\n    core::arch::naked_asm!(\"lea rax, [rcx + 1]\", \"ret\");\n}\n",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "裸函数：NO_FRAME（无栈帧）+ 不建栈槽 + 无落点 asm 以 unreachable 收尾",
    },
    // ── v27 M5：裸函数 + `const` 操作数（`naked_asm!` 允许的另一种操作数形态）──
    //   rustc 前端**明确禁止**在 `naked_asm!` 里用寄存器操作数
    //   （"the `in` operand cannot be used with `naked_asm!`"，实测同一条消息对 LLVM 后端也一样）
    //   ⇒ 语言层面裸函数的操作数只有 `const`/`sym` 两种。`sym` 仍受谱汇编器限制（见下一条用例），
    //   `const` 走 S3 的"就地换成字面文本"路径，本用例钉住它。
    Case {
        name: "naked_const_operand",
        body: "add_k(35) as i32",
        expected: 42,
        extra: "#[unsafe(naked)]\nextern \"C\" fn add_k(x: i64) -> i64 {\n    core::arch::naked_asm!(\"lea rax, [rcx + {k}]\", \"ret\", k = const 7);\n}\n",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "裸函数 + const 操作数：文本就地替换成字面量，仍无栈帧",
    },
    // ── v27 M5：`#[link_section]` 必须**明确拒绝**（不许静默放错段）──
    //   后端只发 .text/.data/.rodata 三段，段名/段标志由发射层决定 ⇒ 静默忽略
    //   等于"写了不生效"，那是最糟的一种错。这里连同诊断一起钉住。
    Case {
        name: "neg_link_section",
        body: "#[unsafe(link_section = \".mydata\")] static X: u32 = 7; X as i32",
        expected: 0,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: true,
        expect_compile_err: "LINK_SECTION",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "#[link_section] 静默忽略 = 写了不生效 ⇒ 明确拒绝并点名段名",
    },
    // ── v27 M5 补面：静对齐（`#[repr(align(N))]`）必须真的生效 ──
    //   对齐取自 layout（`inner.align.bytes()`）⇒ 发射器按 `sd.align` 摆符号。
    //   这里在**运行期**验地址：64 对齐才返回 42（否则 1），不依赖对象格式细节。
    Case {
        name: "static_align_64",
        body: "#[repr(align(64))] struct Aligned(u64); #[unsafe(no_mangle)] static LATE: Aligned = Aligned(7); if (&LATE as *const Aligned as usize) % 64 == 0 { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "#[repr(align(64))] 的符号地址必须 64 对齐（不止是「类型布局」，要真摆对）",
    },
    // ── v27 M5 · M-E：128 位整数**现已真支持**（分字展开）──
    //   历史：`u128`/`i128` 曾落到 `map_type` 的兜底分支被当成 64 位 `PTR`
    //   ⇒ `1u128 << 64` 被掩码成 `1 << 0`、整条链静默算错（实测低半=6）。
    //   **本轮（v27 M6-56）常量形态也通了**：`legalize` 的 `narrow_overflow` pass 现在
    //   接管 8/16 位**与宽值**（> 最大原生位宽）的 `*_overflow` 加/减，改成"算术 + 比较"，
    //   并把该 pass 排在 `wide_split` **之前**（否则 `wide_split` 先因为不认
    //   `UaddOverflow` 而 fail-closed）。⇒ 本用例由负向翻正（O0/O2 实测 exit=5）。
    Case {
        name: "u128_const_shift_add",
        body: "let x: u128 = (1u128 << 64) + 5; x as u64 as i32",
        // (1<<64)+5 截断到 u64 = 5
        expected: 5,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "128 位常量移位 + 加法 + 截断（M6-56 起检查算术也支持；原负向门控已过时）",
    },
    // ── v27 M5 缺陷①修复：窄宽度移位语义 ──
    //   曾经 `i8: -128 >> 7` 算成 1（局部变量用 `MovzxR8Mem` 零扩展载入，
    //   之后 `sar` 得到 1）。修法：移位前按符号性扩展到 64 位、结果截回原宽度。
    //   期望值全部由 LLVM 对照取得（b=-1 d=1 f=1 h=-1）。
    Case {
        name: "narrow_shift_semantics",
        body: "let a: i8 = -128; let b = a >> 7; let c: u8 = 128; let d = c >> 7; let e: u16 = 0x8000; let f = e >> 15; let g: i16 = -32768; let h = g >> 15; if b == -1 && d == 1 && f == 1 && h == -1 { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "窄有符号/无符号右移按操作数位宽与符号性（算术 vs 逻辑）",
    },
    // ── v27 M5 缺陷③：窄有符号算术（期望值由 LLVM 对照：s=-60 m=96 d=-33 r=-1）──
    Case {
        name: "narrow_arith_signed",
        body: "let a: i8 = -100; let b: i8 = 40; let s = a.wrapping_add(b); let m = a.wrapping_mul(b); let d = a / 3; let r = a % 3; if s == -60 && m == 96 && d == -33 && r == -1 { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "窄有符号：回绕加/乘、除法/取模向零截断（负数余数符号跟被除数）",
    },
    // ── v27 M5 缺陷②修复：整数 ↔ 浮点转换矩阵（期望值由 LLVM 对照）──
    //   关键事实：`elem` 谓词用的是 `elem_id_of` 的**自有编号**（I8=5/I16=6/I32=3/I64=4），
    //   既不是位宽也不是 TypeId ⇒ `f64 → i8/i16` 这类此前根本没有规则。
    Case {
        name: "cast_int_float_matrix",
        body: "let x = -7i32; let y = x as i8 as i64 as f64 as i32; let z = 3.99f64 as u8; let w = 12345u64 as f64 as u64; let q = -2.5f32 as i8; let big = 1e10f64 as i64; if y == -7 && z == 3 && w == 12345 && q == -2 && big == 10_000_000_000 { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "整数↔浮点转换（截断向零、符号扩展、u64 大值、f64→窄整）",
    },
    // ── v27 M5 补面：溢出检查**真的**会 panic（e2e 默认 `-C overflow-checks=on`）──
    //   注意：操作数必须经 `black_box` 否则会被**编译期 lint**（unconditional_panic）拦下，
    //   根本走不到运行期检查——第一次写这条用例就是这么被"骗"过的。
    Case {
        name: "overflow_panic_shl",
        body: "let x: u32 = 1; let s = core::hint::black_box(32u32); let _ = x << s; 42",
        expected: 101,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "移位溢出在 overflow-checks=on 下必须 panic（退出码 101）；black_box 防常量折叠",
    },
    // ── v27 M5 补面：`i32::MIN / -1` 溢出 panic ──
    Case {
        name: "overflow_panic_div",
        body: "let x = core::hint::black_box(i32::MIN); let y = core::hint::black_box(-1i32); let _ = x / y; 42",
        expected: 101,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "i32::MIN / -1 在 overflow-checks=on 下必须 panic；black_box 防常量折叠",
    },
    // ── v27 M5 缺陷修复：浮点比较语义（含 NaN 的无序行为）──
    //   根因：rustc 侧把 `Ne` 映射成 `FloatCC::One`（**有序**不等）⇒ `NaN != x` 返回 false；
    //   Rust 的 `!=` 是**无序**不等（`Une`），NaN 参与时必须为 true。
    //   探针实测（位掩码）：其余五种比较都对，只有 `!=` 在 NaN 上错 ⇒ 一处映射错。
    Case {
        name: "float_cmp_semantics",
        body: "let n = 0.0f64 / 0.0f64; let g = 1.5f64; let h = 2.5f64; let mut m: i32 = 0; if g == g { m |= 1 } if !(g == h) { m |= 2 } if n != 1.0 { m |= 4 } if !(n == n) { m |= 8 } if !(n < 1.0) { m |= 16 } if !(n > 1.0) { m |= 32 } if m == 63 { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "浮点比较语义：==/< /> 有序（NaN 全 false），!= 无序（NaN 为 true）",
    },
    Case {
        name: "float_cmp_normalized_bool",
        // **严格数值**（不是 truthy 判断）：把每个比较结果直接 `as i32` 累加。
        // 六个条件里有六个走"两个 setcc 组合"的形态（oeq/olt/ole/ugt/uge/une），
        // 组合用的临时寄存器若只写了低 8 位（`SETCC` 的编码事实）而没清零，高位垃圾
        // 会被 `and`/`or` 带进结果 ⇒ 值不再规范化成 0/1，这条用例立刻偏。
        // 历史实测：`Ugt(1,2)` 曾得 `-118417152`（v27 M5 修：`isa/x86.toml` 的
        // 形状 B/C 规则在 `COMIS*` 之前补 `xor {g}, {g}`）。
        body: "let a = core::hint::black_box(1.5f64); let b = core::hint::black_box(2.5f64); let n = core::hint::black_box(0.0f64 / 0.0f64); let mut m: i32 = 0; m += (a == a) as i32; m += (a < b) as i32; m += (a > b) as i32; m += (a <= a) as i32; m += (n == n) as i32; m += (n != n) as i32; m += (n < a) as i32; m += (n >= a) as i32; m += (a != b) as i32; m += (!(a == b)) as i32; m + 36",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "六个组合形态浮点比较（oeq/olt/ole/ugt/uge/une）必须把结果规范化成 0/1",
    },
    // ── v27 M5 缺陷修复：位运算内建按**操作数位宽** ──
    //   `ctlz`：谱对 8/16 位输入走 32 位 `lzcntl` ⇒ 实测 `u8::leading_zeros(0x10)` = 27，
    //   修正为减 `(32 - w)` 后 = 3 ✔；`rotate_left/right`：此前走 `funnel_shl` 的
    //   "返回 a" 兜底（恒返回原值）⇒ 现已实现真正的 funnel shift。
    //   **期望值全部由 LLVM 对照**：lz=3 tz=4 lz0=8 tz_16=8 ones=16 rotl=**44** rotr=**194** lz_u64=7。
    Case {
        name: "bit_intrinsics_narrow",
        body: "let a = core::hint::black_box(0x10u8); let b = core::hint::black_box(0u8); let c = core::hint::black_box(0xFF00u16); let d = core::hint::black_box(0xF0F0_F0F0u32); let e = core::hint::black_box(0b1011u8); let f = core::hint::black_box(0x0100_0000_0000_0000u64); let mut m: i32 = 0; if a.leading_zeros() != 3 { m |= 1 } if a.trailing_zeros() != 4 { m |= 2 } if b.leading_zeros() != 8 { m |= 4 } if c.trailing_zeros() != 8 { m |= 8 } if d.count_ones() != 16 { m |= 16 } if e.rotate_left(2) != 44 { m |= 32 } if f.leading_zeros() != 7 { m |= 64 } if e.rotate_right(2) != 194 { m |= 128 } if m == 0 { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "窄类型 leading/trailing zeros、count_ones、rotate（期望值 LLVM 对照）",
    },
    // ── v27 M5 补面：**非 2 幂**元素大小的指针算术（add 是乘 12、offset_from 是除 12）──
    Case {
        name: "ptr_arith_size12",
        body: "let arr = [[1u32, 2, 3], [4, 5, 6], [7, 8, 9]]; let p = arr.as_ptr(); let q = unsafe { p.add(2) }; let r = unsafe { q.sub(1) }; let off = unsafe { r.offset_from(p) }; let ok = off == 1 && unsafe { (*r)[0] } == 4 && unsafe { (*q)[2] } == 9; if ok { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "元素大小 12 的 add/sub/offset_from（乘法与除法链）",
    },
    // ── v27 M5 缺陷修复：`Range::contains`（此前 ud2 崩溃）──
    //   根因：`Discriminant` 的 Direct-tag 分支从 `BackendRepr::ScalarPair { a, .. }` 推 tag
    //   宽度，而 `Bound<&u32>` 的 `a` 是 **payload 指针（8B）** ⇒ 读 8 字节载荷去比 case 0
    //   ⇒ payload≠0 就落 otherwise（unreachable）⇒ ud2。修法：tag 宽度按**变体数**、
    //   偏移按 `tag_field`。
    Case {
        name: "range_contains",
        body: "let v = core::hint::black_box(0x11_0000u32); if (0xD800u32..0xE000u32).contains(&v) { 1 } else { 42 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "char::from_u32：有效码点 Some、越界与代理区 None（niche 宽度正确）",
    },
    // ── v27 M5 缺陷修复：`char::from_u32`（niche 值宽度）──
    //   根因：niche 判别值恒按 **I64** 读，而 `Result<char, CharTryFromError>` 只有 4 字节
    //   ⇒ 多读 4 字节栈垃圾掺进 `raw - niche_start` ⇒ `is_niche` 判错 ⇒ 无效码点被当 `Ok`。
    //   修法：niche 宽度按标量真实大小（`backend_repr` 的 Scalar/ScalarPair 一半）读，再 uextend。
    Case {
        name: "char_from_u32",
        body: "let ok = core::char::from_u32(66) == Some('B'); let bad_hi = core::char::from_u32(core::hint::black_box(0x11_0000u32)).is_none(); let bad_sur = core::char::from_u32(core::hint::black_box(0xD800u32)).is_none(); if ok && bad_hi && bad_sur { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "16 字节对齐的栈局部真的 16 对齐（此前被放在 8 对齐地址）",
    },
    // ── v27 M5 补面：CFG 形态「短路 &&（左假则不评估右）」（期望值 LLVM 对照）──
    Case {
        name: "probe_cfg_and",
        body: "let a = core::hint::black_box(true); let b = core::hint::black_box(false); if a && b { 1 } else { 42 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "短路 &&（左假则不评估右）（期望值 LLVM 对照）",
    },
    // ── v27 M5 补面：CFG 形态「短路 ||（左假则评估右）」（期望值 LLVM 对照）──
    Case {
        name: "probe_cfg_or",
        body: "let a = core::hint::black_box(true); let b = core::hint::black_box(false); if b || a { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "短路 ||（左假则评估右）（期望值 LLVM 对照）",
    },
    // ── v27 M5 补面：CFG 形态「if / else-if / else 链」（期望值 LLVM 对照）──
    Case {
        name: "probe_cfg_elsechain",
        body: "let x = core::hint::black_box(2); if x == 0 { 1 } else if x == 1 { 2 } else { 42 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "if / else-if / else 链（期望值 LLVM 对照）",
    },
    // ── v27 M5 补面：CFG 形态「元组 match（两 bool）」（期望值 LLVM 对照）──
    Case {
        name: "probe_cfg_tuplematch",
        body: "let a = core::hint::black_box(true); let b = core::hint::black_box(false); match (a, b) { (true, true) => 1, (true, false) => 42, _ => 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "元组 match（两 bool）（期望值 LLVM 对照）",
    },
    // ── v27 M5 补面：CFG 形态「Option match（niche 判别）」（期望值 LLVM 对照）──
    Case {
        name: "probe_cfg_optmatch",
        body: "let o: Option<i32> = Some(core::hint::black_box(7)); match o { Some(v) => v, None => 1 }",
        expected: 7,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "Option match（niche 判别）（期望值 LLVM 对照）",
    },
    // ── v27 M5 补面：CFG 形态「while + continue（跳过累加）」（期望值 LLVM 对照）──
    Case {
        name: "probe_cfg_loop",
        body: "let mut i = 0; let mut s = 0; while i < 5 { if i == 3 { i += 1; continue } s += i; i += 1 } if s == 7 { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "while + continue（跳过累加）（期望值 LLVM 对照）",
    },
    // ── v27 M5 补面：CFG 形态「函数内 early return」（期望值 LLVM 对照）──
    Case {
        name: "probe_cfg_earlyret",
        body: "fn f(x: i32) -> i32 { if x > 0 { return 42 } 1 } f(core::hint::black_box(5))",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "函数内 early return（期望值 LLVM 对照）",
    },
    // ── v27 M5 补面：CFG 形态「match guard（`n if n > 2`）」（期望值 LLVM 对照）──
    Case {
        name: "probe_cfg_guard",
        body: "match core::hint::black_box(3) { n if n > 2 => 42, _ => 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "match guard（`n if n > 2`）（期望值 LLVM 对照）",
    },
    // ── v27 M5 缺陷修复：**栈槽按类型 ABI 对齐** ──
    //   此前槽位只做 `next_offset -= size`（无对齐）⇒ `#[repr(align(16))]`/u128 的局部
    //   落在 8 对齐地址 ⇒ rustc 插入的 `assert(misaligned pointer dereference)` 运行期触发。
    Case {
        name: "local_align16",
        body: "#[repr(align(16))] struct A(u64); let a = A(1); if (&a as *const A as usize) % 16 == 0 { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 截断到 u64/u32/u8（期望值 LLVM 对照：1122334455667788 / 55667788 / 88）",
    },
    // ── v27 M5-③：`i128/u128` 第一步 —— **128 位 → 更窄整数**的截断 ──
    //   `u128` 在 ABI 上是 Indirect ⇒ 值只存在于 16 字节栈槽；截断取槽的低 8 字节再按
    //   目标宽度截（对齐由 local_align16 的修复保证，否则 rustc 的对齐断言会先崩）。
    Case {
        name: "u128_trunc",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([0x1122_3344_5566_7788, 0]); let p = &a as *const A as *const u128; if unsafe { *p as u64 } == 0x1122_3344_5566_7788u64 && unsafe { *p as u32 } == 0x5566_7788u32 && unsafe { *p as u8 } == 0x88u8 { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 的 <、==、>（含 hi/lo 两级比较）",
    },
    // ── v27 M5-③：`u128/i128` 的**比较**（先比 hi、相等再比 lo，`select` 合成）──
    //   Eq/Ne 用 and/or；Lt/Le/Gt/Ge 用 `select(eq_hi, lo_cmp, hi_cmp)`。
    Case {
        name: "u128_cmp",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([5, 0]); let b = A([7, 0]); let pa = &a as *const A as *const u128; let pb = &b as *const A as *const u128; let lt = unsafe { *pa < *pb }; let eq = unsafe { *pa == *pb }; let gt = unsafe { *pa > *pb }; if lt && !eq && !gt { 42 } else { 1 }",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "probe",
        reason: "临时探针",
    },
    // 带 ZST 字段的 8 字节结构体按值返回后字段不丢
    Case {
        name: "zst_field_struct",
        body: "struct M; struct S { r: core::ops::Range<i32>, f: M } #[inline(never)] fn mk2(r: core::ops::Range<i32>) -> S { S { r, f: M } } let s = mk2(0..5); s.r.end",
        expected: 5,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "带 ZST 字段的 8 字节结构体按值返回后字段不丢",
    },
    // Iterator::map（Map = Range + ZST 闭包）：next 与 size_hint
    Case {
        name: "iter_map",
        body: "let v = (0..5).map(|x| x * 2 + 1).next().unwrap_or(99); let h = (0..5).map(|x| x * 2).size_hint().0 as i32; v + h",
        expected: 6,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "Iterator::map（Map = Range + ZST 闭包）：next 与 size_hint",
    },
    // RangeInclusive（12 字节）迭代 count
    Case {
        name: "range_inclusive_count",
        body: "let n = (1..=4).count(); n as i32",
        expected: 4,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "probe",
        reason: "临时探针",
    },
    // 12 字节结构体按值返回：字段不丢（聚合传递阈值 8 字节）
    Case {
        name: "struct12_byval",
        body: "struct T { a: i32, b: i32, c: i32 } #[inline(never)] fn mk() -> T { T { a: 1, b: 2, c: 3 } } let t = mk(); t.c",
        expected: 3,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "probe",
        reason: "临时探针",
    },
    // RangeInclusive::fold / sum（Variants::Single 判别）
    Case {
        name: "range_fold_sum",
        body: "let s = (1..=4).fold(0, |a, b| a + b); s",
        expected: 10,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "probe",
        reason: "临时探针",
    },
    // RangeInclusive::sum（同上）
    Case {
        name: "range_sum",
        body: "let s: i32 = (1..=4).sum(); s",
        expected: 10,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "probe",
        reason: "临时探针",
    },
    // filter + product（同上）
    Case {
        name: "filter_product",
        body: "let j: i32 = (1..=4).filter(|x| x % 2 == 0).product(); j",
        expected: 8,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "probe",
        reason: "临时探针",
    },
    // v27 M5-③：`u128` 按值传参 + 按值返回（128 位 = ScalarPair 两寄存器；调用点绑定两个返回值）
    Case {
        name: "u128_byval",
        body: "#[repr(align(16))] struct A([u64; 2]); #[inline(never)] fn id(x: u128) -> u128 { x } let a = A([7, 0]); let p = &a as *const A as *const u128; let y = unsafe { id(*p) }; let z = unsafe { y as u64 }; z as i32",
        expected: 7,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 按值传参/返回（调用点按 ScalarPair 绑定两个返回值）",
    },
    // v27 M5-③：`u128` 的 add/sub（lo±lo 取进位/借位，hi±hi±carry）
    Case {
        name: "u128_addsub",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([7, 0]); let b = A([5, 0]); let pa = &a as *const A as *const u128; let pb = &b as *const A as *const u128; let s = unsafe { (*pa).wrapping_add(*pb) }; let d = unsafe { (*pa).wrapping_sub(*pb) }; let sw = unsafe { s as u64 }; let dw = unsafe { d as u64 }; (sw as i32) * 10 + dw as i32",
        expected: 122,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 add/sub（wrapping，含进位/借位）"
    },
    // v27 M5-③：`u128` 的 &/|/^（两半各自做，无跨半进位）
    Case {
        name: "u128_bitops",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([0b1100, 0b1010]); let b = A([0b1010, 0b0110]); let pa = &a as *const A as *const u128; let pb = &b as *const A as *const u128; let x = unsafe { (*pa) & (*pb) }; let y = unsafe { (*pa) | (*pb) }; let z = unsafe { (*pa) ^ (*pb) }; let xw = unsafe { x as u64 }; let yw = unsafe { y as u64 }; let zw = unsafe { z as u64 }; (xw as i32) * 100 + (yw as i32) * 10 + zw as i32",
        expected: 946,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 的 &/|/^（8/14/6 ⇒ 946）",
    },
    // v27 M5-③：`u128` 的移位（跨半拼接；`(1<<70)>>70 = 1`、`(1<<4)>>2 = 4` ⇒ 14）
    Case {
        name: "u128_shift",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([1, 0]); let pa = &a as *const A as *const u128; let h = unsafe { (*pa) << 70 }; let back = unsafe { h >> 70 }; let l = unsafe { (*pa) << 4 }; let lr = unsafe { l >> 2 }; let hw = unsafe { back as u64 }; let lw = unsafe { lr as u64 }; (hw as i32) * 10 + lw as i32",
        expected: 14,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 移位（跨半 70 位往返 + 低位 4/2 ⇒ 14）",
    },
    // v27 M5-③：`u128` 的**变量**移位计数（非常量 ⇒ 走 place 分支）
    Case {
        name: "u128_shift_var",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([1, 0]); let pa = &a as *const A as *const u128; let n = core::hint::black_box(4u32); let s = unsafe { (*pa) << n }; let m = core::hint::black_box(70u32); let h = unsafe { (*pa) << m }; let back = unsafe { h >> m }; let w = unsafe { s as u64 }; let b = unsafe { back as u64 }; (w as i32) * 10 + b as i32",
        expected: 161,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 变量移位（1<<4=16、70 位往返=1 ⇒ 161）",
    },
    // v27 M5：有符号右移语义（-8 >> 1 = -4；同时覆盖夹具的负退出码比对）
    Case {
        name: "signed_shr",
        body: "let x: i32 = -8; let y = x >> 1; y",
        expected: -4,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "有符号右移（-8>>1 = -4）"
    },
    // 临时探针
    // 临时探针
    // v27 M5-③：`i128` 的**算术**右移（-8 >> 1 = -4；高半走 `sshr` 符号填充）
    Case {
        name: "i128_ashr",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([0xFFFF_FFFF_FFFF_FFF8, 0xFFFF_FFFF_FFFF_FFFF]); let p = &a as *const A as *const i128; let y = unsafe { *p >> 1 }; let z = unsafe { y as i64 }; z as i32",
        expected: -4,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "i128 算术右移（-8>>1 = -4，跨半符号填充）",
    },
    // v27 M5-③：128 位一元 `!`（此前掉进通用路径 ⇒ 运行期 AV；现在两半各自 `bnot`）
    Case {
        name: "u128_not",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([0, 0]); let p = &a as *const A as *const u128; let n = unsafe { !(*p) }; let lo = unsafe { n as u64 }; let hi = unsafe { (n >> 64) as u64 }; (lo as i32) + ((hi & 1) as i32)",
        expected: 0,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "128 位 `!`（全 1；lo=-1 + hi&1=1 ⇒ 0）",
    },
    // v27 M5-③：`u128::wrapping_neg`（= `!self + 1`，常量在左侧 + 128 位 checked 加法）
    Case {
        name: "u128_neg",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([1, 0]); let p = &a as *const A as *const u128; let n = unsafe { (*p).wrapping_neg() }; let lo = unsafe { n as u64 }; let hi = unsafe { (n >> 64) as u64 }; (lo as i32) + (hi as i32)",
        expected: -2,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 wrapping_neg（lo/hi 均为 -1 ⇒ -2）",
    },
    // v27 M5-③：`u128` 乘法（32 位肢的梳状乘法，纯 lowering）
    Case {
        name: "u128_mul",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([6, 0]); let b = A([7, 0]); let pa = &a as *const A as *const u128; let pb = &b as *const A as *const u128; let m = unsafe { (*pa).wrapping_mul(*pb) }; let w = unsafe { m as u64 }; w as i32",
        expected: 42,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 乘法（6*7=42）",
    },
    Case {
        name: "u128_mul_hi",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([0x1_0000_0001, 0]); let b = A([0x1_0000_0001, 0]); let pa = &a as *const A as *const u128; let pb = &b as *const A as *const u128; let m = unsafe { (*pa).wrapping_mul(*pb) }; let lo = unsafe { m as u64 }; let hi = unsafe { (m >> 64) as u64 }; ((hi as i32) * 100) + ((lo & 0xFF) as i32)",
        expected: 101,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 乘法跨肢进位（0x100000001² ⇒ 101）",
    },
    // v27 M5-③：`u128` 的 count_ones（两半 SWAR 相加；须用实参 place 取高半）
    Case {
        name: "u128_count_ones",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([0b1011, 0b1]); let p = &a as *const A as *const u128; let c = unsafe { (*p).count_ones() }; c as i32",
        expected: 4,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 count_ones（0b1011 + 高半 1 ⇒ 4）",
    },
    Case {
        name: "u128_leading_zeros",
        body: "#[repr(align(16))] struct A([u64; 2]); let a = A([0, 0]); let p = &a as *const A as *const u128; let z = unsafe { (*p).leading_zeros() }; z as i32",
        expected: 128,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M5",
        reason: "u128 leading_zeros（全 0 ⇒ 128）",
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
    // ── v27 M6：**靠 LLVM 管线的代码生成选项必须明确拒绝**（`[cap:CODEGEN_OPTS]`）──
    //   2026-10-19 实测：`-C lto` / `-C instrument-coverage` / `-Zsanitizer` / PGO /
    //   `-C control-flow-guard` / `-C prefer-dynamic` / `-C relocation-model`(非默认)
    //   此前**全部静默接受**——最坏情况是用户以为开了 ASan/CFG 防护而实际没开。
    //   这三条负向用例把"拒绝 + 点名缺口"钉住（flag 由用例名注入，见下方
    //   `flag_name_suffix` 的分派）。
    Case {
        name: "flag_lto",
        body: "42",
        extra: "",
        entry: "mainCRTStartup",
        expected: 0,
        expect_compile_fail: true,
        expect_compile_err: "CODEGEN_OPTS",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "`-C lto=fat` 必须明确拒绝：我们没有 bitcode 管线，静默忽略等于 LTO 没做",
    },
    Case {
        name: "flag_coverage",
        body: "42",
        extra: "",
        entry: "mainCRTStartup",
        expected: 0,
        expect_compile_fail: true,
        expect_compile_err: "CODEGEN_OPTS",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "`-C instrument-coverage` 必须明确拒绝：覆盖率计数与 `__llvm_covmap` 由 LLVM 插桩产生",
    },
    Case {
        name: "flag_sanitizer",
        body: "42",
        extra: "",
        entry: "mainCRTStartup",
        expected: 0,
        expect_compile_fail: true,
        expect_compile_err: "CODEGEN_OPTS",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "`-Zsanitizer=address` 必须明确拒绝：静默忽略会让用户以为有内存检测",
    },
    // ── v27 M6：**非 Win64 调用约定必须明确拒绝**（`[cap:CALL_CONV]`）──
    //   `extern "sysv64"` 在 rustc 里是 `Conv::X86(X86Conv::SysV64)`（RDI/RSI/RDX…），
    //   而我们按 Win64（RCX/RDX/R8/R9 + shadow space）打包 ⇒ 寄存器全错位。
    //   因为调用方/被调方都用本后端编译，本地"自洽"看不出来，一旦跨到 LLVM 编的代码
    //   或 `extern` 声明的外部函数就是错值 ⇒ 必须 fail-closed。
    Case {
        name: "neg_sysv64",
        body: "add_sysv(1, 2) as i32",
        extra: "#[unsafe(no_mangle)]\npub extern \"sysv64\" fn add_sysv(a: i64, b: i64) -> i64 { a + b }",
        entry: "mainCRTStartup",
        expected: 0,
        expect_compile_fail: true,
        expect_compile_err: "CALL_CONV",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "`extern \"sysv64\"` 不是 Win64 一族 ⇒ 明确拒绝（按 C 打包会静默错值）",
    },
    // ── v27 M6：`switchInt` 的 case 常量必须与判别值同类型（窄判别回归）──
    //   rustc 会把 `b[0] == 7`（u8）优化成 `switchInt(copy _2) -> [7: …]`；我们的
    //   switch 降级此前恒用 `constant(value as i32)` ⇒ `icmp eq i8 %d, i32 7` 是非法 IR，
    //   `-O`（优化管线默认开启）下 verifier 直接拒绝编译。这条在**真执行**上钉住修复
    //   （42 = 走对了分支）。
    Case {
        name: "narrow_switch_compare",
        body: "let mut b = [0u8; 4]; b[0] = 7; if b[0] == 7 { 42 } else { 1 }",
        extra: "",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "u8 判别值的 switchInt：case 常量的 IR 类型必须跟判别值（i8），不能恒 i32",
    },
    // ── v27 M6：`ConstantIndex` 投影必须真的加偏移（优化档专测）──
    //   优化过的 MIR 把 `a[1]` 写成 `ConstantIndex { offset: 1, .. }`（未优化 MIR 是
    //   `Index(_const)`）⇒ 旧实现落到"保持当前地址"的兜底分支，**静默读成 a[0]**。
    //   `pick()` 是 `#[inline(never)]`，防止 rustc 把整个数组折成常量而绕过内存访问。
    // ── v27 M6：`--emit=asm` 必须明确拒绝（`[cap:EMIT_KINDS]`）──
    //   此前是"成功但没产物"：编译退出 0，用户点名的 `.s` 根本不出现。
    Case {
        name: "flag_emit_asm",
        body: "7",
        extra: "",
        entry: "mainCRTStartup",
        expected: 0,
        expect_compile_fail: true,
        expect_compile_err: "EMIT_KINDS",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "`--emit=asm` 需要 LLVM 侧产物 ⇒ 明确拒绝，不许静默不产出",
    },
    Case {
        name: "u128_abi_param",
        // **Rust ABI 的 128 位形参**（`WA-52` 的护栏）：跨函数传 `u128` 必须正确。
        // 这条钉住"Rust ABI 走两整数槽"这一形态——改 ABI 编组时最先会红的就是它。
        body: "let v = core::hint::black_box((7u128 << 64) | 11);              if take_hi(v) == 7 { 42 } else { 7 }",
        extra: "#[inline(never)]\nfn take_hi(a: u128) -> u64 { (a >> 64) as u64 }\n",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "Rust ABI 的 u128 形参（两整数槽）必须正确——ABI 编组改动的护栏",
    },
    Case {
        name: "u128_abi_return",
        // **Rust ABI 的 128 位返回**：实测 LLVM 用 `RAX:RDX`（不是 XMM0），这条钉住该形态。
        body: "let w = make_wide(core::hint::black_box(9u64));              if (w >> 64) as u64 == 9 && (w & 0xFF) as u64 == 3 { 42 } else { 7 }",
        extra: "#[inline(never)]\nfn make_wide(a: u64) -> u128 { ((a as u128) << 64) | 3 }\n",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "Rust ABI 的 u128 返回（RAX:RDX）必须正确——与 C 家族的 XMM0 区分",
    },
    Case {
        name: "fnptr_scalar",
        // **函数指针**：此前 `take as fn(..)` 的常量被标量求值成 `nullptr`
        // （`GlobalAlloc::Function` 不在数据段路径里）⇒ 间接调用 AV。
        body: "let f: fn(i64) -> i64 = add1; if f(core::hint::black_box(6)) == 7 { 42 } else { 7 }",
        extra: "#[inline(never)]\nfn add1(a: i64) -> i64 { a + 1 }\n",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "函数指针常量必须取到符号地址（此前是 null）——间接调用通路的基础",
    },
    Case {
        name: "fnptr_u128",
        // **fn 指针 + 128 位实参**：形态来自操作数类型上的 `FnSig`（fn_abi_of_fn_ptr），
        // 不再因"拿不到被调方实例"而 fail-closed。
        body: "let f: fn(u128) -> u64 = take_hi; let v = core::hint::black_box((7u128) << 64 | 11); if f(v) == 7 { 42 } else { 7 }",
        extra: "#[inline(never)]\nfn take_hi(a: u128) -> u64 { (a >> 64) as u64 }\n",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "fn 指针调用的 u128 实参：形态问 FnSig（两整数槽），且函数地址必须真实",
    },
    Case {
        name: "dyn_args",
        // **虚方法带显式实参**：此前虚调用只传 receiver，显式实参被整批丢掉
        // （被调方读寄存器垃圾）——带参数的 `dyn Trait` 方法一律错值。
        body: "let s = S; let d: &dyn T = &s; if d.m(core::hint::black_box(4), core::hint::black_box(2)) == 42 { 42 } else { 7 }",
        extra: "trait T { fn m(&self, a: i64, b: i64) -> i64; }\nstruct S;\nimpl T for S { #[inline(never)] fn m(&self, a: i64, b: i64) -> i64 { a * 10 + b } }\n",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "虚调用必须把 receiver 之后的显式实参也传给方法（此前只传 receiver）",
    },
    Case {
        name: "dyn_u128",
        // 虚方法 + 128 位实参：形态来自虚方法实例的 `FnAbi`（Rust ABI 两槽）。
        body: "let s = S; let d: &dyn T = &s; let v = core::hint::black_box((9u128) << 64 | 11); if d.w(v) == 9 { 42 } else { 7 }",
        extra: "trait T { fn w(&self, a: u128) -> u64; }\nstruct S;\nimpl T for S { #[inline(never)] fn w(&self, a: u128) -> u64 { (a >> 64) as u64 } }\n",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "dyn 方法的 u128 实参：形态来自 FnAbi/FnSig，且显式实参要传到方法",
    },
    Case {
        name: "u128_div_non_pow2",
        // **非 2 的幂的 128 位除/余**：legalize 的 `wide_div` 分肢长除法（v27 M6-79）。
        body: "let v = core::hint::black_box(0x99aa_bbcc_ddee_ff00u128);              if (v / 7) as u64 == 0x99aa_bbcc_ddee_ff00u64 / 7 && (v % 7) as u64 == 0x99aa_bbcc_ddee_ff00u64 % 7 { 42 } else { 7 }",
        extra: "",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "128 位除/余非 2 的幂必须走分肢长除法且结果正确（O0）",
    },
    Case {
        name: "opt_u128_div_non_pow2",
        body: "let v = core::hint::black_box(0x99aa_bbcc_ddee_ff00u128);              if (v / 7) as u64 == 0x99aa_bbcc_ddee_ff00u64 / 7 && (v % 7) as u64 == 0x99aa_bbcc_ddee_ff00u64 % 7 { 42 } else { 7 }",
        extra: "",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "-O2 下 128 位除/余非 2 的幂同样必须正确",
    },
    Case {
        name: "i128_div_non_pow2",
        body: "let v = core::hint::black_box(-1000i128);              if (v / 7) as i64 == -1000i64 / 7 && (v % 7) as i64 == -1000i64 % 7 { 42 } else { 7 }",
        extra: "",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "有符号 128 位除/余必须按向零取整（绝对值 + 符号补回）",
    },
    Case {
        name: "u128_div_wide_divisor",
        // 除数**跨肢**（> 2^64）：`(2^64 + 100) / (2^63 + 3) = 2`，余数 = 2^64+100 - 2*(2^63+3) = 94
        body: "let a = core::hint::black_box((1u128 << 64) + 100);              let b = core::hint::black_box((1u128 << 63) + 3);              if a / b == 2 && a % b == 94 { 42 } else { 7 }",
        extra: "",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "除数也跨肢的 128 位除/余（长除法逐位比较必须正确）",
    },
    Case {
        name: "u128_div_pow2",
        // 宽值 ÷ **2 的幂常量**：改写层在降级（`x / 2^k ⇒ x >> k`、`x % 2^k ⇒ x & (2^k-1)`），
        // 因为 legalize 的 pass 在 `-O2` 下认不出被提升/load 过的除数常量（v27 M6-70）。
        // oracle 用手算常量：`(2^64 + 5000) / 1024 = 2^54 + 4`，`% 1024 = 904`。
        body: "let a = core::hint::black_box((1u128 << 64) + 5000);              if (a / 1024) as u64 == (1u64 << 54) + 4 && (a % 1024) as u64 == 904 { 42 } else { 7 }",
        extra: "",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "128 位除/余 2 的幂常量必须走移位/掩码并且结果正确",
    },
    Case {
        name: "opt_u128_div_pow2",
        // 同一条在 `-C opt-level=2` 下也要对——O2 的 MIR 形态不同（除数常量会被提升），
        // 这条专门钉"降级层识别常量"这条路径在两个优化档都成立。
        body: "let a = core::hint::black_box((1u128 << 64) + 5000);              if (a / 1024) as u64 == (1u64 << 54) + 4 && (a % 1024) as u64 == 904 { 42 } else { 7 }",
        extra: "",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "-O2 下 128 位除/余 2 的幂常量同样必须正确",
    },
    Case {
        name: "opt_u128_const_low_half",
        // **`u128` 常量的低半在 `-O2` 下必须真的落地**（v27 M6-66 修）：
        // 根因是 `Big::trunc_to_u64()` 对"装不下 u64"的值静默返回 0，而 `-O2` 下
        // 该常量被 intern 成 `Big`（O0 走位值通道）⇒ 拆肢时低半恒 0 ⇒ 取 `>> 56`
        // 得 0 而不是 0x99。这条用例把 `black_box`（内联成恒等）+ 常量 + 截断的组合钉住。
        body: "let v = core::hint::black_box(0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00u128);              ((v as u64) >> 56) as i32",
        extra: "",
        entry: "mainCRTStartup",
        // 0x99 = 153
        expected: 153,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "u128 常量低半在 -O2 必须正确（Big::trunc_to_u64 取低 64 位，不再退化为 0）",
    },
    Case {
        name: "opt_const_index",
        body: "let mut a = [0u8; 4]; a[1] = pick(); if a[1] == 7 && a[0] == 0 { 42 } else { 1 }",
        extra: "#[inline(never)]\nfn pick() -> u8 { 7 }",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "-C opt-level=2 下 `ConstantIndex` 投影必须加元素偏移（否则读成 a[0]）",
    },
    // ── v27 M6：**优化档 MIR 形态**批量覆盖（`opt_*` ⇒ harness 注入 `-C opt-level=2`）──
    //   为什么单独一组：rustc 优化过的 MIR 与 debug 形态**不同**（内联、常量提升、
    //   `switchInt` 多分支、`ConstantIndex`、slice 投影、128 位分字…），
    //   而此前 e2e 全是 debug 编译 ⇒ 这条路径没有任何用例（`ConstantIndex` 就是这样漏的）。
    Case {
        name: "opt_loop_call_inline",
        // 曾经在优化档 AV 崩溃（0xC0000005）的形状：**循环体内调用 + 内联计算**三者同时在场。
        // 根因不在 LICM（外提后 IR 逐行等价），而在发射侧**帧记账**：spill 槽 0 落在
        // `rsp - 8`（`callee_saved_bytes` 误把 fp 保存槽算进 rbp 之下），`call` 的返回地址
        // 恰好压进槽 0 ⇒ 下一轮 reload 拿代码地址当基址。根因已修（v27 M6-95），
        // `StackAddr` 外提回归默认；本用例就是那条崩溃形状的端到端守卫：1+4+9+16 = 30。
        body: "let mut acc = 0u64; let mut i = 1u64; while i <= 4 { acc = acc.wrapping_add(sq(i + seed())); i += 1; } acc as i32",
        extra: "#[inline(always)]\nfn sq(x: u64) -> u64 { x.wrapping_mul(x) }\n#[inline(never)]\nfn seed() -> u64 { 0 }",
        entry: "mainCRTStartup",
        expected: 30,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6-opt",
        reason: "循环内调用 + 内联计算（曾因 LICM 在优化档 AV 崩溃，现由流水线门控 + 本用例钉住）",
    },
    // ── v27 M6-㊽：**溢出检查开启**（`oc_*` ⇒ harness 注入 `-C overflow-checks=on`）──
    //   默认 e2e 编译是 debug-assertions 关 ⇒ 溢出检查关，所以"溢出必须 trap"这条语义
    //   此前**没有任何用例**（修 8 位溢出时才发现只能靠手工探针验）。
    //   期望值口径：`TRAP_EXIT`（panic handler 的 `ExitProcess(101)`），静默回绕会正常退出
    //   一个错值 ⇒ 必红。
    Case {
        name: "oc_u8_add_overflow_traps",
        // 8 位：`200u8 + 100u8` 必须 panic 而不是回绕成 44
        //（v27 M6-㊽ 修前实测 exit=44；修后 exit=101）
        body: "let a = core::hint::black_box(200u8); let b = core::hint::black_box(100u8); (a + b) as i32",
        extra: "",
        entry: "mainCRTStartup",
        expected: TRAP_EXIT,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: true,
        known_failure: false,
        phase: "v27 M6-ovf",
        reason: "8 位加法溢出在 overflow-checks=on 下必须 trap",
    },
    Case {
        name: "oc_i8_sub_overflow_traps",
        // 有符号 8 位**下溢**：`-100i8 - 100i8` 必须 panic
        body: "let a = core::hint::black_box(-100i8); let b = core::hint::black_box(100i8); (a - b) as i32",
        extra: "",
        entry: "mainCRTStartup",
        expected: TRAP_EXIT,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: true,
        known_failure: false,
        phase: "v27 M6-ovf",
        reason: "有符号 8 位减法下溢必须 trap（比较法要同时判上下界）",
    },
    Case {
        name: "oc_u8_mul_overflow_traps",
        // 8 位乘法溢出：`200u8 * 2u8 = 400` 超出 u8 ⇒ 必须 panic
        //（v27 M6-㊿ 修前实测 exit=144 = 回绕）
        body: "let a = core::hint::black_box(200u8); let b = core::hint::black_box(2u8); (a * b) as i32",
        extra: "",
        entry: "mainCRTStartup",
        expected: TRAP_EXIT,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: true,
        known_failure: false,
        phase: "v27 M6-ovf",
        reason: "8 位乘法溢出必须 trap（乘法在 I64 里算再比区间）",
    },
    Case {
        name: "oc_u16_mul_overflow_traps",
        // 16 位乘法：`1000 * 1000 = 1e6` 超出 u16 ⇒ 必须 panic。
        // 这条专门钉"乘法必须在 I64 里算"：I32 里 1e6 不会回绕但 65535*65535 ≈ 4.29e9 会。
        body: "let a = core::hint::black_box(1000u16); let b = core::hint::black_box(1000u16); (a * b) as i32",
        extra: "",
        entry: "mainCRTStartup",
        expected: TRAP_EXIT,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: true,
        known_failure: false,
        phase: "v27 M6-ovf",
        reason: "16 位乘法溢出必须 trap（乘积超出 I32 时不能先回绕）",
    },
    Case {
        name: "oc_u128_add_overflow_traps",
        // **宽值**（128 位）加法溢出：`u128::MAX + 1` ⇒ 必须 panic。
        // 走的是 `narrow_overflow` pass 的**宽值分支**（不扩宽，直接用回绕值比较）。
        body: "let a = core::hint::black_box(u128::MAX); let b = core::hint::black_box(1u128); (a + b) as i32",
        extra: "",
        entry: "mainCRTStartup",
        expected: TRAP_EXIT,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: true,
        known_failure: false,
        phase: "v27 M6-ovf",
        reason: "128 位无符号加法溢出必须 trap（宽值分支用 `value < a` 判）",
    },
    Case {
        name: "oc_i128_add_overflow_traps",
        // **有符号**宽值溢出：判据是 `((a ^ r) & (b ^ r)) < 0`（走 Bxor/Band）——
        // 这条专门钉那个有符号宽值分支，别让它在没人跑的情况下烂掉。
        body: "let a = core::hint::black_box(i128::MAX); let b = core::hint::black_box(1i128); (a + b) as i32",
        extra: "",
        entry: "mainCRTStartup",
        expected: TRAP_EXIT,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: true,
        known_failure: false,
        phase: "v27 M6-ovf",
        reason: "128 位有符号加法溢出必须 trap（符号判据走 bxor/band）",
    },
    Case {
        name: "oc_u16_mul_no_overflow",
        // 16 位**不**溢出：`200u16 * 100u16 = 20000` ⇒ 正常返回 20000 的低 16 位（仍是 20000）
        // 用来防止"把检查做成一律 trap"这种过度修正。
        body: "let a = core::hint::black_box(200u16); let b = core::hint::black_box(100u16); (a * b) as i32",
        extra: "",
        entry: "mainCRTStartup",
        expected: 20000,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6-ovf",
        reason: "不溢出的窄乘法必须正常返回（防止检查过度触发）",
    },
    Case {
        name: "opt_many_params_loop_call",
        // **栈形参 × 循环 × 调用**（v27 M6 的开放缺陷形状）：9 个 u64 形参里后 5 个走栈，
        // 循环体里调一个 `#[inline(never)]` 函数，上界由**栈形参**给出。
        // 这条在 JIT 侧（`test_jit_spill_pressure_loop_with_call`）是确定性错值/挂起；
        // 本用例检查 **rustc 路径**是否同样中招——若通过，说明该缺陷只影响 JIT 侧
        // （rustc 的 MIR 降级走栈槽而非块参数），据此可以给它定优先级。
        // 期望：Σ_{i=1..5}(step(i)) = 4+7+10+13+16 = 50；再加 5*(a-b+c-d+e-f+g-h) = 5*(-4) = -20 ⇒ 30
        body: "let (p0, p1, p2, p3, p4, p5, p6, p7, p8) = \
               (1u64, 2u64, 3u64, 4u64, 5u64, 6u64, 7u64, 8u64, 5u64); \
               let mut acc = 0u64; let mut i = 1u64; \
               while i <= p8 { acc = acc.wrapping_add(step(i)).wrapping_add(p0); \
               acc = acc.wrapping_sub(p1); acc = acc.wrapping_add(p2); acc = acc.wrapping_sub(p3); \
               acc = acc.wrapping_add(p4); acc = acc.wrapping_sub(p5); \
               acc = acc.wrapping_add(p6); acc = acc.wrapping_sub(p7); i += 1; } acc as i32",
        extra: "#[inline(never)]\nfn step(x: u64) -> u64 { x.wrapping_mul(3).wrapping_add(1) }",
        entry: "mainCRTStartup",
        expected: 30,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6-opt",
        reason: "栈形参 × 循环 × 调用：检查 rustc 路径是否与 JIT 侧同样中招",
    },
    Case {
        name: "opt_inline_small",
        // 注意：**不要在循环体里再调用一个 `#[inline(never)]` 函数**——那种组合
        // （`while … { acc += sq(i + seed()) }`）目前在优化档会 AV 崩溃（0xC0000005），
        // 已登记为开放缺陷（见 `docs/archive/forge-rustc-v27-milestones.md`，含源码）。
        // 这里保留"内联 + 循环"这一稳定形态作为回归。
        body: "let mut acc = 0u64; let mut i = 1u64; while i <= 4 { acc = acc.wrapping_add(sq(i)); i += 1; } acc as i32",
        extra: "#[inline(always)]\nfn sq(x: u64) -> u64 { x.wrapping_mul(x) }",
        entry: "mainCRTStartup",
        expected: 30,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6-opt",
        reason: "内联后的算术（1+4+9+16=30）；优化档会把 `sq` 内联进循环体",
    },
    Case {
        name: "opt_switch_multi",
        body: "let mut acc = 0; let mut i = 0u8; while i < 6 { acc += match i { 0 => 1, 1 => 10, 2 => 100, 3 => 1000, 4 => 7, _ => 9 }; i += 1; } acc",
        extra: "",
        entry: "mainCRTStartup",
        expected: 1127,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6-opt",
        reason: "u8 判别的多分支 `switchInt`（case 常量类型须跟判别值 i8）",
    },
    Case {
        name: "opt_slice_sum",
        body: "let data = [1u16, 2, 3, 4, 5]; let s: &[u16] = &data[1..4]; let mut acc = 0u32; for v in s { acc += *v as u32; } acc as i32",
        extra: "",
        entry: "mainCRTStartup",
        expected: 9,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6-opt",
        reason: "`&data[1..4]`：Subslice 投影要按元素大小前移数据指针（2+3+4=9）",
    },
    Case {
        name: "opt_promoted_static",
        body: "let a: &[u8; 4] = b\"ABCD\"; let mut acc = 0u32; let mut i = 0usize; while i < 4 { acc += a[i] as u32; i += 1; } acc as i32",
        extra: "",
        entry: "mainCRTStartup",
        expected: 266,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6-opt",
        reason: "常量提升的静态字节串（65+66+67+68=266）+ 常量下标读取",
    },
    Case {
        name: "opt_nested_field",
        body: "let p = Pair { a: Inner { x: 3, y: 4 }, b: 5 }; let mut acc = 0i64; acc += p.a.x as i64; acc += p.a.y as i64; acc += p.b as i64; (acc * 3) as i32",
        extra: "struct Inner { x: i32, y: i32 }\nstruct Pair { a: Inner, b: i32 }",
        entry: "mainCRTStartup",
        expected: 36,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6-opt",
        reason: "嵌套字段投影（优化档 MIR 会把整个结构体放进一个槽里逐字段读）",
    },
    Case {
        name: "opt_u128_arith",
        // 期望值可手算：r = 3*(2^70+5)+1 = 3*2^70+16 ⇒ r>>70 = 3 ⇒ 3+39 = 42。
        // `wide()` 是 `#[inline(never)]`，保证 128 位运算真的在运行期发生（不被常量折叠）。
        body: "let a: u128 = (1u128 << 70) + 5; let r = wide(a); ((r >> 70) as i32) + 39",
        extra: "#[inline(never)]\nfn wide(x: u128) -> u128 { x.wrapping_mul(3).wrapping_add(1) }",
        entry: "mainCRTStartup",
        expected: 42,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6-opt",
        reason: "128 位算术在优化档下（wide_split 分字 + const-fold 交互）",
    },
    Case {
        name: "opt_bounds_panic",
        body: "let a = [1u8, 2u8]; let i = idx(); a[i] as i32",
        extra: "#[inline(never)]\nfn idx() -> usize { 5 }",
        entry: "mainCRTStartup",
        expected: 101,
        expect_compile_fail: false,
        expect_compile_err: "",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6-opt",
        reason: "越界索引必须走 panic 路径（panic handler 走 ExitProcess(101)），不能静默读垃圾",
    },
    // ── v27 M6：C 变参**定义**必须明确拒绝（`[cap:VARARGS_DEF]`）──
    //   此前：调用这种函数会让 `check_arg_count_consistency` 的 `assert_eq!`
    //   **panic 掉 rustc**（"the compiler unexpectedly panicked. This is a bug"）；
    //   修好 panic 后，剩下的路径只报裸 `unsupported intrinsic: va_end`。
    //   现在定义侧就按缺口 ID 拒绝并说明原因（真做需要被调方序言建寄存器保存区）。
    Case {
        name: "neg_c_variadic",
        body: "unsafe { add_variadic(1, 2i64) as i32 }",
        extra: "#![feature(c_variadic)]\n#![allow(incomplete_features)]\n#[unsafe(no_mangle)]\npub unsafe extern \"C\" fn add_variadic(n: i32, mut ap: ...) -> i64 { let _ = n; let _ = &mut ap; 7 }",
        entry: "mainCRTStartup",
        expected: 0,
        expect_compile_fail: true,
        expect_compile_err: "VARARGS_DEF",
        expect_trap: false,
        known_failure: false,
        phase: "v27 M6",
        reason: "C 变参定义不支持 ⇒ 明确拒绝（此前会让 rustc ICE 在 ABI 计数断言上）",
    },
];

/// 生成 no_std 程序源码。
fn make_source(body: &str, extra: &str, entry: &str) -> String {
    format!(
        "#![no_std]\n#![no_main]\n{extra}\n#[unsafe(no_mangle)]\npub extern \"C\" fn {entry}() -> i32 {{\n    {body}\n}}\n\nunsafe extern \"system\" {{\n    #[link_name = \"ExitProcess\"]\n    fn forge_panic_exit(code: u32) -> !;\n}}\n#[panic_handler]\nfn panic(_info: &core::panic::PanicInfo) -> ! {{\n    unsafe {{ forge_panic_exit({PANIC_ABORT_CODE}) }}\n}}\n"
    )
}

/// **关闭 Windows 错误报告（WER）弹窗**——`expect_trap` 用例会主动 `ud2`/越界，
/// 未处理异常默认会拉起 WER 对话框把进程**挂住**（15 s 超时那句"产物真挂起"就是它）。
///
/// 子进程**继承**父进程的 error mode，所以在 harness 进程里设一次即可覆盖所有用例产物。
/// 这条同时解释了账本里长期记着的"偶发 timeout（挂起）"——那类瞬态大半是 WER 排队/
/// 弹窗的宿主侧行为，不是错编译。幂等：重复调用无害。
#[cfg(windows)]
fn suppress_wer_popups() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX
        const MODE: u32 = 0x0001 | 0x0002 | 0x8000;
        unsafe extern "system" {
            fn SetErrorMode(mode: u32) -> u32;
        }
        unsafe {
            SetErrorMode(MODE);
        }
    });
}

#[cfg(not(windows))]
fn suppress_wer_popups() {}

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
    // 每个用例都可能跑出**未处理异常**（`expect_trap`），先关掉 WER 弹窗：
    // 否则异常的产物会挂 15 s 超时（见 suppress_wer_popups 的说明）。
    suppress_wer_popups();
    // v27 M5 并行化：**每个用例一个子目录**，编译也在其中进行（`current_dir`），
    // 否则多个 rustc 会在同一 workdir 里争抢对象/临时文件（实测并行时出现
    // "退出码是别的用例的期望值" 与大量 compile failed）。
    let case_dir = workdir.join(case.name);
    std::fs::create_dir_all(&case_dir).map_err(|e| e.to_string())?;
    let src = case_dir.join(format!("{}.rs", case.name));
    let exe = case_dir.join(format!("{}.exe", case.name));
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
    // v27 M6：`flag_*` 用例注入"应被明确拒绝"的代码生成选项
    //（断言见用例的 `expect_compile_err = "CODEGEN_OPTS"`）。
    match case.name {
        "flag_lto" => rustc_args.extend(["-C".to_string(), "lto=fat".to_string()]),
        "flag_coverage" => rustc_args.extend(["-C".to_string(), "instrument-coverage".to_string()]),
        "flag_sanitizer" => rustc_args.extend(["-Z".to_string(), "sanitizer=address".to_string()]),
        // `--emit=asm`：本后端只产对象文件 ⇒ 必须按 `[cap:EMIT_KINDS]` 拒绝
        //（此前**静默什么都不生成**：退出码 0、用户要的文件不存在）。
        "flag_emit_asm" => rustc_args.extend(["--emit=asm".to_string()]),
        _ => {}
    }
    // `opt_*` 用例用 `-C opt-level=2` 编译：**优化过的 MIR 形态不同**（例如数组常量
    // 下标从 `Index(_const)` 变成 `ConstantIndex`），只有开优化才走那条 lowering
    // （v27 M6：`ConstantIndex` 曾被静默忽略 ⇒ `buf[1]` 读成 `buf[0]`）。
    // 这类用例专门覆盖"带优化的路径"——其余用例都是 debug（无优化）编译。
    if case.name.starts_with("opt_") {
        rustc_args.extend(["-C".to_string(), "opt-level=2".to_string()]);
    }
    // `oc_*` 用例用 `-C overflow-checks=on` 编译：默认（debug-assertions 关）**溢出检查是关的**，
    // 于是"溢出必须 panic"这条语义在 e2e 里根本没有用例覆盖（v27 M6-㊽ 修 8 位溢出时才发现：
    // 修完只能靠手工探针验）。这类用例专覆盖检查开启后的路径。
    if case.name.starts_with("oc_") {
        rustc_args.extend(["-C".to_string(), "overflow-checks=on".to_string()]);
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
        .envs(extra_envs.iter().copied())
        .current_dir(&case_dir);
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
        return Err("expected compile failure but compilation succeeded (门控失效)".to_string());
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
        // v27 M5：**先**与用例期望对齐，再判 trap。否则"负的期望退出码"（如 `-4`）
        // 会被 `is_trap_code` 误判为 NTSTATUS（`-4 as u32 = 0xFFFFFFFC >= 0xC0000000`），
        // 导致正确的有符号结果被报成"未预期的异常终止"。
        Ok(c) if c == case.expected => Ok(c),
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

    // ── 用例级并行（v27 M5）──
    // 每个 worker 独立"编译 + 运行"一个用例；产物按用例名隔离（`<name>.rs`/`<name>.exe`），
    // 无共享可变状态。结果**按 CASES 顺序回收**后再串行分类 ⇒ 输出/事件/汇总与串行完全一致。
    // 并发度：`FORGE_E2E_JOBS`（显式覆盖）> 可用核数；1 = 串行（原行为）。
    let selected: Vec<&Case> = CASES
        .iter()
        .filter(|c| only.as_ref().is_none_or(|n| n == c.name))
        .collect();
    let jobs = std::env::var("FORGE_E2E_JOBS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        })
        .clamp(1, 16)
        .min(selected.len().max(1));
    println!("cases: {} 并行度: {}", selected.len(), jobs);
    let outcomes: Vec<Result<i32, String>> = if jobs <= 1 {
        selected.iter().map(|c| run_case(c, &workdir)).collect()
    } else {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let next = AtomicUsize::new(0);
        let slots: Mutex<Vec<Option<Result<i32, String>>>> =
            Mutex::new((0..selected.len()).map(|_| None).collect());
        std::thread::scope(|s| {
            for _ in 0..jobs {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= selected.len() {
                            break;
                        }
                        let r = run_case(selected[i], &workdir);
                        slots.lock().expect("e2e slots poisoned")[i] = Some(r);
                    }
                });
            }
        });
        let mut out = Vec::with_capacity(selected.len());
        for slot in slots.into_inner().expect("e2e slots poisoned") {
            out.push(slot.expect("e2e slot not filled"));
        }
        out
    };
    for (case, result) in selected.iter().copied().zip(outcomes) {
        match result {
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
///
/// 两路径退出码必须一致且等于期望值（产物指令序列一致性由 determinism.rs
/// 的 -Z threads 矩阵守护）。
///
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
            .arg(
                "link-args=/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib",
            )
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
