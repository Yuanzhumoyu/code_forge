//! 编译产物携带的**源码签名**（v30 M4）。
//!
//! JIT 的类型化调用（`JitLoader::get::<F>`）需要拿"产物是按什么签名编的"来校验
//! `F` 的形参/返回类型。签名在**整模块编译**时从原函数取（`param_types()` /
//! `return_types()` / `calling_convention`），变参事实来自模块的 `ModuleSigs`；
//! 单函数入口从 `FunctionSignature` 取。产物是纯数据，签名跟着 [`CompiledUnit`] 走。

use forge_ir::{CallConvId, Function, FunctionSignature, TypeId};

/// 一个产物函数的源码签名（JIT 装载期校验用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JitSignatureInfo {
    /// 形参类型（顺序即 ABI 位置序）。
    pub params: Vec<TypeId>,
    /// 返回类型（0 个 = void，1 个 = 单值）。
    pub returns: Vec<TypeId>,
    /// 调用约定（IR 侧标识；`c` 表示平台 C 约定）。
    pub conv: CallConvId,
    /// 是否变参（来自模块签名表；单函数入口来自 `FunctionSignature`）。
    pub variadic: bool,
}

impl JitSignatureInfo {
    /// 从已构建的 IR 函数取（`variadic` 由模块签名表/调用方提供）。
    pub fn from_function(func: &Function, variadic: bool) -> Self {
        Self {
            params: func.param_types(),
            returns: func.return_types(),
            conv: func.calling_convention.clone(),
            variadic,
        }
    }

    /// 从源码签名取（单函数便捷入口）。
    pub fn from_signature(sig: &FunctionSignature) -> Self {
        Self {
            params: sig.params.iter().map(|(ty, _)| *ty).collect(),
            returns: sig.returns.clone(),
            conv: sig.calling_convention.clone(),
            variadic: sig.variadic,
        }
    }
}
