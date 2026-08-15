//! ISA-DSL 全链路集成测试库。
//!
//! 验证 DSL → codegen → 后端 → 编译的完整流程是否正常。
//! 需要 nightly 工具链 + `features = ["nightly"]` 启用 rustc 后端测试。

// nightly feature 依赖 forge-rustc（rustc_private）：需启用该 feature 才能链接 rustc_driver。
#![cfg_attr(feature = "nightly", feature(rustc_private))]

/// 为指定的后端生成完整的全链路测试模块。
///
/// 展开为 `#[cfg(test)] mod backend_tests { ... }` 包含：
/// - `test_full_pipeline`: 编译管线测试
/// - `test_registration_idempotent`: 注册幂等性测试
///
/// # 参数
///
/// - `$tm`: TargetMachine 类型（如 `my_isa::TargetMachine`）
/// - `$ensure_registered`: 注册函数（如 `my_isa::ensure_registered`）
/// - `$name`: ISA 名称字符串（如 `"x86_64"`）
#[macro_export]
macro_rules! backend_tests {
    ($tm:ty, $ensure_registered:path, $name:expr) => {
        #[cfg(test)]
        mod backend_validation {
            #[allow(unused_imports)]
            use super::*;
            use $crate::prelude::*;

            #[test]
            fn test_full_pipeline() {
                $ensure_registered();
                assert!(
                    Registry::global().contains($name),
                    "{} should be registered",
                    $name
                );
                let sig = FunctionSignature::new(&[], &[TypeId::I32]);
                let mut fb = FunctionBuilder::new("test", TypeContext::new(), sig);
                fb.create_block_here();
                let func = {
                    let v = fb.iconst_i32(42);
                    fb.ret(&[v]);
                    fb.finish().expect("build")
                };
                let compiler = code_forge::backend::FunctionCompiler::new(<$tm>::new());
                let compiled = compiler
                    .compile_raw(&func)
                    .expect("full pipeline compile should succeed");
                assert!(!compiled.code.is_empty());
            }

            #[test]
            fn test_registration_idempotent() {
                $ensure_registered();
                assert!(Registry::global().contains($name));
                $ensure_registered();
                assert!(Registry::global().contains($name));
            }
        }
    };
}

/// ISA 测试模块 — 按 ISA/指令类型 feature 门控（覆盖矩阵、编码断言、执行测试）。
pub mod isa;

pub mod coverage;
/// 执行引擎模块（exec-unicorn：unicorn 跨架构模拟执行）。
pub mod exec;

/// compile-only 覆盖矩阵宏：对 ops 列表逐个构建最小函数并 compile_raw，
/// 断言全部编译成功（记录 Unsupported 缺口）。
///
/// # 参数
/// - `$isa`: ISA 名字符串（如 `"x86_64"`）
/// - `$tm`: TargetMachine 类型
/// - `$ensure_registered`: 注册函数
/// - `$build_min`: `fn(Opcode) -> Function`（op → 最小函数构建器）
/// - `$ops`: opcode 名字符串数组
#[macro_export]
macro_rules! coverage {
    ($isa:expr, $tm:ty, $ensure_registered:path, $build_min:path, $ops:expr) => {
        #[cfg(test)]
        mod coverage_tests {
            #[test]
            fn all_ops_compile() {
                $ensure_registered();
                let mut failed: Vec<(String, String)> = Vec::new();
                let mut ok_count = 0usize;
                for op_name in $ops {
                    let op = $crate::parse_opcode_name(op_name);
                    let func = $build_min(op);
                    let compiler = code_forge::backend::FunctionCompiler::new(<$tm>::new());
                    match compiler.compile_raw(&func) {
                        Ok(_) => ok_count += 1,
                        Err(e) => failed.push((op_name.to_string(), format!("{e:?}"))),
                    }
                }
                eprintln!("{}: {}/{} ops lowering ok", $isa, ok_count, $ops.len());
                for (op, err) in &failed {
                    eprintln!("{}  {op} => {err}", $isa);
                }
                assert!(
                    failed.is_empty(),
                    "{}: {} ops not lowering: {:?}",
                    $isa,
                    failed.len(),
                    failed
                );
            }
        }
    };
}

