//! 端到端真值测试：用 forge-rustc 作为 rustc codegen backend 编译 no_std 程序，
//! 运行产物并校验退出码（入口函数返回值 = 进程退出码）。
//!
//! 这是本 crate 的**统一测试体系**（P4.5）：
//! - 用例清单即 `CASES` 数组（58 项，含 `known_failure` + `reason` 回归探针）
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
        known_failure: true,
        phase: "P2",
        reason: "len 读错 (exit=0): String 的 cap/len 槽运行期值错，与 vec_push 同源（[WA-11] 活区间问题）",
    },
    Case {
        name: "dyn_trait_call",
        body: "let d = Dog; let s: &dyn Speak = &d; s.speak()",
        expected: 7,
        extra: "trait Speak { fn speak(&self) -> i32; }\nstruct Dog;\nimpl Speak for Dog { fn speak(&self) -> i32 { 7 } }",
        entry: "mainCRTStartup",
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
        known_failure: false,
        phase: "P4 static",
        reason: "",
    },
    Case {
        name: "static_mut_counter",
        body: "unsafe { CNT += 1; CNT }",
        expected: 1,
        extra: "static mut CNT: i32 = 0;",
        entry: "mainCRTStartup",
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
        known_failure: false,
        phase: "P9 niche",
        reason: "",
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

    // 编译：no_std + no_main + panic=abort + overflow-checks=off（当前后端不支持
    // checked 算术的 (value, bool) 元组结果，见 Phase 3 聚合布局计划）
    // entry=="main"（fn main 形态）：不指定 /ENTRY，MSVC 默认入口自动转调 main
    let entry_args = if case.entry == "main" {
        "/SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib".to_string()
    } else {
        "/ENTRY:mainCRTStartup /SUBSYSTEM:CONSOLE /DEFAULTLIB:kernel32.lib /DEFAULTLIB:vcruntime.lib".to_string()
    };
    let compile = Command::new("rustc")
        .arg("-Zcodegen-backend=".to_string() + &backend_dll().display().to_string())
        .args([
            "-C",
            "panic=abort",
            "-C",
            "overflow-checks=off",
            "--edition",
            "2024",
        ])
        .arg("-C")
        .arg(format!("link-args={entry_args}"))
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .output()
        .map_err(|e| format!("failed to spawn rustc: {e}"))?;

    if !compile.status.success() {
        let stderr = String::from_utf8_lossy(&compile.stderr);
        return Err(format!(
            "compile failed: {}\n{}",
            compile.status,
            stderr.lines().take(10).collect::<Vec<_>>().join("\n")
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
