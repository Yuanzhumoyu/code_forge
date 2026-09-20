//! forge-isa-dsl — ISA-DSL 编译器（普通 lib）：模型 / 解析 / 校验 / 诊断 / 代码生成。
//!
//! **为什么拆出来（v18 S7a）**：proc-macro crate 不能导出非宏项，而 ISA-DSL 的能力
//! （校验、`explain`、JSON Schema、`insts`/`diff`、未来的 `forge-isa` CLI）都要在
//! **宏之外**可用。于是：
//!
//! - 本 crate（普通 lib）= 唯一事实源：严格 TOML 模型 + 语义校验 + 可点击诊断 +
//!   代码生成（`proc_macro2::TokenStream`，不依赖 `proc_macro`）；
//! - `forge-dsl`（proc-macro）= 薄包装：解析 `isa_from_file!` 参数 → 调本 crate →
//!   把 TokenStream 交回编译器。
//!
//! v11 语法层（`encoding` 字符串 + `@原语`、紧凑 `fields` 串、`when` 谓词串）已整体
//! 移除——无兼容层、无转换工具、无逃生门。模型/校验/生成见 [`v12`] 模块。

mod assembler;
mod v12;

pub use v12::V12Error;

/// `isa_from_file!` 的展开选项（宏参数 → 本 crate 的入口参数）。
#[derive(Debug, Clone, Default)]
pub struct ExpandOptions {
    /// 宿主 crate 路径（`krate = <path>`）：生成物里的 `crate::…` 改写到它，
    /// `forge_ir::…` 改写为 `<krate>::ir::…`。缺省（`None`）= "生成在哪个 crate 里
    /// 就属于哪个 crate"（`crate::…` 原样）。
    pub krate: Option<String>,
    /// 是否生成 `#[cfg(test)] mod __spec_tests`（v18 S6；缺省 true）。
    pub spec_tests: bool,
}

impl ExpandOptions {
    /// 宏缺省：`krate = None`、`spec_tests = true`。
    pub fn new() -> Self {
        Self {
            krate: None,
            spec_tests: true,
        }
    }
}

/// 展开一个 ISA 谱文件：读 → 解析/校验 → 生成 → 路径改写 → `pub mod <stem>`。
///
/// `path` 可以是相对路径（相对当前目录 → `CARGO_MANIFEST_DIR` → workspace 根，
/// 见 [`read_isa_file`]）。错误已带 `路径:行:列: 错误码` 前缀（可点击）。
pub fn expand_file(path: &str, opts: &ExpandOptions) -> Result<proc_macro2::TokenStream, String> {
    let (content, resolved) = read_isa_file(path)?;
    let mod_name = module_name(path);
    let krate: Option<proc_macro2::TokenStream> = opts.krate.as_ref().map(|p| path_tokens(p));
    let ts = expand_source(
        &content,
        &mod_name,
        &resolved,
        krate.as_ref(),
        opts.spec_tests,
    )?;
    dump_generated(path, &ts);
    Ok(ts)
}

/// 从**源码字符串**展开（CLI/测试用；不做路径改写——那是宏参数的职责）。
///
/// 返回 `(展开后的模块 token, 模块名)`。
pub fn expand_str(
    source: &str,
    mod_name: &str,
    isa_path: &std::path::Path,
) -> Result<proc_macro2::TokenStream, String> {
    let ident = syn::Ident::new(mod_name, proc_macro2::Span::call_site());
    expand_source(source, &ident, isa_path, None, true)
}

/// 解析 + 校验（不生成代码）：成功返回 `Ok(())`，失败返回**渲染好的诊断行**
/// （每行 `路径:行:列: 错误码: 消息`），供 CLI/工具直接打印。
pub fn validate_source(source: &str, isa_path: &std::path::Path) -> Result<(), Vec<String>> {
    match v12::parse_and_validate(source) {
        Ok(_) => Ok(()),
        Err(e) => Err(e
            .render(Some(isa_path))
            .lines()
            .map(str::to_string)
            .collect()),
    }
}

