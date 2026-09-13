//! forge-dsl — ISA-DSL: TOML-driven ISA code generator.
//!
//! Proc macro: `isa_from_file!`（v12 唯一语法，严格 TOML，不兼容已删除的
//! v11 字符串编码层）。v11 语法层（`encoding` 字符串 + `@原语`、紧凑
//! `fields` 串、`when` 谓词串、asm 隐式魔法名）已整体移除——无兼容层、
//! 无转换工具、无逃生门。模型/解析/校验/生成见 `v12` 模块。

use proc_macro::TokenStream;
use proc_macro2::TokenTree;

// v12 唯一语法：严格 TOML 模型 + 解析 + 校验 + 代码生成。
// `assembler` = v12 asm 规范层（token 模型/参考词法/参考操作数解析），
// 生成器镜像其语义；v11 死代码（generic/enc 引擎）已删除。
mod assembler;
mod v12;

/// `isa_from_file!` 的参数：`"path/to/arch.toml"[, krate = <path>]`。
struct IsaArgs {
    path: syn::LitStr,
    /// 宿主 crate 路径：生成代码里的 `crate::…` 改写为该路径（缺省 = `crate`，
    /// 即"生成在哪个 crate 里、就属于哪个 crate"）。
    krate: Option<syn::Path>,
}

impl syn::parse::Parse for IsaArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let path: syn::LitStr = input.parse()?;
        let mut krate = None;
        while !input.is_empty() {
            input.parse::<syn::Token![,]>()?;
            if input.is_empty() {
                break; // 允许尾随逗号
            }
            let key: syn::Ident = input.parse()?;
            if key != "krate" {
                return Err(syn::Error::new(
                    key.span(),
                    format!("未知参数 `{key}`（`isa_from_file!` 仅支持 `krate = <path>`）"),
                ));
            }
            input.parse::<syn::Token![=]>()?;
            krate = Some(input.parse::<syn::Path>()?);
        }
        Ok(Self { path, krate })
    }
}

/// `isa_from_file!("path/to/arch.toml"[, krate = 宿主路径])` — v12 唯一语法 ISA：
/// 生成自包含 encode/decode/asm 模块 + TargetMachine 集成层。生成模块名 =
/// 文件 stem（非 meta.name；v12 文档化约定）。
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
    let (content, resolved) = match read_isa_file(&path) {
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
    // 仅当显式给出宿主路径时才改写（缺省路径保持生成物逐字节不变）。
    let krate: Option<proc_macro2::TokenStream> = args.krate.as_ref().map(|p| quote::quote!(#p));
    let a: TokenStream = compile_source_v12(&content, &mod_name, &resolved, krate.as_ref())
        .inspect(|ts| dump_generated(&path, ts))
        .map(Into::into)
        .unwrap_or_else(|e| {
            // `路径:行:列: 消息`——终端与 IDE 均可点击跳到 TOML 出错处。
            // proc 宏无法给出 TOML 内的 Span，故位置走消息前缀而非 rustc 诊断。
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("ISA-DSL error\n{}:{e}", resolved.display()),
            )
            .to_compile_error()
            .into()
        });
    a
}

/// v12 编译入口：严格解析 + 校验 → v12 生成器 → `pub mod <name>`（name = 文件 stem）。
///
/// 生成模块首行嵌入 `include_bytes!(<TOML 绝对路径>)`：rustc 据此把 ISA 谱登记为
/// 本 crate 的编译依赖，改 TOML 自动触发重编译。**这是"改谱后必须手动 touch
/// arch/<isa>.rs"这条历史 workaround 的根治**——proc 宏自身无法声明依赖，
/// `include_bytes!` 是 stable 上唯一的表达方式（`proc_macro::tracked_path`
/// 至今 unstable）。常量匿名（`const _`）故不与生成模块任何名字冲突。
fn compile_source_v12(
    source: &str,
    mod_name: &syn::Ident,
    isa_path: &std::path::Path,
    krate: Option<&proc_macro2::TokenStream>,
) -> Result<proc_macro2::TokenStream, String> {
    let model = v12::parse_and_validate(source).map_err(|e| e.to_string())?;
    let inner = v12::codegen::generate(&model).map_err(|e| v12::anchor_msg(source, &e))?;
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
            TokenTree::Group(g) => {
                let inner = rewrite_path_roots(g.stream(), krate);
                let mut ng = proc_macro2::Group::new(g.delimiter(), inner);
                ng.set_span(g.span());
                out.extend([TokenTree::Group(ng)]);
            }
            TokenTree::Ident(id) => {
                let is_path_root = matches!(
                    it.peek(),
                    Some(TokenTree::Punct(p)) if p.as_char() == ':'
                );
                if is_path_root && id == "crate" {
                    out.extend(krate.clone());
                } else if is_path_root && id == "forge_ir" {
                    out.extend(krate.clone());
                    out.extend(quote::quote!(::ir));
                } else {
                    out.extend([TokenTree::Ident(id)]);
                }
            }
            other => out.extend([other]),
        }
    }
    out
}

/// 读取 ISA 文件：相对当前目录 → CARGO_MANIFEST_DIR → workspace 根（上 3 级）。
/// 返回 (内容, **绝对**路径)——绝对路径供 `include_bytes!` 依赖跟踪与错误前缀用。
fn read_isa_file(path: &str) -> Result<(String, std::path::PathBuf), String> {
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

    /// 参数解析：缺省无 `krate`；`krate = path` 可解析；未知键报错。
    #[test]
    fn parse_args() {
        let a: IsaArgs = syn::parse2(quote! { "isa/x86_v12.toml" }).expect("缺省");
        assert!(a.krate.is_none());
        assert_eq!(a.path.value(), "isa/x86_v12.toml");
        let b: IsaArgs =
            syn::parse2(quote! { "tests/isa/demo_v12.toml", krate = forge_codegen }).expect("显式");
        assert!(b.krate.is_some());
        let err = match syn::parse2::<IsaArgs>(quote! { "x.toml", nope = a }) {
            Ok(_) => panic!("未知参数必须报错"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("未知参数"), "{err}");
    }
}
