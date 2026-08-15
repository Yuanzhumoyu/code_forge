//! forge-dsl — ISA-DSL: TOML-driven ISA code generator.
//!
//! Proc macros: `isa!` and `isa_from_file!`.
//!
//! - v11（现行）：`parser`/`model`/`bitstring` —— 编码字符串 + `@原语` 语法，
//!   由 `isa_from_file!` 消费。
//! - v12（迭代中，`v12` 模块）：唯一 DSL 语法（严格 TOML，不兼容 v11），
//!   迭代 1 已含模型 + 解析 + 语义校验；代码生成接入随迭代 2+ 进行。

use proc_macro::TokenStream;

mod bitstring;
mod codegen;
mod model;
mod parser;

// v12 唯一语法：严格 TOML 模型 + 解析 + 校验（迭代 1；生成接入随迭代 2+）。
mod v12;

// 每 ISA 专属汇编语法生成器（模板 → lalrpop 语法 → parser 代码）
mod asm_grammar;

// Instruction resolver (shared with CST codegen)
mod asm_resolver;

// Grammar rule expression parser (used by build_grammar_from_model)
mod grammar_rule;

// CST-based code generation using forge-grammar
mod cst_codegen;

fn compile_source(source: &str) -> Result<proc_macro2::TokenStream, DslError> {
    let mut model = parser::parse(source).map_err(DslError::Parse)?;
    model.validate().map_err(DslError::Validation)?;
    model.expand_opcodes();
    model.expand_variants();
    model.expand_templates();
    model.validate_lowering().map_err(DslError::Validation)?;

    let inner = codegen::generate(&model).map_err(DslError::Codegen)?;

    let mod_name = syn::Ident::new(
        &model.meta.name.to_lowercase().replace('-', "_"),
        proc_macro2::Span::call_site(),
    );

    Ok(quote::quote! {
        // 生成代码：允许 DSL 产出的手写等价模式（? 重写 / range contains / 多余引用）
        #[allow(clippy::question_mark, clippy::manual_range_contains, clippy::needless_borrow)]
        pub mod #mod_name {
            use crate::prelude::*;
            #inner
        }
    })
}

#[derive(Debug, thiserror::Error)]
enum DslError {
    #[error("parse: {0}")]
    Parse(String),
    #[error("validate: {0}")]
    Validation(String),
    #[error("codegen: {0}")]
    Codegen(String),
}

/// `isa!{ ... }` — inline TOML ISA definition.
/// Input is raw TOML text (not a string literal).
#[proc_macro]
pub fn isa(input: TokenStream) -> TokenStream {
    compile_source(&input.to_string())
        .map(Into::into)
        .unwrap_or_else(|e| {
            syn::Error::new(proc_macro2::Span::call_site(), e.to_string())
                .to_compile_error()
                .into()
        })
}

/// `isa_from_file!("path/to/arch.toml")` — load ISA from file.
/// Input is a string literal (parsed with syn::LitStr).
#[proc_macro]
pub fn isa_from_file(input: TokenStream) -> TokenStream {
    let lit: syn::LitStr = match syn::parse(input) {
        Ok(lit) => lit,
        Err(e) => return e.to_compile_error().into(),
    };
    let path = lit.value();
    let content = match read_isa_file(&path) {
        Ok(c) => c,
        Err(e) => {
            return syn::Error::new(proc_macro2::Span::call_site(), e)
                .to_compile_error()
                .into();
        }
    };
    compile_source(&content)
        .inspect(|ts| dump_generated(&path, ts))
        .map(Into::into)
        .unwrap_or_else(|e| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("ISA-DSL error in '{path}':\n{e}"),
            )
            .to_compile_error()
            .into()
        })
}

/// `isa_v12_from_file!("path/to/arch.toml")` — v12 唯一语法 ISA（迭代 2：
/// 定宽 32 位自包含 encode/decode/asm 模块）。生成模块名 = 文件 stem
/// （与 v11 用 meta.name 不同；避免双嵌套与命名冲突，v12 文档化约定）。
#[proc_macro]
pub fn isa_v12_from_file(input: TokenStream) -> TokenStream {
    let lit: syn::LitStr = match syn::parse(input) {
        Ok(lit) => lit,
        Err(e) => return e.to_compile_error().into(),
    };
    let path = lit.value();
    let content = match read_isa_file(&path) {
        Ok(c) => c,
        Err(e) => {
            return syn::Error::new(proc_macro2::Span::call_site(), e)
                .to_compile_error()
                .into();
        }
    };
    let mod_name = syn::Ident::new(
        &std::path::Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "isa".into())
            .to_lowercase()
            .replace('-', "_"),
        proc_macro2::Span::call_site(),
    );
    compile_source_v12(&content, &mod_name)
        .inspect(|ts| dump_generated(&path, ts))
        .map(Into::into)
        .unwrap_or_else(|e| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("ISA-DSL v12 error in '{path}':\n{e}"),
            )
            .to_compile_error()
            .into()
        })
}

/// v12 编译入口：严格解析 + 校验 → v12 生成器 → `pub mod <name>`（name = 文件 stem）。
fn compile_source_v12(
    source: &str,
    mod_name: &syn::Ident,
) -> Result<proc_macro2::TokenStream, String> {
    let model = v12::parse_and_validate(source).map_err(|e| e.to_string())?;
    let inner = v12::codegen::generate(&model)?;
    Ok(quote::quote! { pub mod #mod_name { #inner } })
}

/// 读取 ISA 文件：相对当前目录 → CARGO_MANIFEST_DIR → workspace 根（上 3 级）。
fn read_isa_file(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path)
        .or_else(|_| {
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
            std::fs::read_to_string(std::path::PathBuf::from(&manifest_dir).join(path))
        })
        .or_else(|_| {
            // 向上查找：workspace 布局下 crate 位于 crates/<layer>/<crate>/
            //（如 crates/backend/forge-codegen → 上 3 级到 <root>/isa/...）。
            let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
            std::fs::read_to_string(
                std::path::PathBuf::from(&manifest_dir)
                    .join("../../..")
                    .join(path),
            )
        })
        .map_err(|e| format!("Cannot read ISA file '{path}': {e}"))
}

/// 调试：FGE_DEBUG_GEN 时按文件 stem 导出生成代码。
fn dump_generated(path: &str, ts: &proc_macro2::TokenStream) {
    if std::env::var("FGE_DEBUG_GEN").is_ok() {
        let base = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "isa".into());
        let _ = std::fs::write(
            std::env::temp_dir().join(format!("forge_gen_{base}.rs")),
            ts.to_string(),
        );
    }
}