/// 校验一个谱文件（读文件的便捷包装；文件读不到按一条诊断返回）。
pub fn validate_file(path: &str) -> Result<(), Vec<String>> {
    let (content, resolved) = match read_isa_file(path) {
        Ok(v) => v,
        Err(e) => return Err(vec![e]),
    };
    validate_source(&content, &resolved)
}

/// 文件 stem → 生成模块名（小写、`-` → `_`）。
pub fn module_name(path: &str) -> syn::Ident {
    syn::Ident::new(
        &std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "isa".into())
            .to_lowercase()
            .replace('-', "_"),
        proc_macro2::Span::call_site(),
    )
}

/// 把 `krate = <路径>` 的文本解析成 token（`a::b` 这类路径）。
fn path_tokens(path: &str) -> proc_macro2::TokenStream {
    path.parse().unwrap_or_else(|_| {
        let ident = syn::Ident::new(path, proc_macro2::Span::call_site());
        quote::quote!(#ident)
    })
}

/// v12 编译入口：严格解析 + 校验 → 生成器 → `pub mod <name>`（name = 文件 stem）。
///
/// 生成模块首行嵌入 `include_bytes!(<TOML 绝对路径>)`：rustc 据此把 ISA 谱登记为
/// 本 crate 的编译依赖，改 TOML 自动触发重编译。**这是"改谱后必须手动 touch
/// arch/<isa>.rs"这条历史 workaround 的根治**——proc 宏自身无法声明依赖，
/// `include_bytes!` 是 stable 上唯一的表达方式（`proc_macro::tracked_path`
/// 至今 unstable）。常量匿名（`const _`）故不与生成模块任何名字冲突。
fn expand_source(
    source: &str,
    mod_name: &syn::Ident,
    isa_path: &std::path::Path,
    krate: Option<&proc_macro2::TokenStream>,
    spec_tests: bool,
) -> Result<proc_macro2::TokenStream, String> {
    // 诊断一次列全（S1）：`render(Some(path))` 每行都带可点击的 `路径:行:列: 码:`。
    let model = v12::parse_and_validate(source).map_err(|e| e.render(Some(isa_path)))?;
    let inner = v12::codegen::generate_with(&model, spec_tests)
        .map_err(|e| v12::anchor_msg(source, isa_path, &e))?;
    // 宿主 crate 路径：改写生成物里的路径根（见 `rewrite_path_roots`）。
    let inner = match krate {
        Some(k) => rewrite_path_roots(inner, k),
        None => inner,
    };
    let dep = isa_path.to_string_lossy().replace('\\', "/");
    Ok(quote::quote! {
        pub mod #mod_name {
            const _: &[u8] = include_bytes!(#dep);
            #inner
        }
    })
}

/// 把**生成物**里的路径根改写到宿主 crate：
/// - `crate::…` → `<krate>::…`；
/// - `forge_ir::…` → `<krate>::ir::…`（forge-codegen `pub use forge_ir as ir`）。
///
/// 只改写**路径位置**的标识符（后随 `::`）：`pub(crate)` 这类可见性标记、以及
/// 字符串字面量里的同名文本都不受影响。生成器自身源码里的 `crate::v12`（生成期
/// 代码）不经过本函数。
fn rewrite_path_roots(
    ts: proc_macro2::TokenStream,
    krate: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let mut out = proc_macro2::TokenStream::new();
    let mut it = ts.into_iter().peekable();
    while let Some(tt) = it.next() {
        match tt {
            proc_macro2::TokenTree::Group(g) => {
                let inner = rewrite_path_roots(g.stream(), krate);
                let mut ng = proc_macro2::Group::new(g.delimiter(), inner);
                ng.set_span(g.span());
                out.extend([proc_macro2::TokenTree::Group(ng)]);
            }
            proc_macro2::TokenTree::Ident(id) => {
                let is_path_root = matches!(
                    it.peek(),
                    Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == ':'
                );
                if is_path_root && id == "crate" {
                    out.extend(krate.clone());
                } else if is_path_root && id == "forge_ir" {
                    out.extend(krate.clone());
                    out.extend(quote::quote!(::ir));
                } else {
                    out.extend([proc_macro2::TokenTree::Ident(id)]);
                }
            }
            other => out.extend([other]),
        }
    }
    out
}

/// 读取 ISA 文件：相对当前目录 → CARGO_MANIFEST_DIR → workspace 根（上 3 级）。
/// 返回 (内容, **绝对**路径)——绝对路径供 `include_bytes!` 依赖跟踪与错误前缀用。
pub fn read_isa_file(path: &str) -> Result<(String, std::path::PathBuf), String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let candidates = [
        std::path::PathBuf::from(path),
        std::path::PathBuf::from(&manifest_dir).join(path),
        // workspace 布局下 crate 位于 crates/<layer>/<crate>/
        //（如 crates/backend/forge-codegen → 上 3 级到 <root>/isa/...）。
        std::path::PathBuf::from(&manifest_dir)
            .join("../../..")
            .join(path),
    ];
    let mut last = String::new();
    for cand in candidates {
        match std::fs::read_to_string(&cand) {
            Ok(s) => {
                let abs = std::fs::canonicalize(&cand)
                    .map(|p| {
                        // Windows 的 canonicalize 返回 \\?\ 前缀，include_bytes! 不接受
                        let s = p.to_string_lossy().to_string();
                        std::path::PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
                    })
                    .unwrap_or(cand);
                return Ok((s, abs));
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("Cannot read ISA file '{path}': {last}"))
}

/// 调试：FGE_DEBUG_GEN 时按文件 stem 导出生成代码。
pub fn dump_generated(path: &str, ts: &proc_macro2::TokenStream) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    /// `crate::…` 改写为宿主路径；`pub(crate)` 等可见性标记**不**被改写。
    #[test]
    fn rewrite_path_roots_rewrites_only_path_roots() {
        let src = quote! {
            impl crate::machine::abi::TargetABI for ABI {
                fn f(&self) -> crate::IrError { crate::IrError::Unsupported(x) }
            }
            pub(crate) const C: u8 = 1;
            pub(in crate) fn g() {}
            let s = "crate::not_a_path";
        };
        let out = rewrite_path_roots(src, &quote! { forge_codegen }).to_string();
        assert!(
            out.contains("forge_codegen :: machine :: abi :: TargetABI"),
            "{out}"
        );
        assert!(out.contains("forge_codegen :: IrError"), "{out}");
        assert!(
            out.contains("pub (crate) const C"),
            "可见性标记必须保留：{out}"
        );
        assert!(
            out.contains("pub (in crate) fn g"),
            "可见性标记必须保留：{out}"
        );
        assert!(
            out.contains("\"crate::not_a_path\""),
            "字符串字面量必须保留：{out}"
        );
        assert!(!out.contains("forge_codegen :: not_a_path"), "{out}");
    }

    /// `forge_ir::…` 改写为 `<krate>::ir::…`（宿主路径才改写，缺省路径不动）。
    #[test]
    fn rewrite_path_roots_rewrites_forge_ir() {
        let src = quote! { impl forge_ir::PhysReg for Reg {} };
        let out = rewrite_path_roots(src, &quote! { forge_codegen }).to_string();
        assert!(out.contains("forge_codegen :: ir :: PhysReg"), "{out}");
    }

    /// 模块名 = 文件 stem（小写、`-` → `_`）。
    #[test]
    fn module_name_is_file_stem() {
        assert_eq!(module_name("isa/x86_v12.toml").to_string(), "x86_v12");
        assert_eq!(module_name("a/My-ISA.toml").to_string(), "my_isa");
    }
}
