//! v12 — 唯一 ISA-DSL 语法（严格 TOML）。
//!
//! 与 v11 的关系：**无兼容**。v11 的 `encoding` 字符串、`@原语`、紧凑 `fields` 串、
//! `when` 谓词串全部移除；本模块是唯一模型，forge-dsl 代码生成直接消费本模型
//! （v11 文件解析必然失败——`deny_unknown_fields`）。v11 语法层已物理删除。
//!
//! 模型范围：`[meta]` / `[reg.*]` / `[conventions.bitfields]`（+modrm）/
//! `[[operand_slots]]` / `[[forms]]`（语义键）/ `[[instructions]]` /
//! `[[families]]` / `[[lowering]]`（符号化操作数）/ `[abi]` / `[emit]`；
//! 代码生成见 `codegen`（自包含 encode/decode/asm + TargetMachine 集成层）。

#![cfg_attr(not(test), allow(dead_code))]

pub(crate) mod codegen;
pub(crate) mod diag;
mod match_tree;
pub(crate) mod model;
mod parse;
mod pred;
pub(crate) mod shared;
mod validate;

#[cfg(test)]
mod diag_matrix_tests;
#[cfg(test)]
mod tests;

pub(crate) use model::V12Model;
pub(crate) use parse::parse;

/// v12 错误：解析（TOML 语法/结构）与语义校验。
///
/// `line`/`col` 是 **ISA TOML 内**的 1-based 位置；`isa_from_file!` 拼上文件绝对路径后
/// 构成可点击的 `路径:行:列`。
///
/// **S1 起校验错误是多条**：`Validation.msg` 是全部消息（`\n` 连接，便于既有
/// `msg.contains(...)` 断言），结构化明细在 `diags` 里；`Display` 用无路径形态渲染
/// （`行:列: 码: 消息`），`render(Some(path))` 给宏用（每行带可点击路径）。
#[derive(Debug, thiserror::Error)]
pub enum V12Error {
    #[error("{line}:{col}: DSL-TOML: {msg}")]
    Parse {
        line: usize,
        col: usize,
        msg: String,
    },
    #[error("{}", render_diags(.diags, *dropped, None))]
    Validation {
        line: usize,
        col: usize,
        msg: String,
        diags: Vec<diag::Diag>,
        /// 被上限丢弃的条数（渲染尾巴用）。
        dropped: usize,
    },
}

/// 渲染诊断集合（`path = None` 时不加路径前缀）。
pub(crate) fn render_diags(
    diags: &[diag::Diag],
    dropped: usize,
    path: Option<&std::path::Path>,
) -> String {
    diag::render(diags, dropped, path)
}

impl V12Error {
    /// 渲染成可点击文本（宏路径用 `Some(isa_path)`）。
    pub(crate) fn render(&self, path: Option<&std::path::Path>) -> String {
        match self {
            V12Error::Parse { line, col, msg } => match path {
                Some(p) => format!("{}:{line}:{col}: DSL-TOML: {msg}\n", p.display()),
                None => format!("{line}:{col}: DSL-TOML: {msg}\n"),
            },
            V12Error::Validation { diags, dropped, .. } => render_diags(diags, *dropped, path),
        }
    }

    /// 结构化诊断（`Parse` 只有一条）。
    pub(crate) fn diags(&self) -> Vec<diag::Diag> {
        match self {
            V12Error::Parse { line, col, msg } => vec![diag::Diag {
                code: "DSL-TOML",
                line: *line,
                col: *col,
                msg: msg.clone(),
                notes: Vec::new(),
            }],
            V12Error::Validation { diags, .. } => diags.clone(),
        }
    }
}

/// 解析 + 语义校验一步到位（`isa_from_file!` 与单测的入口）。
///
/// 校验**收集全部错误**（按节 + 逐条），一次返回；`Parse` 仍是单条（TOML 语法错
/// 没法继续解析）。
pub(crate) fn parse_and_validate(source: &str) -> Result<V12Model, V12Error> {
    let model = parse(source)?;
    let idx = diag::DeclIndex::build(source);
    let mut diags = diag::Diags::new();
    validate::validate_all(&model, &idx, &mut diags);
    if diags.is_empty() {
        return Ok(model);
    }
    let first = diags.iter().next().expect("非空");
    let (line, col) = (first.line, first.col);
    let msg = diags
        .iter()
        .map(|d| d.msg.clone())
        .collect::<Vec<_>>()
        .join("\n");
    Err(V12Error::Validation {
        line,
        col,
        msg,
        dropped: diags.dropped(),
        diags: diags.into_items(),
    })
}

/// 给生成器的裸消息补上 `路径:行:列` 前缀（codegen 的错误不经 `V12Error`）。
pub(crate) fn anchor_msg(source: &str, isa_path: &std::path::Path, msg: &str) -> String {
    let idx = diag::DeclIndex::build(source);
    let a = idx.anchor(msg);
    format!(
        "{}:{}:{}: {}: {msg}",
        isa_path.display(),
        a.line,
        a.col,
        a.code
    )
}
