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
pub mod loader;
pub mod report;
pub mod schema;
mod v12;

pub use v12::V12Error;

/// 生成**部件**选择（`parts = [...]`，v18 S7d；方案 §5.8）。
///
/// `Inst` 枚举、寄存器表、内存支撑（`MemRef`/`__render_mem`）是任何部件的公共前提，
/// **恒定生成**；这里只控制四块可选件。缺省全开（= 历史行为，逐字节不变）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parts {
    /// `encode()`（含位域助手 `__place`/`__bits`）。
    pub encode: bool,
    /// `decode()` / `decode_partial()`。
    pub decode: bool,
    /// `disassemble()` / `assemble()`。
    pub asm: bool,
    /// TargetMachine 集成层（Encoder/Decoder/ABI/Lowering/FrameLowering/RegInfo/IsaInfo）。
    pub tm: bool,
}

impl Default for Parts {
    fn default() -> Self {
        Self::all()
    }
}

impl Parts {
    /// 全开（缺省）。
    pub fn all() -> Self {
        Self {
            encode: true,
            decode: true,
            asm: true,
            tm: true,
        }
    }

    /// 是否全开（决定能否打开生成期自测）。
    pub fn is_full(&self) -> bool {
        *self == Self::all()
    }

    /// 从 `parts = [...]` 的名字表构造；未知名字/空表报错（错误消息列出可用名）。
    pub fn from_names(names: &[String]) -> Result<Self, String> {
        if names.is_empty() {
            return Err("parts 不能为空数组（省略该参数 = 四块全开）".into());
        }
        let mut p = Self {
            encode: false,
            decode: false,
            asm: false,
            tm: false,
        };
        for n in names {
            match n.as_str() {
                "encode" => p.encode = true,
                "decode" => p.decode = true,
                "asm" => p.asm = true,
                "tm" => p.tm = true,
                other => {
                    return Err(format!(
                        "未知部件 `{other}`（可用：encode / decode / asm / tm）"
                    ));
                }
            }
        }
        Ok(p)
    }

    /// 当前部件名列表（错误消息 / 诊断用）。
    pub fn names(&self) -> String {
        let mut v = Vec::new();
        if self.encode {
            v.push("encode");
        }
        if self.decode {
            v.push("decode");
        }
        if self.asm {
            v.push("asm");
        }
        if self.tm {
            v.push("tm");
        }
        v.join(", ")
    }
}

/// `isa_from_file!` 的展开选项（宏参数 → 本 crate 的入口参数）。
#[derive(Debug, Clone, Default)]
pub struct ExpandOptions {
    /// 宿主 crate 路径（`krate = <path>`）：生成物里的 `crate::…` 改写到它，
    /// `forge_ir::…` 改写为 `<krate>::ir::…`。缺省（`None`）= "生成在哪个 crate 里
    /// 就属于哪个 crate"（`crate::…` 原样）。
    pub krate: Option<String>,
    /// 是否生成 `#[cfg(test)] mod __spec_tests`（v18 S6；缺省 true）。
    pub spec_tests: bool,
    /// 模块名覆盖（`name = "..."`）：生成 `pub mod <name>`（缺省 = 文件 stem）。
    pub name: Option<String>,
    /// 部件选择（`parts = [...]`）：缺省全开。
    pub parts: Parts,
}

impl ExpandOptions {
    /// 宏缺省：`krate = None`、`spec_tests = true`、`name = None`、`parts` 全开。
    pub fn new() -> Self {
        Self {
            krate: None,
            spec_tests: true,
            name: None,
            parts: Parts::all(),
        }
    }
}

