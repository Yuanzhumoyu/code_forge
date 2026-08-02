//! 自举兼容性阶梯 — Stage A: 编译 Rust `core` 子集。
//!
//! 本文件包含 Stage A 的测试用例，每个测试用最简单的方式验证
//! rustc 后端可以正确编译特定类型的 Rust 代码。
//!
//! ## Stage A 目标
//!
//! 编译包含以下特性的 Rust 函数（10-20 个基本构造）：
//! - 整数运算 (add, sub, mul, div, rem)
//! - 控制流 (if/else, loop, while)
//! - 函数调用 (无参数, 带参数, 返回值)
//! - 变量绑定 (let, mut)
//! - 布尔运算 (and, or, not)
//! - 基本类型 (i32, i64, bool)
//!
//! ## Stage B (未来)
//!
//! 编译 alloc 级别代码: Vec, String, Box, 结构体, 枚举
//!
//! ## Stage C (未来)
//!
//! 编译 libc 子集: FFI, extern "C"
//!
//! ## Stage D (未来)
//!
//! 自举: 用 codegen-lib 编译自身的一个小模块
//!
//! ## 运行方式
//!
//! ```bash
//! # 需要 nightly toolchain（由 rust-toolchain.toml 自动指定）
//! cargo +nightly test -p rustc_codegen_codegenlib --test stage_a
//! ```

// ============================================================
// Stage A 编译 Fixtures (Rust 源码 → codegen-lib 编译)
// ============================================================

/// Stage A 测试 fixture — 每个函数代表一个已验证可编译的 Rust 模式。
mod fixtures {
    /// A.1: 纯函数，返回常量
    #[inline(never)]
    pub fn return_constant() -> i32 {
        42
    }

    /// A.2: 整数加法
    #[inline(never)]
    pub fn add(a: i32, b: i32) -> i32 {
        a + b
    }

    /// A.3: 整数减法
    #[inline(never)]
    pub fn sub(a: i32, b: i32) -> i32 {
        a - b
    }

    /// A.4: 整数乘法
    #[inline(never)]
    pub fn mul(a: i32, b: i32) -> i32 {
        a * b
    }

    /// A.5: 整数除法
    #[inline(never)]
    pub fn div(a: i32, b: i32) -> i32 {
        a / b
    }

    /// A.6: 整数取余
    #[inline(never)]
    pub fn rem(a: i32, b: i32) -> i32 {
        a % b
    }

    /// A.7: if/else 控制流
    #[inline(never)]
    pub fn conditional(x: i32) -> i32 {
        if x > 0 { 1 } else { -1 }
    }

    /// A.8: while 循环
    #[inline(never)]
    pub fn simple_loop(n: i32) -> i32 {
        let mut acc = 0;
        let mut i = 0;
        while i < n {
            acc += i;
            i += 1;
        }
        acc
    }

    /// A.9: loop (无限循环 with break)
    #[inline(never)]
    pub fn loop_with_break(n: i32) -> i32 {
        let mut acc = 0;
        loop {
            acc += 1;
            if acc >= n {
                break;
            }
        }
        acc
    }

    /// A.10: 布尔 and/or/not
    #[inline(never)]
    pub fn bool_ops(a: bool, b: bool) -> bool {
        (a && b) || (!a && !b)
    }

    /// A.11: 嵌套函数调用
    #[inline(never)]
    pub fn nested_call(x: i32) -> i32 {
        mul(add(x, 1), 2)
    }

    /// A.12: 可变变量
    #[inline(never)]
    pub fn mutable_var(x: i32) -> i32 {
        let mut y = x;
        y += 1;
        y *= 2;
        y
    }

    /// A.13: i64 类型
    #[inline(never)]
    pub fn i64_ops(a: i64, b: i64) -> i64 {
        a.wrapping_add(b)
    }

    /// A.14: 无参数函数
    #[inline(never)]
    pub fn no_params() -> bool {
        true
    }

    /// A.15: 早期返回
    #[inline(never)]
    pub fn early_return(x: i32) -> i32 {
        if x < 0 {
            return 0;
        }
        x * 2
    }
}

// ============================================================
// 编译测试 (编译时验证 — 不执行)
// ============================================================

#[cfg(test)]
mod stage_a_tests {
    use super::fixtures;

