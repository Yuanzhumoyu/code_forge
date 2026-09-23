//! forge-dsl — ISA-DSL 的 **proc-macro 薄层**（v18 S7a 拆 crate）。
//!
//! 真正的能力（TOML 模型 / 校验 / 诊断 / 代码生成）在 [`forge_isa_dsl`]（普通 lib）里；
//! 本 crate 只做两件事：
//!
//! 1. 把 `isa_from_file!` 的实参交给 [`forge_isa_dsl::parse_macro_args`]（参数语义在
//!    DSL crate 里，build script 预生成走**同一份**解析——v18 S10d）；
//! 2. 调 [`forge_isa_dsl::expand_file_emitted`]，把"生成物落盘 + 一句 `include!`"的
//!    `TokenStream` 交回编译器。
//!
//! 拆分的理由：proc-macro crate **不能导出非宏项**，而 ISA-DSL 的校验/解释/Schema/CLI
//! 都需要在宏之外可用（v18 方案 §7 S7）。

use proc_macro::TokenStream;

/// `isa_from_file!("path/to/arch.toml"[, spec_tests = <bool>]
/// [, name = "…"][, parts = ["encode", …]])`
/// — v18 唯一语法 ISA：生成自包含 encode/decode/asm 模块 + TargetMachine 集成层。
/// 生成模块名 = 文件 stem（非 meta.name；文档化约定）。
///
/// **v19 V1b 起没有 `krate`**：生成物只依赖运行时 crate——`crate::…` 一律改写为
/// `forge_isa_runtime::…`、`forge_ir::…` 改写为 `forge_isa_runtime::ir::…`，因此
/// 把 ISA 谱放在**任何** crate（含 `tests/`）里生成都只要求该 crate 依赖
/// `forge-isa-runtime`（再加 build script 预生成，见 v18 S10d）。
/// `parts` 含 `tm` 时，宿主要用 `forge_isa_runtime::register_pipeline` 注册编译管线。
///
/// v18 S10d 起生成物**不再**是宏展开结果：它由宿主 build script 预生成成
/// `$OUT_DIR/forge_gen_<模块名>_<参数哈希>.rs`，本宏只发一句 `include!`
/// （见 [`forge_isa_dsl::expand_file_emitted`] 与 `docs/reference/isa-dsl.md`「宿主接入」）。
#[proc_macro]
pub fn isa_from_file(input: TokenStream) -> TokenStream {
    let args = match forge_isa_dsl::parse_macro_args(input.into()) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    match forge_isa_dsl::expand_file_emitted(&args) {
        Ok(ts) => ts.into(),
        Err(e) => {
            // `路径:行:列: 消息`——终端与 IDE 均可点击跳到 TOML 出错处。
            // proc 宏无法给出 TOML 内的 Span，故位置走消息前缀而非 rustc 诊断。
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("ISA-DSL error\n{e}"),
            )
            .to_compile_error()
            .into()
        }
    }
}

#[cfg(test)]
mod tests {
    /// 参数解析：缺省 `spec_tests = true`；显式给出可解析；未知键报错。
    ///
    /// v18 S10d 起参数语义住在 `forge_isa_dsl`（build script 预生成走同一份解析），
    /// 本用例钉的是**宏侧入口**仍然按这套语义工作。
    #[test]
    fn parse_args() {
        let a =
            forge_isa_dsl::parse_macro_args(quote::quote! { "isa/x86_v12.toml" }).expect("缺省");
        assert!(a.opts.spec_tests, "缺省 = 生成自测（true）");
        assert_eq!(a.path, "isa/x86_v12.toml");
        let b = forge_isa_dsl::parse_macro_args(quote::quote! {
            "tests/isa/demo_v12.toml"
        })
        .expect("显式");
        assert!(b.opts.spec_tests);
        let c = forge_isa_dsl::parse_macro_args(quote::quote! {
            "tests/isa/demo_v12.toml", spec_tests = false
        })
        .expect("两个参数");
        assert!(!c.opts.spec_tests);
        let d = forge_isa_dsl::parse_macro_args(quote::quote! { "x.toml", spec_tests = true, })
            .expect("尾随逗号");
        assert!(d.opts.spec_tests);
        let e = forge_isa_dsl::parse_macro_args(quote::quote! {
            "tests/isa/include_root_v12.toml",
            spec_tests = false, name = "my_isa",
            parts = ["encode", "decode"]
        })
        .expect("v18 S7d 新参数");
        assert_eq!(e.opts.name.as_deref(), Some("my_isa"));
        assert_eq!(e.opts.parts.names(), "encode, decode");
        let err = forge_isa_dsl::parse_macro_args(quote::quote! { "x.toml", parts = [1, 2] })
            .expect_err("parts 元素必须是字符串");
        assert!(err.to_string().contains("字符串字面量"), "{err}");
        let err = forge_isa_dsl::parse_macro_args(quote::quote! { "x.toml", nope = a })
            .expect_err("未知参数必须报错");
        assert!(err.to_string().contains("未知参数"), "{err}");
    }
}