/// 展开一个 ISA 谱文件：读 → 解析/校验 → 生成 → 路径改写 → `pub mod <stem>`。
///
/// `path` 可以是相对路径（相对当前目录 → `CARGO_MANIFEST_DIR` → workspace 根，
/// 见 [`read_isa_file`]）。错误已带 `路径:行:列: 错误码` 前缀（可点击）。
pub fn expand_file(path: &str, opts: &ExpandOptions) -> Result<proc_macro2::TokenStream, String> {
    let (_, resolved) = read_isa_file(path)?;
    let spec = loader::LoadedSpec::load(&resolved)?;
    // `name = "..."` 覆盖模块名（缺省 = 文件 stem）。
    let mod_name = match &opts.name {
        Some(n) => syn::Ident::new(n, proc_macro2::Span::call_site()),
        None => module_name(path),
    };
    let krate: Option<proc_macro2::TokenStream> = opts.krate.as_ref().map(|p| path_tokens(p));
    let ts = expand_loaded(
        &spec,
        &mod_name,
        krate.as_ref(),
        opts.spec_tests,
        opts.parts,
    )?;
    dump_generated(path, &ts);
    Ok(ts)
}

/// 渲染错误：每条诊断的合并行 → (来源文件, 文件内行)，输出可点击的
/// `路径:行:列: 码: 消息`（多文件谱因此指向**真正写那一行的文件**）。
pub fn render_error_for(spec: &loader::LoadedSpec, err: &V12Error) -> String {
    let mut out = String::new();
    for d in err.diags() {
        let (file, line) = spec.map_line(d.line);
        out.push_str(&format!(
            "{}:{}:{}: {}: {}\n",
            file.display(),
            line,
            d.col,
            d.code,
            d.msg
        ));
        for n in &d.notes {
            out.push_str(&format!("  = {n}\n"));
        }
    }
    out
}

/// 生成期裸消息 → 带 `路径:行:列` 前缀（同样按来源文件映射）。
fn anchor_msg_for(spec: &loader::LoadedSpec, msg: &str) -> String {
    let idx = v12::diag::DeclIndex::build(&spec.text);
    let a = idx.anchor(msg);
    let (file, line) = spec.map_line(a.line);
    format!("{}:{}:{}: {}: {msg}", file.display(), line, a.col, a.code)
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

/// 校验一个谱文件（**支持 `include`**：多文件谱的诊断按来源文件渲染）。
pub fn validate_file(path: &str) -> Result<(), Vec<String>> {
    let (_, resolved) = match read_isa_file(path) {
        Ok(v) => v,
        Err(e) => return Err(vec![e]),
    };
    let spec = match loader::LoadedSpec::load(&resolved) {
        Ok(s) => s,
        Err(e) => return Err(vec![e]),
    };
    match v12::parse_and_validate(&spec.text) {
        Ok(_) => Ok(()),
        Err(e) => Err(render_error_for(&spec, &e)
            .lines()
            .map(str::to_string)
            .collect()),
    }
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
    let spec = loader::LoadedSpec::from_text(source.to_string(), isa_path);
    expand_loaded(&spec, mod_name, krate, spec_tests, Parts::all())
}

/// 展开**已加载**的谱（`include` 已合并、诊断带来源文件）。
fn expand_loaded(
    spec: &loader::LoadedSpec,
    mod_name: &syn::Ident,
    krate: Option<&proc_macro2::TokenStream>,
    spec_tests: bool,
    parts: Parts,
) -> Result<proc_macro2::TokenStream, String> {
    // 生成期自测要用 encode/decode/asm 全部部件——部件受限时明确报错，
    // 而不是悄悄生成一份编译不过的自测。
    if spec_tests && !parts.is_full() {
        return Err(format!(
            "parts = [{}] 时不能生成生成期自测（`__spec_tests` 需要 encode/decode/asm 全部）\
             ——请显式写 `spec_tests = false`，或去掉 parts",
            parts.names()
        ));
    }
    // 诊断一次列全（S1）：每行都带可点击的 `路径:行:列: 码:`（按来源文件映射）。
    let model = v12::parse_and_validate(&spec.text).map_err(|e| render_error_for(spec, &e))?;
    let inner = v12::codegen::generate_with_parts(&model, spec_tests, parts)
        .map_err(|e| anchor_msg_for(spec, &e))?;
    // 短名折叠（S8c，纯等价）→ 宿主 crate 路径改写（见各自文档）。
    let inner = fold_short_forms(inner);
    let inner = match krate {
        Some(k) => rewrite_path_roots(inner, k),
        None => inner,
    };
    // 短名定义：折叠**之后**发射（自己写全路径），同样按宿主 crate 改写。
    let helpers = short_form_helpers();
    let helpers = match krate {
        Some(k) => rewrite_path_roots(helpers, k),
        None => helpers,
    };
    // `include_bytes!` 是 stable 上唯一能让 rustc 登记编译依赖的方式：
    // **每个来源文件**都要登记（多文件谱里改 include 也必须触发重编译）。
    let deps: Vec<syn::LitStr> = spec
        .sources
        .iter()
        .map(|p| {
            syn::LitStr::new(
                &p.to_string_lossy().replace('\\', "/"),
                proc_macro2::Span::call_site(),
            )
        })
        .collect();
    Ok(quote::quote! {
        pub mod #mod_name {
            #(const _: &[u8] = include_bytes!(#deps);)*
            #helpers
            #inner
        }
    })
}

