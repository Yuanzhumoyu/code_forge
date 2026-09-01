//! 端到端真值测试：用 forge-rustc 作为 rustc codegen backend 编译 no_std 程序，
//! 运行产物并校验退出码（入口函数返回值 = 进程退出码）。
//!
//! 这是本 crate 的**统一测试体系**（P4.5）：
//! - 用例清单即 `CASES` 数组（81 项，含 `known_failure` + `reason` 回归探针）
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
/// `toolchain: nightly-2026-08-07` 单点同步；bump 时两处一起改）。
/// `cargo +nightly` 显式 channel 不受 rustup override 影响，必须用钉版
/// 字面量保证 CI/本地一致（rustc_compat.rs 适配层按此版本维护）。
const NIGHTLY: &str = "nightly-2026-08-07";

/// 本机工具链选择：CI 用钉版 NIGHTLY；本机可用 `FORGE_E2E_NIGHTLY`
/// 环境变量覆盖（如 `nightly`——浮动 channel 指向已安装目录，避免
/// 版本化目录缺失/rustup 安装权限限制；本机 .rustup 被 Defender 拦截
/// 时用 `RUSTUP_HOME=target\rustup_home` junction 重定向）。
fn e2e_toolchain() -> String {
    std::env::var("FORGE_E2E_NIGHTLY").unwrap_or_else(|_| NIGHTLY.to_string())
}