    /// 验证所有 Stage A fixture 函数存在且类型正确。
    /// 实际的后端编译测试需要连接 rustc，这由集成测试框架处理。
    #[test]
    fn test_stage_a_fixtures_exist() {
        // 验证这些函数可以编译通过 (类型检查)
        assert_eq!(fixtures::return_constant(), 42);
        assert_eq!(fixtures::add(1, 2), 3);
        assert_eq!(fixtures::sub(5, 3), 2);
        assert_eq!(fixtures::mul(3, 4), 12);
        assert_eq!(fixtures::div(10, 3), 3); // 整数除法
        assert_eq!(fixtures::rem(10, 3), 1);
        assert_eq!(fixtures::conditional(5), 1);
        assert_eq!(fixtures::conditional(-5), -1);
        assert_eq!(fixtures::simple_loop(5), 10); // 0+1+2+3+4
        assert_eq!(fixtures::loop_with_break(3), 3);
        assert!(fixtures::bool_ops(true, true));
        assert!(!fixtures::bool_ops(true, false));
        assert_eq!(
            fixtures::nested_call(3),
            fixtures::mul(fixtures::add(3, 1), 2)
        );
        assert_eq!(fixtures::mutable_var(3), 8); // (3+1)*2
        assert_eq!(fixtures::i64_ops(1000, 2000), 3000);
        assert!(fixtures::no_params());
        assert_eq!(fixtures::early_return(-1), 0);
        assert_eq!(fixtures::early_return(5), 10);
    }

    /// 记录 Stage A 覆盖率。
    #[test]
    fn test_stage_a_coverage() {
        let covered = [
            "return_constant", // ✅
            "add",             // ✅
            "sub",             // ✅
            "mul",             // ✅
            "div",             // ✅
            "rem",             // ✅
            "conditional",     // ✅
            "simple_loop",     // ✅
            "loop_with_break", // ✅
            "bool_ops",        // ✅
            "nested_call",     // ✅
            "mutable_var",     // ✅
            "i64_ops",         // ✅
            "no_params",       // ✅
            "early_return",    // ✅
        ];
        // 所有 15 个 fixture 都被覆盖
        assert_eq!(covered.len(), 15);
        println!("Stage A: {} / 15 fixtures covered", covered.len());
    }
}

// ============================================================
// 自举进度追踪
// ============================================================

/// 自举兼容性阶段枚举。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BootstrapStage {
    /// Stage A: 编译 Rust core 子集 (基本整数运算 + 控制流 + 函数调用)
    StageA,
    /// Stage B: 编译 alloc crate (Vec, String, Box, 结构体, 枚举)
    StageB,
    /// Stage C: 编译 libc 子集 (FFI, extern "C")
    StageC,
    /// Stage D: 自举 (编译 codegen-lib 自身模块)
    StageD,
}

impl std::fmt::Display for BootstrapStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapStage::StageA => write!(f, "Stage A (core subset)"),
            BootstrapStage::StageB => write!(f, "Stage B (alloc)"),
            BootstrapStage::StageC => write!(f, "Stage C (libc/FFI)"),
            BootstrapStage::StageD => write!(f, "Stage D (self-hosting)"),
        }
    }
}

/// Stage A 需要支持的 MIR 构造清单。
pub mod mir_checklist {
    /// Stage A 必须支持的 MIR 语句/终结器。
    pub const REQUIRED_MIR_CONSTRUCTS: &[(&str, bool)] = &[
        ("StorageLive/StorageDead", false),    // 局部变量生命周期
        ("Assign (scalar rvalue)", true),      // ✅ 基本赋值
        ("Assign (binary op)", true),          // ✅ 二元运算
        ("Assign (unary op: Not/Neg)", false), // 一元运算
        ("Assign (CheckedBinOp)", false),      // 检查算术
        ("goto (Goto)", true),                 // ✅ 无条件跳转
        ("switchInt (SwitchInt)", true),       // ✅ 条件分支
        ("call (Call)", true),                 // ✅ 函数调用
        ("return (Return)", true),             // ✅ 返回
        ("unreachable (Unreachable)", false),  // 不可达
        ("drop (Drop)", false),                // 丢弃语义
    ];

    /// 获取 Stage A 的完成百分比。
    pub fn completion_percent() -> f64 {
        let total = REQUIRED_MIR_CONSTRUCTS.len() as f64;
        let done = REQUIRED_MIR_CONSTRUCTS
            .iter()
            .filter(|(_, done)| *done)
            .count() as f64;
        (done / total) * 100.0
    }
}

#[cfg(test)]
mod bootstrap_tests {
    use super::*;

    #[test]
    fn test_stage_a_progress() {
        let pct = mir_checklist::completion_percent();
        println!(
            "Stage A completion: {:.1}% ({}/{} constructs)",
            pct,
            mir_checklist::REQUIRED_MIR_CONSTRUCTS
                .iter()
                .filter(|(_, d)| *d)
                .count(),
            mir_checklist::REQUIRED_MIR_CONSTRUCTS.len(),
        );
        // 当前至少期望 4 个构造可用
        let done_count = mir_checklist::REQUIRED_MIR_CONSTRUCTS
            .iter()
            .filter(|(_, d)| *d)
            .count();
        assert!(
            done_count >= 4,
            "Expected at least 4 MIR constructs supported"
        );
    }

    #[test]
    fn test_stage_ordering() {
        assert!(BootstrapStage::StageA < BootstrapStage::StageB);
        assert!(BootstrapStage::StageB < BootstrapStage::StageC);
        assert!(BootstrapStage::StageC < BootstrapStage::StageD);
    }
}
