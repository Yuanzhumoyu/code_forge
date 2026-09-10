# clippy 修复清单

## ⚠️ ARCHIVED（2026-09）

> 2026-07-31 clippy 全量清零的单次整改记录（已完成）。
> 本文为历史记录，仅供参考；代码现状以仓库代码与现行文档为准，不再维护。

`cargo clippy --workspace --exclude forge-rustc --all-targets` 全量清零记录。

## 背景

2026-07-31 全量 clippy 检查共 17 处警告：6 项手写代码问题（8 处）+ 11 处
lalrpop 生成代码问题。全部修复后 **0 warning / 0 error**。

## A. 手写代码修复（6 项 8 处）

| # | 文件 | lint | 修复内容 |
| --- | --- | --- | --- |
| 1 | `forge-ir/src/ir_parser/semantics.rs:65` | `match_like_matches` | `match k.as_str() { "triple" => ..., _ => {} }` → `if k.as_str() == "triple" { ... }` |
| 2 | `forge-opt/src/lib.rs:453`（测试） | `type_complexity` | `Vec<(&str, fn() -> Function)>` → `type FunctionBuilderFn = fn() -> Function` 别名 |
| 3 | `forge-ir/tests/ir_parser_llvm.rs:15` | `clone_on_copy` | `.opcode.clone()` → `.opcode`（`Opcode` 实现 `Copy`） |
| 4 | `forge-ir/src/display.rs` | `items_after_test_module` | `#[cfg(test)] mod tests` 移到文件尾（`value_as_literal` 之后） |
| 5 | `forge-ir/src/ir_parser/lexer.rs` | `items_after_test_module` | `#[cfg(test)] mod tests` 移到文件尾（`TokenStream` 定义之后） |
| 6a | `forge-codegen/src/machine/frame.rs:46` | 预留位置（`unimplemented!`） | `emit_epilogue_jump` 默认实现补为 `Err(CompileError::Unimplemented(...))` |
| 6b | `forge-codegen/src/machine/frame.rs:58` | 预留位置（`unimplemented!`） | `emit_spill_load` 默认实现同上（错误优雅传播替代 panic） |
| 6c | `forge-codegen/src/machine/frame.rs:70` | 预留位置（`unimplemented!`） | `emit_spill_store` 默认实现同上 |

> 注 6a-6c：这三个是 `TargetFrameLowering` trait 的默认方法（各 ISA 覆盖）。
> 原 `unimplemented!()` 在未覆盖 ISA 上会 panic；补为返回
> `CompileError::Unimplemented` 错误，失败可被上层捕获传播而非崩溃。

## B. 生成代码豁免（11 处）

lalrpop 生成的 `grammar.rs`（build 输出）有 11 处 `redundant_field_names`
（生成器的 struct 初始化风格），**生成代码不手改**（每次 build 重新生成）。

方案：`lalrpop_mod!` 宏支持 `$(#[$attr])*` 透传 attribute——

```rust
// forge-ir/src/ir_parser/mod.rs
lalrpop_mod!(
    #[allow(clippy::redundant_field_names)]
    pub grammar,
    "/ir_parser/grammar.rs"
);
```

attribute 落到 `mod grammar` 声明上，覆盖 include! 的生成代码。
（注：build.rs 内注入 `#![allow]` 的方案被否决——inner attribute 在
include! 上下文非法，报 "an inner attribute is not permitted in this context"。）

## 验证

- `cargo clippy --workspace --exclude forge-rustc --all-targets` → **0 warning / 0 error**
- `cargo clean -p forge-ir` + rebuild → 编译 0 error（豁免随 build 稳定生效）
- `cargo test --workspace --exclude forge-rustc` → 全量通过（41 组 ok，0 FAILED）
