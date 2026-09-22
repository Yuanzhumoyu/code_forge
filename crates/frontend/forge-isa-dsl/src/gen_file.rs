//! 生成物**落盘**与**宿主预生成**（v18 S10d）。
//!
//! ## 为什么生成物要落文件
//!
//! 整份生成物（x86 全部件 1.0 MB token、arm64 0.5 MB）以前是 `isa_from_file!` 的
//! **宏展开结果**。rust-analyzer 的展开管线对它报一批假阳性
//! `expected expression` / `expected R_PAREN`（定宽/混合字长 ISA 上必现；`rustc`/
//! `clippy`/全门禁完全无关——证据见 `docs/guides/rust-analyzer-notes.md`）。
//! 换成"生成物写进 `$OUT_DIR` 的文件 + 展开只留 `include!`"后，RA 走的是**文件**这条
//! 常规路径：同一份 token 文本，RA 自己的解析器对它是零 `ERROR` 节点。
//!
//! ## 为什么由 build script 生成，而不是宏自己写
//!
//! 两条实测约束（2026-09-23）：
//!
//! 1. **RA 只在分析开始前登记一次可加载文件**。它自己会跑 build script（本仓
//!    `forge-ir` 的 `opcode_gen.rs` 就是这么被正常加载的），但看不见分析开始后才由
//!    proc 宏写出来的文件——实测那个文件明明存在、mtime 早于诊断，RA 仍报
//!    `macro-error: failed to load file …`，随后生成模块的项在 20+ 个文件里全变
//!    "unresolved import"，比原来的假阳性更糟。所以生成物必须**在 build script 阶段**
//!    就位：宿主在 build.rs 里调 [`pregenerate_host`]。
//! 2. **`TokenStream::to_string()` 是**上下文相关**的**：同样的 token 流，proc 宏里
//!    （rustc 的美化打印，带换行缩进）与普通二进制里（proc-macro2 的紧凑打印）产出的
//!    文本不同。若宏侧与 build script 侧都写文件，两侧会**互相覆盖**（x86 2.6 MB vs
//!    1.3 MB），每次构建都多一轮重编。因此**只允许一个写者**：文件一律由 build script
//!    写（内容是确定的紧凑打印），宏侧只发 `include!`；文件缺失就 fail-closed 报出
//!    可操作的错误（见 [`expand_file_emitted`]）。
//!
//! ## 文件名为什么是"参数哈希"而不是"内容哈希"
//!
//! `include!` 的路径必须**稳定**：RA（以及任何缓存了展开结果的东西）会记住上一次的
//! 路径。内容哈希会让"改了 TOML"变成"换了一个文件名"，旧路径永远不再生成 ⇒ 报
//! "文件不存在"。参数哈希（TOML 路径 + `krate`/`spec_tests`/`name`/`parts`）同时保证
//! 两件事：① 同一调用点的路径跨内容变更**恒定**；② 同一份谱的不同变体（`spec_tests`
//! 真假、部件不同、宿主不同）落到**不同**文件，互不覆盖。
//!
//! ## 与 cargo 的交互（为什么不重编死循环）
//!
//! build script 只在 cargo 判定它"脏"时才跑（`cargo:rerun-if-changed` 覆盖谱的**全部**
//! 来源文件、被扫描的源文件与目录、以及生成器自身）；它跑的时候无条件重写生成物，
//! 而这次构建本来就要重编 lib（TOML 是 lib 的 `include_bytes!` 依赖），因此
//! "文件比产物新"不会累积成每轮都脏。

use crate::{ExpandOptions, Parts};

/// `isa_from_file!` 的完整参数（路径 + 展开选项）——宏侧与 build script 侧共用同一份解析。
#[derive(Debug, Clone)]
pub struct MacroArgs {
    /// 谱路径**原样**（相对路径按 [`crate::read_isa_file`] 的候选顺序解析）。
    pub path: String,
    /// 展开选项。
    pub opts: ExpandOptions,
}

/// 解析 `isa_from_file!` 的参数：
/// `"path.toml"[, krate = <path>][, spec_tests = <bool>][, name = "…"][, parts = ["encode", …]]`。
///
/// 宏（`forge-dsl`）与 build script 预生成都走这里，**参数语义只有一处**。
/// 返回 `syn::Error` 是为了让宏侧能用 `to_compile_error()` 保住出错 token 的 span。
pub fn parse_macro_args(tokens: proc_macro2::TokenStream) -> syn::Result<MacroArgs> {
    RawArgs::parse_checked(syn::parse2::<RawArgs>(tokens)?)
}

