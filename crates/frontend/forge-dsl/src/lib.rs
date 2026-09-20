//! forge-dsl — ISA-DSL 的 **proc-macro 薄层**（v18 S7a 拆 crate）。
//!
//! 真正的能力（TOML 模型 / 校验 / 诊断 / 代码生成）在 [`forge_isa_dsl`]（普通 lib）里；
//! 本 crate 只做两件事：
//!
//! 1. 解析 `isa_from_file!` 的参数（`krate = <path>`、`spec_tests = <bool>`）；
//! 2. 调 [`forge_isa_dsl::expand_file`]，把生成的 `TokenStream` 交回编译器。
//!
//! 拆分的理由：proc-macro crate **不能导出非宏项**，而 ISA-DSL 的校验/解释/Schema/CLI
//! 都需要在宏之外可用（v18 方案 §7 S7）。

use proc_macro::TokenStream;

/// `isa_from_file!` 的参数：`"path/to/arch.toml"[, krate = <path>][, spec_tests = <bool>]`。
struct IsaArgs {
    path: syn::LitStr,
    /// 宿主 crate 路径：生成代码里的 `crate::…` 改写为该路径（缺省 = `crate`，
    /// 即"生成在哪个 crate 里、就属于哪个 crate"）。
    krate: Option<syn::Path>,
    /// 是否生成 `#[cfg(test)] mod __spec_tests`（v18 S6，缺省 true）。
    /// 关掉它的理由只有一个：同一份谱被**多个测试二进制**反复展开（夹具谱住在
    /// `tests/common/mod.rs`），生成的自测会在每个二进制里重复跑——那里显式关掉，
    /// 由专门的用例二进制打开（见 `crates/backend/forge-codegen/tests/spec_tests_v12.rs`）。
    spec_tests: Option<bool>,
}

impl syn::parse::Parse for IsaArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let path: syn::LitStr = input.parse()?;
        let mut krate = None;
        let mut spec_tests = None;
        while !input.is_empty() {
            input.parse::<syn::Token![,]>()?;
            if input.is_empty() {
                break; // 允许尾随逗号
            }
            let key: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;
            match key.to_string().as_str() {
                "krate" => krate = Some(input.parse::<syn::Path>()?),
                "spec_tests" => spec_tests = Some(input.parse::<syn::LitBool>()?.value),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "未知参数 `{other}`（`isa_from_file!` 支持 `krate = <path>`\
                             与 `spec_tests = <bool>`）"
                        ),
                    ));
                }
            }
        }
        Ok(Self {
            path,
            krate,
            spec_tests,
        })
    }
}

/// `isa_from_file!("path/to/arch.toml"[, krate = 宿主路径][, spec_tests = <bool>])`
/// — v18 唯一语法 ISA：生成自包含 encode/decode/asm 模块 + TargetMachine 集成层。
/// 生成模块名 = 文件 stem（非 meta.name；文档化约定）。
///
/// `krate` 让生成代码**不依赖"被生成在 forge-codegen 内部"这一事实**：给出宿主
/// crate 路径后，生成物里的 `crate::…` 全部改写为 `<krate>::…`、`forge_ir::…`
/// 改写为 `<krate>::ir::…`（forge-codegen 提供 `pub use forge_ir as ir`）。
/// 用途：把 ISA 谱放在**测试**或其它 crate 里生成（demo 夹具不再进库本体）；
/// 缺省参数保持逐字节兼容（生成物与历史完全一致）。
#[proc_macro]
pub fn isa_from_file(input: TokenStream) -> TokenStream {
    let args: IsaArgs = match syn::parse(input) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    let path = args.path.value();
    let opts = forge_isa_dsl::ExpandOptions {
        krate: args.krate.as_ref().map(|p| quote::quote!(#p).to_string()),
        spec_tests: args.spec_tests.unwrap_or(true),
    };
    match forge_isa_dsl::expand_file(&path, &opts) {
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
    use super::*;

    /// 参数解析：缺省无 `krate`/`spec_tests`；显式给出可解析；未知键报错。
    #[test]
    fn parse_args() {
        let a: IsaArgs = syn::parse2(quote::quote! { "isa/x86_v12.toml" }).expect("缺省");
        assert!(a.krate.is_none());
        assert_eq!(a.spec_tests, None, "缺省 = 生成自测（true）");
        assert_eq!(a.path.value(), "isa/x86_v12.toml");
        let b: IsaArgs = syn::parse2(quote::quote! {
            "tests/isa/demo_v12.toml", krate = forge_codegen
        })
        .expect("显式");
        assert!(b.krate.is_some());
        let c: IsaArgs = syn::parse2(quote::quote! {
            "tests/isa/demo_v12.toml", krate = forge_codegen, spec_tests = false
        })
        .expect("两个参数");
        assert!(c.krate.is_some());
        assert_eq!(c.spec_tests, Some(false));
        let d: IsaArgs =
            syn::parse2(quote::quote! { "x.toml", spec_tests = true, }).expect("尾随逗号");
        assert_eq!(d.spec_tests, Some(true));
        let err = match syn::parse2::<IsaArgs>(quote::quote! { "x.toml", nope = a }) {
            Ok(_) => panic!("未知参数必须报错"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("未知参数"), "{err}");
    }
}
