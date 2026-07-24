//! ISA-DSL 全链路集成测试库。
//!
//! 验证 DSL → codegen → 后端 → 编译的完整流程是否正常。
//! 需要 nightly 工具链 + `features = ["nightly"]` 启用 rustc 后端测试。
//!
//! # 用法
//!
//! ```ignore
//! // tests/full_pipeline.rs
//! codegen_syntax_test::backend_tests!(my_isa::Inst, my_isa::Reg, my_isa::MyIsa, my_isa::ensure_registered);
//! ```

/// 为指定的后端生成完整的全链路测试模块。
///
/// 展开为 `#[cfg(test)] mod backend_tests { ... }` 包含：
/// - `test_full_pipeline`: 编译管线测试
/// - `test_registration_idempotent`: 注册幂等性测试
/// - `test_prologue_epilogue`: 前后文验证
///
/// # 参数
///
/// - `$inst`: 指令枚举类型（如 `my_isa::Inst`）
/// - `$reg`: 寄存器枚举类型（如 `my_isa::Reg`）
/// - `$isa`: ISA 实现类型（如 `my_isa::X86Isa`）
/// - `$ensure_registered`: 注册函数（如 `my_isa::ensure_registered`）
/// - `$name`: ISA 名称字符串（如 `"x86_64"`）
#[macro_export]
macro_rules! backend_tests {
    ($inst:ty, $reg:ty, $isa:ty, $ensure_registered:path, $name:expr) => {
        #[cfg(test)]
        mod backend_validation {
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
                let sig = Signature::new(&[], &[Type::I32]);
                let mut b = FunctionBuilder::new("test", sig);
                b.create_block_here();
                let v = b.iconst_i32(42);
                b.return_(&[v]);
                let func = b.finish();
                let compiled = FunctionCompiler::<$isa>::compile_raw(&func)
                    .expect("full pipeline compile should succeed");
                assert!(!compiled.code.is_empty());
                assert_eq!(compiled.code[0], 0x55, "prologue: push rbp");
                assert_eq!(
                    compiled.code[compiled.code.len() - 1],
                    0xC3,
                    "epilogue: ret"
                );
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

pub mod prelude {
    pub use codegen_lib::backend::{CodeSink, FunctionCompiler, RegMap, Registry};
    pub use codegen_lib::prelude::{BlockId, FunctionBuilder, Signature, Type, VReg};
}

/// 为 x86_64 后端运行全链路测试
pub fn run_x86_64_pipeline_tests() {
    use codegen_lib::backend::x86_64::ensure_registered;
    use codegen_lib::backend::{FunctionCompiler, Registry};
    use codegen_lib::prelude::{FunctionBuilder, Signature, Type};

    ensure_registered();
    assert!(
        Registry::global().contains("x86_64"),
        "x86_64 ISA should be registered"
    );

    // 测试完整编译管线：iconst + return
    let sig = Signature::new(&[], &[Type::I32]);
    let mut b = FunctionBuilder::new("test", sig);
    b.create_block_here();
    let v = b.iconst_i32(42);
    b.return_(&[v]);
    let func = b.finish();
    let compiled = FunctionCompiler::<codegen_lib::backend::x86_64::X86Isa>::compile_raw(&func)
        .expect("full pipeline compile should succeed");
    assert!(
        !compiled.code.is_empty(),
        "generated code should not be empty"
    );
    assert_eq!(compiled.code[0], 0x55, "prologue: push rbp");
    assert_eq!(
        compiled.code[compiled.code.len() - 1],
        0xC3,
        "epilogue: ret"
    );
}

// ============================================================
// nightly feature: rustc 后端集成测试
// ============================================================

#[cfg(feature = "nightly")]
pub mod nightly {
    //! rustc 后端集成测试 — 需要 nightly 工具链 + rustc_private。

    /// 验证 rustc 后端能通过 `auto_register_isa_for_target` 正确注册 x86_64 ISA。
    pub fn test_auto_register_isa_for_target() {
        rustc_codegen_codegenlib::auto_register_isa_for_target("x86_64-unknown-linux-gnu");
        rustc_codegen_codegenlib::auto_register_isa_for_target("amd64");
        use codegen_lib::backend::Registry;
        assert!(Registry::global().contains("x86_64"));
    }

    /// 验证后端结构体存在且可构建。
    pub fn test_codegen_backend_exists() {
        let _backend = rustc_codegen_codegenlib::CodegenLibBackend;
    }

    /// 全链路测试：ISA 注册 + 编译 + 验证。
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
        use codegen_lib::backend::Registry;
        use codegen_lib::backend::x86_64::ensure_registered;

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