/// 用例：程序体（i32 表达式）+ 期望退出码。
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
        extra: "extern crate alloc;\nuse alloc::boxed::Box;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 4096] = [0; 4096];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe { core::ptr::addr_of_mut!(HEAP) as *mut u8 } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        known_failure: false,
        phase: "P6 alloc",
        reason: "",
    },
    Case {
        name: "box_write",
        body: "let mut b = Box::new(7i32); *b = 42; *b",
        expected: 42,
        extra: "extern crate alloc;\nuse alloc::boxed::Box;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 4096] = [0; 4096];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe { core::ptr::addr_of_mut!(HEAP) as *mut u8 } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
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
        known_failure: false,
        phase: "P6 alloc",
        reason: "",
    },
    // ── Vec/String 完整运行（本轮回归未修，显式已知失败清单）：push 写元素时 SEGV。
    //    最小复现见 README 限制区⑧（probe_n n=5，~40 行）：grow 链 finish_grow 的 sret
    //    copy_agg 在高压 spill 下写 [0]（sret_ptr 值丢失——活区间/spill 决策 bug，
    //    指向 forge-codegen regalloc）。修复后移除 known_failure 转硬断言。──
    Case {
        name: "vec_push",
        body: "let mut v = alloc::vec::Vec::new(); v.push(1); v.push(2); v.len() as i32",
        expected: 2,
        extra: "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe { core::ptr::addr_of_mut!(HEAP) as *mut u8 } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        known_failure: false,
        phase: "P2",
        reason: "2026-09 转正：Windows x64 栈参数（5dba34b）+ spilled 寄存器参数收参到 spill 槽（move_args 的 #spilled_int_receive——mod.rs 210 写槽依赖 entry vreg 值）——grow 链（RawVec grow_amortized/finish_grow/Global::grow_impl_runtime）在栈参数 + 高压 spill 下正确执行，exit=2",
    },
    Case {
        name: "vec_string",
        body: "let s = alloc::string::String::from(\"hi\"); s.len() as i32",
        expected: 2,
        extra: "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe { core::ptr::addr_of_mut!(HEAP) as *mut u8 } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        known_failure: false,
        phase: "P2",
        reason: "2026-09 转正：Slice 常量落盘（&str 字面量 ConstValue::Slice → rodata 数据段 + 写槽 ptr@0/len@8，statement.rs + mod.rs 实参拆 lo/hi）+ PtrMetadata（fat pointer 的 len，rvalue.rs fat_ptr_metadata）——String::from(\"hi\") 的 &str 实参正确传 ptr+len",
    },
    Case {
        name: "vec_from_slice",
        body: "let v = alloc::vec::Vec::from(&[1u8, 2, 3][..]); v.len() as i32",
        expected: 3,
        extra: "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe { core::ptr::addr_of_mut!(HEAP) as *mut u8 } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        known_failure: false,
        phase: "P6 alloc",
        reason: "2026-09 转正：promoted 数组引用（&[1,2,3] = const promoted[0]，Unevaluated eval → GlobalAlloc::Memory → intern_promoted 落盘 rodata，statement.rs 写 8B 槽）+ Unsize cast &[T;N]→&[T] 生成 len 元数据（statement.rs slice 分支 hi=N）+ 收参跳过 ZST 参数（mod.rs P4.6：RangeFull 在有效参数前时 fat ptr 拆包错位根因）——Vec::from 的 &[u8] 实参正确传 ptr+len",
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
        known_failure: false,
        phase: "P1 agg",
        reason: "",
    },
    // ── SIMD 门控（B3 前置）：向量参与跨函数 ABI（参数/返回）在主库
    //    ymm-abi-plan 就绪前编译期拒绝，绝不产出静默错值 exe。──
    Case {
        name: "simd_abi_gated",
        body: "let v = core::simd::Simd::<f32, 4>::from_array([1.0, 2.0, 3.0, 4.0]); v[0] as i32",
        expected: -1, // 编译失败：向量 ABI 门控（期望 compile error，非运行结果）
        extra: "#![feature(portable_simd)]",
        entry: "mainCRTStartup",
        expect_compile_fail: true,
        expect_compile_err: "向量 ABI",
        known_failure: false,
        phase: "B3 simd",
        reason: "编译期拒绝：Simd 返回/参数依赖主库向量调用约定（ymm-abi-plan S1-S5），未就绪前 lower_body 门控报错（失败即报错，不产静默错值）",
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
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
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
        known_failure: false,
        phase: "F1 iter",
        reason: "2026-08 转正：WA-24（switchInt const 判别折叠）——Iter::next 的 `switchInt(UbChecks)` 判别是编译期常量（core::intrinsics::ub_checks 求值 false），折叠后只生成目标分支，不可达的 precondition_check 调用块不再 lower → 无 LNK2019",
    },
    Case {
        name: "vec_iter_enumerate",
        body: "let mut v = alloc::vec::Vec::new(); v.push(10); v.push(20); v.push(30); let mut s = 0; for (i, x) in v.iter().enumerate() { s += i as i32 * x; } s",
        expected: 80, // 0*10 + 1*20 + 2*30
        extra: "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe { core::ptr::addr_of_mut!(HEAP) as *mut u8 } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
        known_failure: false,
        phase: "F1 iter",
        reason: "2026-09 转正：WA-29（Niche CONSTRUCT 写入宽度按 tag 标量宽度——8 字节指针 tag 恒 movl 32 位写残留高 4 字节，判别读 I64 读出垃圾 → None 误判 Some → **x 解引用 null SEGV）",
    },
    Case {
        name: "string_concat_len",
        body: "let mut s = alloc::string::String::from(\"ab\"); s.push('c'); s.push('d'); s.len() as i32",
        expected: 4,
        extra: "extern crate alloc;\nuse core::alloc::{GlobalAlloc, Layout};\nstatic mut HEAP: [u8; 8192] = [0; 8192];\nstruct A;\nunsafe impl GlobalAlloc for A {\n    unsafe fn alloc(&self, _l: Layout) -> *mut u8 { unsafe { core::ptr::addr_of_mut!(HEAP) as *mut u8 } }\n    unsafe fn dealloc(&self, _p: *mut u8, _l: Layout) {}\n}\n#[global_allocator]\nstatic ALLOC: A = A;",
        entry: "mainCRTStartup",
        expect_compile_fail: false,
        expect_compile_err: "",
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
        known_failure: false,
        phase: "C1 dwarf",
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
        known_failure: false,
        phase: "D intrinsics",
        reason: "",
    },
    // ── F2 负向测试：编译失败断言（失败即报错）──
    Case {
        name: "neg_global_asm",
        body: "42",
        expected: -1,
        extra: "core::arch::global_asm!(\"nop\");",
        entry: "mainCRTStartup",
        expect_compile_fail: true,
        expect_compile_err: "global_asm",
        known_failure: false,
        phase: "F2 neg",
        reason: "MonoItem::GlobalAsm → dcx().err（WA-03）",
    },
    Case {
        name: "neg_std_program",
        body: "let s = String::from(\"hi\"); s.len() as i32",
        expected: -1,
        extra: "",
        entry: "mainCRTStartup",
        expect_compile_fail: true,
        expect_compile_err: "",
        known_failure: false,
        phase: "F2 neg",
        reason: "std 程序（no_std 上下文用 String）→ 编译失败（std 未链接/未声明 extern crate std）",
    },
];

