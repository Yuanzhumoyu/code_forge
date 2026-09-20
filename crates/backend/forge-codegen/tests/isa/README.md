# 测试夹具：ISA-DSL 示例谱（非发行后端）

这些 TOML 只服务于 `forge-codegen` 的测试：指令集极小，便于对
汇编器/解码器/编码器/lowering 做穷尽断言。它们**不**属于
`forge-codegen` 的公开面——库本体只有 `x86_v12` / `arm64_v12` /
`riscv64_v12` 这类真实后端（见 `crates/backend/forge-codegen/src/arch/`）。

| 文件 | 用途 |
| --- | --- |
| `demo_v12.toml` | 同助记符多宽度自动分发（`add`/`mov` 按操作数类分发到 16/32/64 位编码） |
| `demo8_v12.toml` | **1 字节寄存器** ISA（唯一 `[reg.gpr1]` 组）——「寄存器宽度写死」回归夹具 |
| `demo_inst8_v12.toml` | **8 位指令字** ISA；label 域在字内（bits 0..2）——「指令字宽写死」回归夹具 |
| `demo_inst12_v12.toml` | **12 位指令字**（非 8 倍数）：2 字节存储 + 填充位必须为 0 |
| `demo_inst100_v12.toml` | **100 位指令字**（13 字节，超机器字）：位域落在 bit 92..100 |
| `demo_mixed16_32_v12.toml` | **混合字长**（`[encoding] kind = "mixed"`，`widths = [16, 32]`）：低 2 位判别短/长编码，解码按字长升序分组 |

宿住方式见 `tests/common/mod.rs`：`forge_dsl::isa_from_file!("…", krate = forge_codegen)`
让生成代码落在测试 crate 里、且依赖面只有 `forge_codegen` 的公开 API。

回归守卫：`tests/library_surface.rs`（库源码/仓库根 `isa/` 不得再出现 demo 谱）。
字宽方向的规范见 `docs/reference/isa-dsl.md` 的「`[encoding]` — 指令宽度三态」与
「指令字宽」两节；生成期自测（v18 S6）见同文档「生成期自测（`__spec_tests`）」节
——夹具在 `tests/common/mod.rs` 里用 `spec_tests = false` 宿住（避免同一份谱被多个
测试二进制重复展开），由 `tests/spec_tests_v12.rs` 重新打开其中三个极端形状夹具。
