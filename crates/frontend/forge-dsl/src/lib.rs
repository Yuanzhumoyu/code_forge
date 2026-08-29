//! forge-dsl — ISA-DSL: TOML-driven ISA code generator.
//!
//! Proc macro: `isa_from_file!`（v12 唯一语法，严格 TOML，不兼容已删除的
//! v11 字符串编码层）。v11 语法层（`encoding` 字符串 + `@原语`、紧凑
//! `fields` 串、`when` 谓词串、asm 隐式魔法名）已整体移除——无兼容层、
//! 无转换工具、无逃生门。模型/解析/校验/生成见 `v12` 模块。

use proc_macro::TokenStream;

// v12 唯一语法：严格 TOML 模型 + 解析 + 校验 + 代码生成。
// `assembler` = v12 asm 规范层（token 模型/参考词法/参考操作数解析），
// 生成器镜像其语义；v11 死代码（generic/enc 引擎）已删除。
mod assembler;
mod v12;

/// `isa_from_file!("path/to/arch.toml")` — v12 唯一语法 ISA：生成自包含
/// encode/decode/asm 模块 + TargetMachine 集成层。生成模块名 = 文件 stem
/// （非 meta.name；v12 文档化约定）。
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
    let mod_name = syn::Ident::new(
        &std::path::Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "isa".into())
            .to_lowercase()
            .replace('-', "_"),
        proc_macro2::Span::call_site(),
    );
    let a: TokenStream = compile_source_v12(&content, &mod_name)
        .inspect(|ts| dump_generated(&path, ts))
        .map(Into::into)
        .unwrap_or_else(|e| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("ISA-DSL error in '{path}':\n{e}"),
            )
            .to_compile_error()
            .into()
        });
    a
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