/// 宏参数的语法形态。
struct RawArgs {
    path: syn::LitStr,
    krate: Option<syn::Path>,
    spec_tests: Option<bool>,
    name: Option<String>,
    parts: Option<Vec<String>>,
}

impl syn::parse::Parse for RawArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let path: syn::LitStr = input.parse()?;
        let mut krate = None;
        let mut spec_tests = None;
        let mut name = None;
        let mut parts = None;
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
                "name" => name = Some(input.parse::<syn::LitStr>()?.value()),
                "parts" => {
                    let arr = input.parse::<syn::ExprArray>()?;
                    let mut v = Vec::new();
                    for e in arr.elems {
                        match e {
                            syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Str(s),
                                ..
                            }) => v.push(s.value()),
                            other => {
                                return Err(syn::Error::new_spanned(
                                    other,
                                    "parts 的元素必须是字符串字面量（encode / decode / asm / tm）",
                                ));
                            }
                        }
                    }
                    parts = Some(v);
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "未知参数 `{other}`（`isa_from_file!` 支持 `krate = <path>`、`spec_tests = <bool>`、`name = \"…\"`、`parts = [\"encode\", …]`）"
                        ),
                    ));
                }
            }
        }
        Ok(Self {
            path,
            krate,
            spec_tests,
            name,
            parts,
        })
    }
}

impl RawArgs {
    fn parse_checked(self) -> syn::Result<MacroArgs> {
        let parts = match &self.parts {
            Some(names) => Parts::from_names(names)
                .map_err(|e| syn::Error::new(proc_macro2::Span::call_site(), e))?,
            None => Parts::all(),
        };
        Ok(MacroArgs {
            path: self.path.value(),
            opts: ExpandOptions {
                krate: self.krate.as_ref().map(|p| quote::quote!(#p).to_string()),
                spec_tests: self.spec_tests.unwrap_or(true),
                name: self.name,
                parts,
            },
        })
    }
}

/// 生成物落盘目录 = **`$OUT_DIR`**（cargo 为宿主包的所有目标设置，含 `tests/` 目标）。
///
/// 只有 build script 会调 [`pregenerate_host`]，因此这里必然有 `OUT_DIR`；缺它就是
/// 宿主没按要求接入（错误信息会点名）。
pub fn generated_dir() -> Result<std::path::PathBuf, String> {
    std::env::var("OUT_DIR")
        .map(std::path::PathBuf::from)
        .map_err(|_| {
            // 注意：**不要**用 Rust 字符串续行（行尾 `\`）拆长字符串——rust-analyzer 会把它
            // 判成 `Invalid escape`（见 docs/guides/rust-analyzer-notes.md §2）。写一行即可。
            "缺少 `OUT_DIR`：ISA 生成物必须由宿主的 build script 预生成（请在 build.rs 里调用 `forge_isa_dsl::pregenerate_host()`，见 docs/reference/isa-dsl.md「宿主接入」）".to_string()
        })
}

/// 生成物文件名：**只由参数决定**（TOML 路径 + 展开选项），与内容无关。
///
/// 见模块头"文件名为什么是参数哈希"。同名 ⇒ 同一调用点的同一变体，宏侧与 build
/// script 侧因此必然算出同一个名字。
pub fn generated_file_name(path: &str, opts: &ExpandOptions) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    opts.krate.hash(&mut h);
    opts.spec_tests.hash(&mut h);
    opts.name.hash(&mut h);
    [
        opts.parts.encode,
        opts.parts.decode,
        opts.parts.asm,
        opts.parts.tm,
    ]
    .hash(&mut h);
    format!(
        "forge_gen_{}_{:016x}.rs",
        crate::resolve_mod_name(path, opts),
        h.finish()
    )
}

/// 写出生成物（**无条件**原子替换：唯一写者是 build script，它只在 cargo 判定脏时才跑，
/// 因此不存在每轮重写的抖动问题——见模块头"与 cargo 的交互"）。
fn write_generated_file(
    dir: &std::path::Path,
    name: &str,
    path: &str,
    ts: &proc_macro2::TokenStream,
) -> Result<(), String> {
    let body = generated_body(path, ts);
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("无法创建生成物目录 {}：{e}", dir.display()))?;
    let target = dir.join(name);
    // 先写临时文件再 rename：cargo 并行编译的另一个目标不会读到写了一半的文件。
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, body.as_bytes())
        .map_err(|e| format!("无法写生成物 {}：{e}", tmp.display()))?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("无法就位生成物 {}：{e}", target.display())
    })
}