/// 生成物内部**短名折叠**（v18 S8c）：纯等价改写，唯一目的是缩短生成代码（token 数）。
///
/// - `forge_ir :: RegClass` → `__RC`（模块内 `type __RC = forge_ir::RegClass;`）；
/// - `Reg :: from_index (…)` 与 `< Reg as forge_ir :: PhysReg > :: from_index (…)`
///   → `__ph (…)`（模块内 `#[inline] fn __ph(idx: u32, class: __RC) -> Reg`）；
/// - `< Reg as forge_ir :: PhysReg > :: to_index (…)` → `__ti (…)`（薄封装）。
///
/// 这几处是生成物里出现最多的长文本（x86 里 `Reg::from_index` 1,700 处、
/// `forge_ir::RegClass` 1,400 处），折叠后每处省 20–28 B 且**语义完全不变**。
/// 只认**路径/调用位置**（后随 `::`、调用组必须是小括号），因此不会碰字符串字面量；
/// 参数组**递归折叠**（否则 `from_index(…, forge_ir::RegClass::…)` 里的路径会漏）。
fn fold_short_forms(ts: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let toks: Vec<proc_macro2::TokenTree> = ts.into_iter().collect();
    let mut out = proc_macro2::TokenStream::new();
    let mut i = 0;
    while i < toks.len() {
        // `<Reg as forge_ir::PhysReg>::from_index(<组>)` / `:: to_index(<组>)`
        if is_punct(toks.get(i), '<')
            && ident_is(toks.get(i + 1), "Reg")
            && ident_is(toks.get(i + 2), "as")
            && ident_is(toks.get(i + 3), "forge_ir")
            && is_colon2(&toks, i + 4)
            && ident_is(toks.get(i + 6), "PhysReg")
            && is_punct(toks.get(i + 7), '>')
            && is_colon2(&toks, i + 8)
            && (ident_is(toks.get(i + 10), "from_index") || ident_is(toks.get(i + 10), "to_index"))
            && let Some(proc_macro2::TokenTree::Group(g)) = toks.get(i + 11)
            && g.delimiter() == proc_macro2::Delimiter::Parenthesis
        {
            out.extend(folded_call(&toks[i + 10], g));
            i += 12;
            continue;
        }
        // `Reg::from_index(<组>)`
        if ident_is(toks.get(i), "Reg")
            && is_colon2(&toks, i + 1)
            && ident_is(toks.get(i + 3), "from_index")
            && let Some(proc_macro2::TokenTree::Group(g)) = toks.get(i + 4)
            && g.delimiter() == proc_macro2::Delimiter::Parenthesis
        {
            out.extend(folded_call(&toks[i + 3], g));
            i += 5;
            continue;
        }
        // `forge_ir::RegClass`
        if ident_is(toks.get(i), "forge_ir")
            && is_colon2(&toks, i + 1)
            && ident_is(toks.get(i + 3), "RegClass")
        {
            out.extend([proc_macro2::TokenTree::Ident(proc_macro2::Ident::new(
                "__RC",
                proc_macro2::Span::call_site(),
            ))]);
            i += 4;
            continue;
        }
        match &toks[i] {
            proc_macro2::TokenTree::Group(g) => {
                let inner = fold_short_forms(g.stream());
                let mut ng = proc_macro2::Group::new(g.delimiter(), inner);
                ng.set_span(g.span());
                out.extend([proc_macro2::TokenTree::Group(ng)]);
            }
            other => out.extend([other.clone()]),
        }
        i += 1;
    }
    out
}

