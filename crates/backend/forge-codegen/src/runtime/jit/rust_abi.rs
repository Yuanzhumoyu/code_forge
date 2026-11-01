//! `jit_rust_abi` feature 的反射边界（v32 §7）。
//!
//! 目标：Rust 类型**不必** `#[repr(C)]`，反射读取逻辑字段/变体后生成 C ABI 线格式
//! 与适配器。当前环境 registry 未 vendor `bevy_reflect`，所以这里先固定边界：
//! 反射来源（Bevy `Reflect`，或官方反射稳定后替换）实现 [`JitReflect`]，核心
//! `JitType`/`bind` 不依赖任何反射库。
//!
//! **不在核心 crate 里硬编码 Bevy**：启用 `jit_rust_abi` 时才编译本模块；适配器
//! 只把 `ReflectKind`（Struct/TupleStruct/Enum/Value/…）投影成 [`JitType`]，
//! 带字段枚举/union 的按值走 tag+payload 线格式，Rust 侧无需 C 布局。

use crate::pipeline::signature::JitType;

/// Rust 类型的反射投影（Bevy `Reflect` 适配器实现点）。
pub trait JitReflect {
    /// 逻辑布局描述；`JitType::Enum/Union` 由反射字段/变体生成。
    fn jit_type() -> JitType;
}