/// 落盘件的正文（头部注释 + lint 门闩 + 生成 token）。
fn generated_body(path: &str, ts: &proc_macro2::TokenStream) -> String {
    let header = format!(
        "// 由 forge-dsl 的 `isa_from_file!` 生成（v18 S10d）——请勿手工编辑。\n// 源谱：{path}；改谱后重新构建即会重新生成（TOML 由 include_bytes! 登记为编译依赖）。\n"
    );
    format!("{header}{}", lint_guard(ts))
}

/// **机器产物的 lint 门闩**：作为宏展开结果时 rustc/clippy 一律不 lint 生成物；
/// 改成 `include!` 后代码来自**文件**，这个豁免随之消失——实测 `cargo check
/// -p forge-codegen` 立刻多出 223 条风格类警告（`unused_parens` 191、
/// `non_snake_case` 6、`dead_code` 8…），会把 `-D warnings` 门禁打红。
/// 显式恢复"不 lint 生成物"这一语义：**不要**把它拆成"只列当前命中的 lint"——
/// 生成物随谱/部件变化会不断命中新 lint，拆开等于给门禁加一份永远追不上的维护负担。
fn lint_guard(ts: &proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    quote::quote! {
        #[allow(warnings, clippy::all)]
        #ts
    }
}

/// 展开结果本身：**只有一句** `include!(concat!(env!("OUT_DIR"), "/<文件名>"))`。
///
/// 这是 S10d 的全部目的——交给 rust-analyzer 的展开从 1.0 MB token 变成 2 个 token，
/// 生成物改由文件承载（同文本，RA 走常规文件解析路径，零语法错误）。
fn include_tokens(name: &str) -> proc_macro2::TokenStream {
    let suffix = syn::LitStr::new(&format!("/{name}"), proc_macro2::Span::call_site());
    quote::quote! {
        include!(concat!(env!("OUT_DIR"), #suffix));
    }
}

/// **`isa_from_file!` 的展开入口（v18 S10d）**：生成物已在 `$OUT_DIR`（由 build script
/// 预生成），这里只发一句 `include!`。
///
/// 文件不存在 ⇒ 明确报错（fail-closed），而不是悄悄让生成模块变空——空模块会让下游
/// 几十个文件报 "unresolved import"，比一条点名原因的错误难查得多。
pub fn expand_file_emitted(args: &MacroArgs) -> Result<proc_macro2::TokenStream, String> {
    let dir = generated_dir()?;
    let name = generated_file_name(&args.path, &args.opts);
    let file = dir.join(&name);
    if !file.is_file() {
        return Err(format!(
            "找不到 ISA 生成物 {}（谱 `{}`）。v18 S10d 起生成物由宿主 crate 的 build script 预生成——请在 build.rs 里调用 `forge_isa_dsl::pregenerate_host()`，并在 Cargo.toml 的 `[build-dependencies]` 里加 `forge-isa-dsl`（见 docs/reference/isa-dsl.md「宿主接入」）。",
            file.display(),
            args.path
        ));
    }
    Ok(include_tokens(&name))
}

/// **build script 入口**：为宿主 crate 里**每一处** `isa_from_file!` 预生成一份文件。
///
/// 必须在 build script 里调用（见模块头"为什么由 build script 生成"），并在同一个
/// `Cargo.toml` 里让 `[build-dependencies]` 指向本 crate。返回预生成的调用点数。
///
/// 扫描范围：`src/ tests/ benches/ examples/` 下的 `.rs`。同时打印
/// `cargo:rerun-if-changed=`（被扫描的目录与源文件、谱的**全部来源文件**），因此改谱、
/// 改 include 分片、增删调用点都会让 build script 重跑。
pub fn pregenerate_host() -> Result<usize, String> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| {
        "pregenerate_host 必须在 cargo build script 里调用（缺 CARGO_MANIFEST_DIR）".to_string()
    })?;
    let root = std::path::PathBuf::from(manifest);
    let dir = generated_dir()?;
    let mut count = 0usize;
    for sub in ["src", "tests", "benches", "examples"] {
        let sub_dir = root.join(sub);
        if !sub_dir.is_dir() {
            continue;
        }
        println!("cargo:rerun-if-changed={}", sub_dir.display());
        for file in rust_files(&sub_dir) {
            println!("cargo:rerun-if-changed={}", file.display());
            count += pregenerate_in_file(&file, &dir)?;
        }
    }
    Ok(count)
}

