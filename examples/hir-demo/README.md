# hir-demo（示例 / 非产品）

Mini C 子集编译器：`.lx` 语法 → CST/AST（`forge-grammar`）→ HIR 砖块框架（`forge-hir`）→
forge-ir → forge-codegen 后端 → JIT 执行。它同时演示"同一份前端走两条后端路"（`codegen.rs`
手写降级 vs `codegen_hir.rs` 砖块框架）。

## 为什么它在 `examples/` 而不是 `crates/`

2026-10-11（项目 v26 M2）审查实测：`forge-hir`(3533 行) + `forge-hir-macro`(835) + `mini_c`
**只服务这一个示例**，`forge-grammar` 的唯一消费者也是它——即约 11.5k 行只养一个演示。
按"要么真用、要么别养"的判据，这一线**降级为示例**：

- 目录搬进 `examples/hir-demo/`，并从工作区 `exclude` ⇒ **不参与** `cargo test --workspace`
  与 CI 门禁（仓库的其余部分不再为它的编译/测试付费）；
- `forge-grammar`（CST/AST/visit 核心 + `ir.lx` 测试）**仍是工作区成员**——它自带完整测试，
  且是这一线的前端引擎。

## 手动检查（不在门禁里，改动后自己跑）

```bash
cargo check --manifest-path examples/hir-demo/Cargo.toml
cargo test  --manifest-path examples/hir-demo/Cargo.toml
```

## 后果（如实写明）

它不再有 CI 覆盖 ⇒ 会随工作区演进**静默失效**。若要重新纳入门禁，做两件事即可：
把它移回 `crates/` 并加回 `members`，或（更根本地）给它一条真实产品路径。