/// `from_index` / `to_index` 调用 → 短名调用（参数组递归折叠）。
fn folded_call(
    method: &proc_macro2::TokenTree,
    args: &proc_macro2::Group,
) -> proc_macro2::TokenStream {
    let short = if ident_is(Some(method), "to_index") {
        "__ti"
    } else {
        "__ph"
    };
    let mut ng = proc_macro2::Group::new(args.delimiter(), fold_short_forms(args.stream()));
    ng.set_span(args.span());
    let mut call = proc_macro2::TokenStream::new();
    call.extend([
        proc_macro2::TokenTree::Ident(proc_macro2::Ident::new(short, args.span())),
        proc_macro2::TokenTree::Group(ng),
    ]);
    call
}

fn is_punct(tt: Option<&proc_macro2::TokenTree>, ch: char) -> bool {
    matches!(tt, Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == ch)
}

fn ident_is(tt: Option<&proc_macro2::TokenTree>, name: &str) -> bool {
    matches!(tt, Some(proc_macro2::TokenTree::Ident(i)) if i == name)
}

fn is_colon2(toks: &[proc_macro2::TokenTree], i: usize) -> bool {
    is_punct(toks.get(i), ':') && is_punct(toks.get(i + 1), ':')
}

/// `fold_short_forms` 折叠出来的短名定义（模块内发射一次）。
///
/// 放在折叠**之后**发射，所以这两行自己仍写全路径——再由 `rewrite_path_roots`
/// 按宿主 crate 改写（否则跨 crate 生成时 `forge_ir::` 会指错）。
fn short_form_helpers() -> proc_macro2::TokenStream {
    quote::quote! {
        /// 生成物内部短名（v18 S8c）：`forge_ir::RegClass` 的别名。
        #[allow(dead_code)]
        type __RC = forge_ir::RegClass;
        /// 生成物内部短名（v18 S8c）：`Reg::from_index` 的薄封装（占位寄存器构造）。
        #[allow(dead_code)]
        #[inline]
        fn __ph(idx: u32, class: forge_ir::RegClass) -> Reg {
            <Reg as forge_ir::PhysReg>::from_index(idx, class)
        }
        /// 生成物内部短名（v18 S8c）：`Reg::to_index` 的薄封装。
        #[allow(dead_code)]
        #[inline]
        fn __ti(r: Reg) -> u32 {
            forge_ir::PhysReg::to_index(r)
        }
    }
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

    /// v18 S7d：`parts` 名字解析（未知名/空表报错，错误消息列举可用名）。
    #[test]
    fn parts_from_names() {
        let all = Parts::all();
        assert!(all.is_full());
        assert_eq!(all.names(), "encode, decode, asm, tm");
        let p = Parts::from_names(&["asm".to_string(), "tm".to_string()]).expect("两个部件");
        assert!(p.asm && p.tm && !p.encode && !p.decode);
        assert!(!p.is_full());
        assert_eq!(p.names(), "asm, tm");
        let e = Parts::from_names(&["nope".to_string()]).unwrap_err();
        assert!(e.contains("未知部件"), "{e}");
        assert!(e.contains("encode / decode / asm / tm"), "{e}");
        let e = Parts::from_names(&[]).unwrap_err();
        assert!(e.contains("空数组"), "{e}");
    }

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