/// 递归收集目录下的 `.rs` 文件（顺序稳定：字典序）。
fn rust_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// 一个源文件里的所有 `isa_from_file!` 调用 → 各自预生成。
fn pregenerate_in_file(file: &std::path::Path, dir: &std::path::Path) -> Result<usize, String> {
    let Ok(src) = std::fs::read_to_string(file) else {
        return Ok(0);
    };
    if !src.contains("isa_from_file") {
        return Ok(0);
    }
    let Ok(ast) = syn::parse_file(&src) else {
        // 解析不了的源文件交给 rustc 报错，这里不越权。
        return Ok(0);
    };
    let mut calls = Vec::new();
    collect_calls(&ast, &mut calls);
    let mut n = 0;
    for tokens in calls {
        let args = match parse_macro_args(tokens) {
            Ok(a) => a,
            Err(e) => {
                // 参数写错时由 rustc 的宏展开报更准确的错，这里只提示。
                println!(
                    "cargo:warning={}: isa_from_file! 参数解析失败（由 rustc 报错）：{e}",
                    file.display()
                );
                continue;
            }
        };
        // 谱的全部来源文件（含 `include` 分片）都登记为构建依赖。
        match crate::spec_source_files(&args.path) {
            Ok(srcs) => {
                for s in srcs {
                    println!("cargo:rerun-if-changed={}", s.display());
                }
            }
            Err(e) => println!("cargo:warning={}: {e}", file.display()),
        }
        pregenerate(&args, dir)?;
        n += 1;
    }
    Ok(n)
}

/// 收集 AST 里所有 `…isa_from_file!(…)` 的实参 token。
fn collect_calls(ast: &syn::File, out: &mut Vec<proc_macro2::TokenStream>) {
    use syn::visit::Visit;
    struct V<'a>(&'a mut Vec<proc_macro2::TokenStream>);
    impl<'a, 'ast> Visit<'ast> for V<'a> {
        fn visit_macro(&mut self, m: &'ast syn::Macro) {
            if m.path
                .segments
                .last()
                .is_some_and(|s| s.ident == "isa_from_file")
            {
                self.0.push(m.tokens.clone());
            }
            syn::visit::visit_macro(self, m);
        }
    }
    V(out).visit_file(ast);
}

