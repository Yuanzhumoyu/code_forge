//! 宿主面编译错误（v30 M1）。
//!
//! `IrError` 仍是**唯一**的错误内容（分类学见 `backend-pipeline.md` §9）；
//! [`CompileError`] 只在外面补两件宿主真正需要的事实：**失败发生在哪个阶段**、
//! **哪个函数**。缺口 ID 直通 `IrError::gap_id()`，台账守卫不受影响。
//!
//! 边界：erased/插件通道仍返回 `IrError`，用 [`CompileError::into_ir`] 或
//! `From<CompileError> for IrError` 折叠（会丢掉阶段标签，只允许在边界这么做）。

use std::fmt;

use forge_ir::{ImmStr, IrError};

use crate::pipeline::stage::Stage;

/// 一次编译失败：阶段 + 函数 + 原始 [`IrError`]。
#[derive(Clone, Debug)]
pub struct CompileError {
    stage: Stage,
    function: Option<ImmStr>,
    source: IrError,
}

impl CompileError {
    /// 单函数失败。
    pub(crate) fn at(stage: Stage, function: &ImmStr, source: IrError) -> Self {
        Self {
            stage,
            function: Some(function.clone()),
            source,
        }
    }

    /// 整模块编排失败（没有具体函数，如编译线程 panic）。
    pub(crate) fn module(source: IrError) -> Self {
        Self {
            stage: Stage::Module,
            function: None,
            source,
        }
    }

    /// 失败阶段。
    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// 失败函数名（整模块编排失败时为空）。
    pub fn function(&self) -> Option<&str> {
        self.function.as_deref()
    }

    /// 能力缺口 ID（`[cap:ID]`）；不是缺口错误 ⇒ `None`。
    pub fn gap_id(&self) -> Option<String> {
        self.source.gap_id()
    }

    /// 原始错误（保留四分类与缺口文本）。
    pub fn ir_error(&self) -> &IrError {
        &self.source
    }

    /// 折叠回 [`IrError`]（erased/插件边界用）。
    pub fn into_ir(self) -> IrError {
        self.source
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.function.as_deref() {
            Some(name) => write!(
                f,
                "编译失败（阶段 {}，函数 {name}）：{}",
                self.stage.id(),
                self.source
            ),
            None => write!(f, "编译失败（阶段 {}）：{}", self.stage.id(), self.source),
        }
    }
}

impl std::error::Error for CompileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl From<CompileError> for IrError {
    fn from(e: CompileError) -> Self {
        e.source
    }
}