/// encode-golden 宏：对 `(指令, 期望字节)` 列表逐条断言编码精确匹配。
///
/// # 参数
/// - `$encoder`: 编码器表达式（如 `x86_64::Encoder`）
/// - `$cases`: `&[(Inst, &[u8])]` — (指令实例, 期望字节)
///
/// 寄存器预置：`VReg(0..64)` → Int 类 PReg，`VReg(64..128)` → Float 类 PReg
/// （浮点/SSE 指令编码用例使用 `VReg(64+n)` 表示浮点寄存器）。
#[macro_export]
macro_rules! encode_golden {
    ($encoder:expr, $cases:expr) => {
        #[cfg(test)]
        mod encode_golden_tests {
            #[allow(unused_imports)]
            use super::*;
            use code_forge::backend::machine::encoder::TargetEncoder;

            #[test]
            fn encode_matches_golden() {
                let encoder = $encoder;
                // 字段已物理化（Reg）：编码直接用字段 to_index()，无需 VReg 预置
                let rm = code_forge::AllocResult::new();
                for (inst, expected) in $cases {
                    let bytes = encoder
                        .encode_to_bytes(inst, &rm)
                        .unwrap_or_else(|e| panic!("encode failed: {e:?}"));
                    assert_eq!(&bytes[..], *expected, "encoding mismatch for {inst:?}");
                }
            }
        }
    };
}

/// encode-golden 负例宏：对指令列表逐条断言编码**失败**（期望报错）。
///
/// # 参数
/// - `$encoder`: 编码器表达式（如 `x86_64::Encoder`）
/// - `$cases`: `&[Inst]` — 期望编码失败的指令实例
#[macro_export]
macro_rules! encode_golden_err {
    ($encoder:expr, $cases:expr) => {
        #[cfg(test)]
        mod encode_golden_err_tests {
            #[allow(unused_imports)]
            use super::*;
            use code_forge::backend::machine::encoder::TargetEncoder;

            #[test]
            fn encode_fails_as_expected() {
                let encoder = $encoder;
                let mut rm = code_forge::AllocResult::new();
                for n in 0..64u32 {
                    rm.insert(
                        code_forge::prelude::VReg(n),
                        code_forge::ir::PReg::new((n % 32) as u32, code_forge::ir::RegClass::Int),
                    );
                    rm.insert(
                        code_forge::prelude::VReg(64 + n),
                        code_forge::ir::PReg::new((n % 32) as u32, code_forge::ir::RegClass::Float),
                    );
                }
                for inst in $cases {
                    assert!(
                        encoder.encode_to_bytes(inst, &rm).is_err(),
                        "expected encode failure for {inst:?}"
                    );
                }
            }
        }
    };
}

/// exec 宏：将最小函数编译后在本机（x86_64）执行并断言返回值。
///
/// # 参数
/// - `$name`: 测试名（字符串）
/// - `$build`: `fn(&mut FunctionBuilder) -> Value`（函数体构建器，返回 i32）
/// - `$expected`: 期望返回值
///
/// 同一模块内每个 `exec!` 需位于独立 `mod`（宏内部生成 `mod exec_tests`）。
#[macro_export]
macro_rules! exec {
    ($name:expr, $build:expr, $expected:expr) => {
        #[cfg(test)]
        mod exec_tests {
            #[allow(unused_imports)]
            use super::*;

            #[test]
            fn executes_and_returns_expected() {
                let r = $crate::exec::harness::run_i32($name, $build);
                assert_eq!(r, $expected, "{}: got {}, expected {}", $name, r, $expected);
            }
        }
    };
}

/// exec_f64 宏：本机执行无参返回 f64 的函数并断言返回值（近似比较）。
///
/// # 参数
/// - `$name`: 测试名（字符串）
/// - `$build`: `fn(&mut FunctionBuilder) -> Value`（函数体构建器，返回 f64）
/// - `$expected`: 期望返回值（f64）
#[macro_export]
macro_rules! exec_f64 {
    ($name:expr, $build:expr, $expected:expr) => {
        #[cfg(test)]
        mod exec_f64_tests {
            #[allow(unused_imports)]
            use super::*;

            #[test]
            fn executes_and_returns_expected() {
                let r = $crate::exec::harness::run_f64($name, $build);
                let exp = $expected as f64;
                assert!(
                    (r - exp).abs() < 1e-6,
                    "{}: got {}, expected {}",
                    $name,
                    r,
                    exp
                );
            }
        }
    };
}

/// exec_args 宏：本机执行带整型参数的函数（≤4 个）并断言 i64 返回值。
///
/// # 参数
/// - `$name`: 测试名（字符串）
/// - `$params`: `&[(TypeId, &str)]` 参数类型列表
/// - `$args`: `&[u64]` 参数值列表（长度须与 params 一致）
/// - `$build`: `fn(&mut FunctionBuilder, &[Value]) -> Value`（返回 i64）
/// - `$expected`: 期望返回值（i64）
#[macro_export]
macro_rules! exec_args {
    ($name:expr, $params:expr, $args:expr, $build:expr, $expected:expr) => {
        #[cfg(test)]
        mod exec_args_tests {
            #[allow(unused_imports)]
            use super::*;

            #[test]
            fn executes_with_args_returns_expected() {
                let r = $crate::exec::harness::run_args_i64($name, $params, $args, $build);
                assert_eq!(r, $expected, "{}: got {}, expected {}", $name, r, $expected);
            }
        }
    };
}

