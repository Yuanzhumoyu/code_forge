//! v12 — 唯一 ISA-DSL 语法（严格 TOML）。
//!
//! 与 v11 的关系：**无兼容**。v11 的 `encoding` 字符串、`@原语`、紧凑 `fields` 串、
//! `when` 谓词串全部移除；本模块是唯一模型，forge-dsl 代码生成（迭代 2+）将直接
//! 消费本模型。v11 文件解析必然失败（`deny_unknown_fields`）。
//!
//! 迭代 1 范围：`[meta]` / `[reg.*]` / `[conventions.bitfields]`（+modrm/rex/
//! opsize_prefix）/ `[[operand_slots]]` 的模型 + 严格解析 + 语义校验。
//! `[[forms]]` / `[[instructions]]` / `[[families]]` / `[[lowering]]` / `[abi]` /
//! `[emit]` 已纳入模型与结构校验，语义键集合与谓词语言在迭代 3/4 定型。
//!
//! 迭代 2 起 codegen 将消费本模块（届时 `isa_from_file!` 切换到 v12 路径）；
//! 在此之前 lib 构建中本模块仅被单测使用，放行 dead_code 以通过
//! `clippy -D warnings` 门禁（test 构建仍完整检查）。

#![cfg_attr(not(test), allow(dead_code))]

pub(crate) mod codegen;
pub(crate) mod model;
mod parse;
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

/// 解析 + 语义校验一步到位（未来 `compile_source` 的 v12 入口）。
pub(crate) fn parse_and_validate(source: &str) -> Result<V12Model, V12Error> {
    let model = parse(source)?;
    validate::validate(&model).map_err(V12Error::Validation)?;
    Ok(model)
}