/// 为单个调用点生成并落盘（只由 build script 调用）。返回落盘文件名。
pub fn pregenerate(args: &MacroArgs, dir: &std::path::Path) -> Result<String, String> {
    let ts = crate::expand_file(&args.path, &args.opts)?;
    let name = generated_file_name(&args.path, &args.opts);
    write_generated_file(dir, &name, &args.path, &ts)?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn opts() -> ExpandOptions {
        ExpandOptions::new()
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("forge_s10d_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// 宏参数解析：路径 + 四个选项；未知键/坏元素报错。
    #[test]
    fn macro_args_parse() {
        let a = parse_macro_args(quote! {
            "isa/x86_v12.toml", krate = forge_codegen, spec_tests = false,
            name = "demo", parts = ["encode", "asm"],
        })
        .expect("完整参数");
        assert_eq!(a.path, "isa/x86_v12.toml");
        assert_eq!(a.opts.krate.as_deref(), Some("forge_codegen"));
        assert!(!a.opts.spec_tests);
        assert_eq!(a.opts.name.as_deref(), Some("demo"));
        assert_eq!(a.opts.parts.names(), "encode, asm");

        let e = parse_macro_args(quote! { "x.toml", nope = 1 })
            .unwrap_err()
            .to_string();
        assert!(e.contains("未知参数"), "{e}");
        let e = parse_macro_args(quote! { "x.toml", parts = [1] })
            .unwrap_err()
            .to_string();
        assert!(e.contains("字符串字面量"), "{e}");
        let e = parse_macro_args(quote! { "x.toml", parts = [] })
            .unwrap_err()
            .to_string();
        assert!(e.contains("空数组"), "{e}");
    }

    /// S10d：展开结果**只有一句** `include!(concat!(env!("OUT_DIR"), "…"))`——这是"生成物不再
    /// 作为宏展开结果交给 RA"的全部机制。
    #[test]
    fn emitted_expansion_is_a_single_include() {
        let flat =
            |ts: &proc_macro2::TokenStream| ts.to_string().split_whitespace().collect::<String>();
        assert_eq!(
            flat(&include_tokens("forge_gen_x_1.rs")),
            "include!(concat!(env!(\"OUT_DIR\"),\"/forge_gen_x_1.rs\"));"
        );
    }

    /// S10d 关键不变量：文件名**只由参数决定**——同参数同名字（宏侧与 build script 侧
    /// 必须算出同一个名字），选项不同（`krate`/`spec_tests`/`name`/`parts`）必须分开落盘，
    /// 否则同一份谱的不同变体互相覆盖。
    #[test]
    fn file_name_is_a_function_of_args_only() {
        let p = "isa/demo.toml";
        let base = generated_file_name(p, &opts());
        assert_eq!(base, generated_file_name(p, &opts()), "同参数必须同名");
        assert!(
            base.starts_with("forge_gen_demo_") && base.ends_with(".rs"),
            "{base}"
        );

        let mut o = opts();
        o.spec_tests = false;
        assert_ne!(base, generated_file_name(p, &o), "spec_tests 不同必须分开");
        let mut o = opts();
        o.krate = Some("forge_codegen".into());
        assert_ne!(base, generated_file_name(p, &o), "krate 不同必须分开");
        let mut o = opts();
        o.name = Some("other".into());
        assert_ne!(base, generated_file_name(p, &o), "name 不同必须分开");
        let mut o = opts();
        o.parts.encode = false;
        assert_ne!(base, generated_file_name(p, &o), "parts 不同必须分开");
        assert_ne!(
            base,
            generated_file_name("isa/other.toml", &opts()),
            "路径不同必须分开"
        );
    }

    /// S10d：落盘件是**合法 Rust 文件**，且 token 层面逐字节等于生成物
    /// （头部注释与 lint 门闩是文件侧的东西，不参与 token 文本的等价性）。
    #[test]
    fn generated_file_is_a_faithful_rust_file() {
        let dir = tmp_dir("unit");
        let ts = quote! {
            pub mod demo {
                /// 文档注释也要能往返。
                pub fn f(x: u32) -> u32 {
                    if (x > 0) { x } else { 0 }
                }
            }
        };
        let name = generated_file_name("isa/demo.toml", &opts());
        write_generated_file(&dir, &name, "isa/demo.toml", &ts).expect("落盘");

        let text = std::fs::read_to_string(dir.join(&name)).expect("读回");
        assert!(text.starts_with("// 由 forge-dsl"), "{text}");
        assert!(text.contains("pub mod demo"), "{text}");
        let parsed = syn::parse_file(&text).expect("落盘件必须是合法文件");
        assert_eq!(
            quote::ToTokens::to_token_stream(&parsed).to_string(),
            lint_guard(&ts).to_string(),
            "syn 往返必须逐字节稳定"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// S10d：**未接入 build script / 生成物缺失**时宏必须 fail-closed 并给出可操作的接入两步，
    /// 而不是悄悄让生成模块变空（空模块会让下游几十个文件报 unresolved import，更难查）。
    ///
    /// 两条失败路径都算通过：本 crate 的测试进程没有 `OUT_DIR`（缺环境变量），宿主编译里则是
    /// 文件不存在——两条消息都必须点名"build script + `pregenerate_host` + 参考文档"。
    #[test]
    fn missing_generated_file_is_fail_closed_with_hint() {
        let args = MacroArgs {
            path: "isa/x86_v12.toml".into(),
            opts: opts(),
        };
        let e = match expand_file_emitted(&args) {
            Ok(_) => panic!("生成物不存在时不该成功"),
            Err(e) => e,
        };
        assert!(e.contains("build script"), "{e}");
        assert!(e.contains("pregenerate_host"), "{e}");
        assert!(e.contains("docs/reference/isa-dsl.md"), "{e}");
    }

    /// S10d：宿主扫描真的能找到 `isa_from_file!` 调用（守卫 build script 预生成的前提）。
    #[test]
    fn host_scan_finds_calls() {
        let ast = syn::parse_file(
            r#"
            forge_dsl::isa_from_file!("a.toml", spec_tests = false);
            mod m { use forge_dsl::isa_from_file as x; }
            fn f() { forge_dsl::isa_from_file!("b.toml", parts = ["asm"]); }
            "#,
        )
        .expect("测试源码");
        let mut calls = Vec::new();
        collect_calls(&ast, &mut calls);
        assert_eq!(calls.len(), 2, "应找到 2 处调用（`use` 不算）");
        let a = parse_macro_args(calls[0].clone()).expect("第一个");
        assert_eq!(a.path, "a.toml");
        let b = parse_macro_args(calls[1].clone()).expect("第二个");
        assert_eq!(b.path, "b.toml");
        assert_eq!(b.opts.parts.names(), "asm");
    }
}