/// disasm 宏：对 `(指令, 期望反汇编文本)` 列表逐条断言反汇编精确匹配。
///
/// # 参数
/// - `$tm`: TargetMachine 类型（如 `x86_64::TargetMachine`）
/// - `$cases`: `&[(Inst, &str)]` — (指令实例, 期望文本)
#[macro_export]
macro_rules! disasm {
    ($tm:ty, $cases:expr) => {
        #[cfg(test)]
        mod disasm_tests {
            #[allow(unused_imports)]
            use super::*;
            use code_forge::backend::machine::target::TargetMachine as _;

            #[test]
            fn disasm_matches_golden() {
                let tm = <$tm>::new();
                let dis = tm.disassembler().expect("disassembler should be available");
                for (inst, expected) in $cases {
                    let s = dis.disassemble(inst);
                    assert_eq!(&s, expected, "disasm mismatch for {inst:?}");
                }
            }
        }
    };
}

/// 执行测试辅助函数（兼容转发层 → 统一 harness）。
pub mod exec_helpers {
    use code_forge::prelude::*;

    /// 编译无参返回 i32 的函数并在本机执行（转发到 `exec::harness::run_i32`）。
    pub fn run_test_i32(name: &str, build: fn(&mut FunctionBuilder) -> Value) -> i32 {
        crate::exec::harness::run_i32(name, build)
    }
}

/// opcode 名字 → Opcode 的解析（coverage 宏用）。
pub fn parse_opcode_name(name: &str) -> code_forge::ir::Opcode {
    crate::coverage::parse_opcode_name(name)
}

pub mod prelude {
    pub use code_forge::backend::{AllocResult, CodeSink, FunctionCompiler, Registry};
    pub use code_forge::prelude::{
        Block, FunctionBuilder, FunctionSignature, TypeId, TypeStore, VReg,
    };
    pub type Signature = FunctionSignature;
    pub use TypeId as Type;
}

/// 为 x86_64 后端运行全链路测试
pub fn run_x86_64_pipeline_tests() {
    use code_forge::backend::arch::x86_64::{self, ensure_registered};
    use code_forge::backend::{FunctionCompiler, Registry};
    use code_forge::prelude::{FunctionBuilder, FunctionSignature, TypeContext, TypeId};

    ensure_registered();
    assert!(
        Registry::global().contains("x86_64"),
        "x86_64 ISA should be registered"
    );

    // 测试完整编译管线：iconst + return
    let sig = FunctionSignature::new(&[], &[TypeId::I32]);
    let mut fb = FunctionBuilder::new("test", TypeContext::new(), sig);
    let (entry, _) = fb.create_entry_block();
    let func = {
        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        fb.finish().expect("build")
    };
    let compiler = FunctionCompiler::new(x86_64::TargetMachine::new());
    let compiled = compiler
        .compile_raw(&func)
        .expect("full pipeline compile should succeed");
    assert!(
        !compiled.code.is_empty(),
        "generated code should not be empty"
    );
}

// ============================================================
// nightly feature: rustc 后端集成测试
// ============================================================

#[cfg(feature = "nightly")]
pub mod nightly {
    pub fn test_auto_register_isa_for_target() {
        forge_rustc::auto_register_isa_for_target("x86_64-unknown-linux-gnu");
        forge_rustc::auto_register_isa_for_target("amd64");
        use code_forge::backend::Registry;
        assert!(Registry::global().contains("x86_64"));
        assert_eq!(forge_rustc::isa_name_for_target("amd64"), "x86_64");
        assert_eq!(
            forge_rustc::isa_name_for_target("x86_64-unknown-linux-gnu"),
            "x86_64"
        );
    }

    pub fn test_codegen_backend_exists() {
        // `codegen_backend_name` 是 forge-rustc 的公开入口（不泄漏 rustc_private 类型）。
        assert_eq!(forge_rustc::codegen_backend_name(), "code-forge");
    }

    pub fn test_full_pipeline() {
        super::run_x86_64_pipeline_tests();
    }
}

// ============================================================
// 测试模块
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pipeline() {
        run_x86_64_pipeline_tests();
    }

    #[test]
    fn isa_registration_idempotent() {
        use code_forge::backend::Registry;
        use code_forge::backend::arch::x86_64::ensure_registered;

        ensure_registered();
        assert!(Registry::global().contains("x86_64"));

        // 多次调用应不会 panic
        ensure_registered();
        assert!(Registry::global().contains("x86_64"));
    }

    #[cfg(feature = "nightly")]
    mod nightly_tests {
        use super::*;

        #[test]
        fn auto_register_isa() {
            nightly::test_auto_register_isa_for_target();
        }

        #[test]
        fn codegen_backend() {
            nightly::test_codegen_backend_exists();
        }

        #[test]
        fn nightly_full_pipeline() {
            nightly::test_full_pipeline();
        }
    }
}
