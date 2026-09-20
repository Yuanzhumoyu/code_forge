//! v12 asm 规范层：汇编源码 token 模型 + 参考词法器 + 参考操作数解析。
//!
//! v11 时代的插值词法（`{{0:gpr}}`、`Interpolation*`）已删除——asm 模板语法
//! 已改为 v12 的 `{n:[槽:角色]}` 占位符（模板段解析在 `v12/codegen`，编译期
//! 消费）。本模块是 **v12 asm 源码文本**的规范实现：
//!
//! - [`lex::Tok`]：源码 token 模型（寄存器/助记符/数字/标点）。
//! - [`lex::tokenize`]：logos 参考词法器。`v12/codegen` 生成器在编译期用它把
//!   asm 模板的**字面段** token 化（生成匹配代码），并在生成模块内**镜像**出
//!   std-only 的 `__Tok`/`__lex`（token 集合与词法语义必须一致）。
//! - [`operand`]：参考操作数解析（reg/imm/label/cond/mem，含范围校验与符号
//!   引用），生成器镜像其语义；单测载体。
//!
//! 为什么需要"镜像"而非直接复用：forge-dsl 是 proc-macro crate，不能导出
//! 非宏项（架构规则 3），生成模块无法调用本模块代码，故生成器按本规范
//! 重新发出等价的 std-only 实现。

pub(crate) mod lex;
pub(crate) mod operand;

pub use lex::{Tok, tokenize};