/// 生成 no_std 程序源码。
fn make_source(body: &str, extra: &str, entry: &str) -> String {
    format!(
        "#![no_std]\n#![no_main]\n{extra}\n#[unsafe(no_mangle)]\npub extern \"C\" fn {entry}() -> i32 {{\n    {body}\n}}\n\n#[panic_handler]\nfn panic(_info: &core::panic::PanicInfo) -> ! {{\n    loop {{}}\n}}\n"
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
    let mut rustc_args = vec![
        "-Zcodegen-backend=".to_string() + &backend_dll().display().to_string(),
        "-C".to_string(),
        "panic=abort".to_string(),
        "--edition".to_string(),
        "2024".to_string(),
    ];
    // 用例级溢出开关：`overflow_off` 用例（需要 wrapping 语义的）显式关闭。
    if case.name.contains("overflow_off") {
        rustc_args.push("-C".to_string());
        rustc_args.push("overflow-checks=off".to_string());
    }
    // C1 DebugInfo：`debuginfo_` 前缀用例加 `-C debuginfo=1`（line-tables）。
    if case.name.starts_with("debuginfo_") {
        rustc_args.push("-C".to_string());
        rustc_args.push("debuginfo=1".to_string());
    }
    let compile = Command::new("rustc")
        .args(&rustc_args)
        .arg("-C")
        .arg(format!("link-args={entry_args}"))
        .arg(&src)
        .arg("-o")
        .arg(&exe)
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

    // 运行并取退出码（带超时：panic handler 是 loop{}，assert 失败会挂起）
    let mut child = Command::new(&exe)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", exe.display()))?;
    let mut code = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            code = status.code();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    match code {
        Some(c) => Ok(c),
        None => {
            let _ = child.kill();
            Err("timeout (挂起：可能 assert 失败进入 panic loop)".to_string())
        }
    }
}

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
                if case.known_failure {
                    // 回归探针（P3.3）：输出失败模式（reason），转正时对照验证
                    println!(
                        "KNOWN {:<16} {msg} [{}] reason: {}",
                        case.name, case.phase, case.reason
                    );
                    known_failures.push(case.name);
                } else {
                    println!("FAIL  {:<16} {msg}", case.name);
                    unexpected_failures.push(format!("{}: {msg}", case.name));
                }
            }
            Err(e) => {
                let msg = format!("error: {e}");
                if case.known_failure {
                    println!(
                        "KNOWN {:<16} {msg} [{}] reason: {}",
                        case.name, case.phase, case.reason
                    );
                    known_failures.push(case.name);
                } else {
                    println!("FAIL  {:<16} {msg}", case.name);
                    unexpected_failures.push(format!("{}: {msg}", case.name));
                }
            }
        }
    }

    let _ = std::fs::remove_dir_all(&workdir);

    println!("\n=== e2e 汇总 ===");
    println!("passed: {passed}/{}", CASES.len());
    if !known_failures.is_empty() {
        println!("known failures (待 Phase 2/3): {:?}", known_failures);
    }
    if !unexpected_failures.is_empty() {
        panic!("unexpected failures:\n{}", unexpected_failures.join("\n"));
    }
    assert!(passed > 0, "no cases passed — backend broken");
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
    let rustflags = format!(
        "-Zcodegen-backend={} -C panic=abort -C overflow-checks=off -C link-arg=/SUBSYSTEM:CONSOLE -C link-arg=/DEFAULTLIB:kernel32.lib -C link-arg=/DEFAULTLIB:vcruntime.lib",
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
        .args([toolchain.as_str(), "-Z", "build-std=core", "build"])
        .output()
        .map_err(|e| format!("failed to spawn cargo: {e}"))
        .expect("cargo spawn");
    assert!(
        build.status.success(),
        "cargo build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

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

