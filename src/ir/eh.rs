//! 异常处理 (Exception Handling) IR 扩展。
//!
//! 添加 `invoke`/`landingpad` 指令支持，用于 Itanium C++ EH (Linux/macOS)
//! 和 Windows SEH 的栈展开。
//!
//! ## 指令
//!
//! - **Invoke**: 带异常路径的调用 — 如果被调用函数抛出异常，跳转到 landing pad
//! - **LandingPad**: 异常处理入口 — 接收异常对象和选择器
//! - **Resume**: 重新抛出异常（向上层传播）
//!
//! ## 使用
//!
//! ```ignore
//! // try { call @may_throw() } catch { handle_exception }
//! %result = invoke @may_throw() to %normal_bb unwind %landing_bb
//! landing_bb:
//!   %ex = landingpad
//!   %handled = call @handle_exception(%ex)
//!   resume %ex
//! ```

use super::types::*;

/// 异常处理指令（扩展 Opcode 之外的特殊指令）。
///
/// 这些是 EH 专用指令，与普通 IR 指令分开管理，
/// 以避免不涉及 EH 的 pass 需要处理它们。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EhOpcode {
    /// 带异常路径的调用。
    /// operands: [func_ptr, arg0, arg1, ...]
    /// 结果: 调用返回值（正常路径）
    Invoke {
        /// 被调用的函数引用。
        func: super::FuncRef,
        /// 正常返回时跳转到的目标块。
        normal_target: super::BlockId,
        /// 异常发生时跳转到的 landing pad 块。
        unwind_target: super::BlockId,
    },
    /// Landing pad — 异常处理入口。
    /// 接收运行时异常对象，决定是否处理该异常。
    LandingPad {
        /// 捕获子句（catch 类型过滤）。
        clauses: Vec<CatchClause>,
        /// 是否清理（cleanup）landing pad（总是执行）。
        is_cleanup: bool,
    },
    /// 重新抛出异常（传播到上层调用栈）。
    Resume {
        /// 异常值。
        exception: crate::ir::Value,
    },
}

/// Landing pad 的捕获子句。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatchClause {
    /// 捕获所有异常（catch-all）。
    CatchAll,
    /// 捕获指定类型的异常（通过类型信息指针过滤）。
    CatchType {
        /// 类型信息全局变量的索引。
        type_info_index: u32,
    },
    /// 过滤子句（异常规范）。
    Filter {
        /// 过滤器函数索引。
        filter_index: u32,
    },
}

/// 函数级 EH 属性。
#[derive(Clone, Debug, Default)]
pub struct EhFuncAttributes {
    /// 此函数是否使用了异常处理。
    pub has_eh: bool,
    /// 此函数是否保证不抛出异常（nounwind）。
    pub nounwind: bool,
    /// Landing pad 列表（按 block ID 索引）。
    pub landing_pads: Vec<(super::BlockId, LandingPadInfo)>,
    /// 需要注册的个人ality 函数。
    pub personality_function: Option<String>,
}

/// Landing pad 运行时信息。
#[derive(Clone, Debug)]
pub struct LandingPadInfo {
    /// LP 的类型信息。
    pub clauses: Vec<CatchClause>,
    /// 是否总是执行清理代码。
    pub is_cleanup: bool,
    /// LP 的异常对象类型。
    pub exception_type: Type,
}

impl EhFuncAttributes {
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 personality 函数（如 `__gxx_personality_v0`）。
    pub fn with_personality(mut self, name: &str) -> Self {
        self.personality_function = Some(name.to_string());
        self
    }

    /// 标记为 nounwind。
    pub fn nounwind(mut self) -> Self {
        self.nounwind = true;
        self
    }
}
