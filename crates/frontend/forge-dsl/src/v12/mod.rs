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
mod tests;

pub(crate) use model::V12Model;
pub(crate) use parse::parse;

/// v12 错误：解析（含 TOML 语法/结构诊断）与语义校验。
///
/// `line`/`col` 是 **ISA TOML 内**的 1-based 位置；`isa_from_file!` 拼上文件
/// 绝对路径后构成可点击的 `路径:行:列`。锚不到具体声明时退化为 `1:1`
/// （只丢跳转，不产生错误定位）。
#[derive(Debug, thiserror::Error)]
pub enum V12Error {
    #[error("{line}:{col}: v12 parse: {msg}")]
    Parse {
        line: usize,
        col: usize,
        msg: String,
    },
    #[error("{line}:{col}: v12 validation: {msg}")]
    Validation {
        line: usize,
        col: usize,
        msg: String,
    },
}

/// 解析 + 语义校验一步到位（`isa_from_file!` 的 v12 入口）。
pub(crate) fn parse_and_validate(source: &str) -> Result<V12Model, V12Error> {
    let model = parse(source)?;
    validate::validate(&model).map_err(|msg| {
        let (line, col) = diag::anchor(source, &msg);
        V12Error::Validation { line, col, msg }
    })?;
    Ok(model)
}

/// 给生成器的裸消息补上 `行:列:` 前缀（codegen 的错误不经 `V12Error`）。
pub(crate) fn anchor_msg(source: &str, msg: &str) -> String {
    let (line, col) = diag::anchor(source, msg);
    format!("{line}:{col}: {msg}")
}
