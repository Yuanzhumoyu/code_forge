//! v12 — 唯一 ISA-DSL 语法（严格 TOML）。
//!
//! 与 v11 的关系：**无兼容**。v11 的 `encoding` 字符串、`@原语`、紧凑 `fields` 串、
//! `when` 谓词串全部移除；本模块是唯一模型，forge-dsl 代码生成直接消费本模型
//! （v11 文件解析必然失败——`deny_unknown_fields`）。v11 语法层已物理删除。
//!
//! 模型范围：`[meta]` / `[reg.*]` / `[conventions.bitfields]`（+modrm/rex/
//! opsize_prefix）/ `[[operand_slots]]` / `[[forms]]`（语义键）/ `[[instructions]]` /
//! `[[families]]` / `[[lowering]]`（符号化操作数）/ `[abi]` / `[emit]`；
//! 代码生成见 `codegen`（自包含 encode/decode/asm + TargetMachine 集成层）。

#![cfg_attr(not(test), allow(dead_code))]

pub(crate) mod codegen;
pub(crate) mod model;
mod parse;
mod pred;
mod validate;

#[cfg(test)]
mod tests;

pub(crate) use model::V12Model;
pub(crate) use parse::parse;

/// v12 错误：解析（含 TOML 语法/结构诊断）与语义校验。
#[derive(Debug, thiserror::Error)]
pub enum V12Error {
    #[error("v12 parse: {0}")]
    Parse(String),
    #[error("v12 validation: {0}")]
    Validation(String),
}

/// 解析 + 语义校验一步到位（`isa_from_file!` 的 v12 入口）。
pub(crate) fn parse_and_validate(source: &str) -> Result<V12Model, V12Error> {
    let model = parse(source)?;
    validate::validate(&model).map_err(V12Error::Validation)?;
    Ok(model)
}
