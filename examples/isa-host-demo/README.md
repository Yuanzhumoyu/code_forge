# isa-host-demo — 最小外部宿主（ISA-DSL v19 V2）

这个 crate 只有一个目的：给出 ISA-DSL「**谱可以独立接入**」的硬证据——一份 ISA 谱
不需要 `forge-codegen`（JIT / regalloc / 指令发射）也能承载、编译、跑通生成期自测。

## 它证明了什么

| 事实 | 证据 |
| --- | --- |
| 运行期依赖**只有** `forge-isa-runtime` | `cargo tree -p isa-host-demo --edges normal,build`（无 forge-codegen / forge-opt / forge-dsl） |
| 生成物路径一律指向运行时 crate | `tests/host_surface.rs::generated_file_targets_the_runtime_crate_only`（生成物文本里 **0 处** `crate::`、0 处 `forge_codegen`） |
| 部件可以只取 `encode`/`decode`/`asm` | `build.rs` 的 `parts`——不含 `tm` ⇒ 不注册后端、不需要任何编译管线 |
| 不带 `tm` 也能开生成期自测 | `cargo test -p isa-host-demo` 会跑出 9 条 `toy16::__spec_tests::*`（v19 V2 放宽了这条约束） |
| 生成物可以**不用 proc-macro** | `build.rs` 直接调 `pregenerate`，`src/lib.rs` 只写一句 `include!` |

依赖面本身也被守卫钉住（`tests/host_surface.rs::dependency_surface_is_runtime_only`）：
`Cargo.toml` 一旦加回 `forge-codegen`/`forge-ir`/`forge-dsl`，或加 dev-dependencies，测试立刻失败。

## 怎么跑

```bash
cargo test -p isa-host-demo                       # 9 条规格自测 + 5 条宿主用例
cargo tree -p isa-host-demo --edges normal,build  # 依赖闭包（应只有 forge-isa-runtime 一棵树）
```

## 与 `isa_from_file!` 的关系

`isa_from_file!` 展开出来只有一句
`include!(concat!(env!("OUT_DIR"), "/<参数哈希文件名>"))`；本 crate 手写这两步
（`build.rs` 的 `pregenerate` + `src/lib.rs` 的 `include!`），因此连 proc-macro crate
都不依赖——这是"最极端"的依赖面。**一般宿主写宏即可**，两条路落在同一份生成物、
同一个文件名上（`build.rs` 里显式断言了这一点）。

谱是教程 §0 的玩具 ISA（16 位定宽、4 条指令），见 [`isa/toy16.toml`](isa/toy16.toml)
与 [`docs/guides/isa-dsl-tutorial.md`](../../docs/guides/isa-dsl-tutorial.md)。
