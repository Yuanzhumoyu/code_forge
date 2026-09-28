//! 测试共享夹具：在**测试 crate**里生成三个 demo ISA（库本体不含它们）。
//!
//! `isa_from_file!(…)` 把生成物里的 `crate::…` 改写为
//! `forge_codegen::…`、`forge_ir::…` 改写为 `forge_codegen::ir::…`——因此生成
//! 代码只依赖 forge-codegen 的**公开 API**，与"生成在库内部"完全等价，却不再
//! 把 demo 谱编进 rlib。
//!
//! 谱文件在 `tests/isa/`（路径相对 `CARGO_MANIFEST_DIR` 解析）：
//! `demo_v12`（8 字节寄存器 / 32 位字）、`demo8_v12`（1 字节寄存器 / 32 位字）、
//! `demo_inst8_v12`（8 位指令字）、`demo_inst12_v12`（12 位字，非 8 倍数）、
//! `demo_inst100_v12`（100 位字：超机器字 + 非 8 倍数）、
//! `demo_mixed16_32_v12`（**混合字长**：16 位短编码 + 32 位长编码共存）、
//! `include_root_v12`（**多文件谱**：`include` 片段 + `[[override]]`，v18 S7d；
//! 另以 `name`/`parts` 再展开一份"只有编码器"的模块）。
//!
//! `spec_tests = false`：本文件被**多个测试二进制**包含，生成的自测会在每个二进制里
//! 重复跑一遍。生成期自测（v18 S6）由专门的二进制
//! `tests/spec_tests_v12.rs` 重新宿主这三个夹具并打开——那里每个夹具只跑一次。
#![allow(dead_code)] // 各 test target 只用到其中一个夹具；未用到的分支不算错误

forge_dsl::isa_from_file!("tests/isa/demo_v12.toml", spec_tests = false);
forge_dsl::isa_from_file!("tests/isa/demo8_v12.toml", spec_tests = false);
forge_dsl::isa_from_file!("tests/isa/demo_inst8_v12.toml", spec_tests = false);
forge_dsl::isa_from_file!("tests/isa/demo_inst12_v12.toml", spec_tests = false);
forge_dsl::isa_from_file!("tests/isa/demo_inst100_v12.toml", spec_tests = false);
forge_dsl::isa_from_file!("tests/isa/demo_mixed16_32_v12.toml", spec_tests = false);
// v18 S7d：`include` 组合（根 + 片段两个文件合成一份谱）+ `[[override]]`。
// 生成物里登记了**两个**来源文件的 `include_bytes!`（改任一片段都触发重编译）。
forge_dsl::isa_from_file!("tests/isa/include_root_v12.toml", spec_tests = false);
// v18 S7d：`name = "…"`（模块名覆盖）+ `parts = ["encode"]`（只生成编码器）。
// 同一份谱再展开一次，证明部件选择在**真实宏路径**上也成立：这个模块里
// 没有 `decode`/`disassemble`/`assemble`/`TargetMachine`（另见
// `crates/frontend/forge-isa-dsl/tests/parts_selection.rs` 的文本级断言）。
forge_dsl::isa_from_file!(
    "tests/isa/include_root_v12.toml",
    spec_tests = false,
    name = "include_enc_v12",
    parts = ["encode"]
);

// ───────────────── 测试本地约定（v20 A5-3） ─────────────────

/// `demo_v12` 的**测试本地约定**：这份夹具谱没有绑定，而 v20 起"实参放哪/返回怎么回"
/// 由 `AbiRules` + `AbiBinding` 给（谱里那套 `[abi]` 键是生成期近似，正在逐键下线）。
/// 注册后 `CallConvId::Builtin(C)` 经 `aliases = ["c"]` 落到本约定，夹具函数就有 plan 了。
///
/// 数据照着夹具谱的口径写：`[[abi.arg_class]] int = X0-X3`、返回槽 X0、
/// `[stack] slot = 8 / align = 8`、`[machine] callee_saved_gpr = []`（无 callee-saved）。
const DEMO_RULES: &str = r#"
name = "demo"
position = "by_class"
int_pool = "int"
stack_align = 8
stack = { slot_bytes = 8, first_offset_slots = 0 }
aliases = ["c"]  # 这台机器（demo_v12）上的 C 约定
classify = [
  { when = { kind = "scalar" }, do = { direct = { pool = "int" } } },
  { when = { kind = "aggregate", size_le = 8 }, do = { direct = { pool = "int" } } },
]
ret_classify = [
  { when = { kind = "scalar" }, do = { direct = { pool = "ret_int" } } },
]
fallback = { stack = {} }
note = "测试夹具约定（demo_v12）：只覆盖夹具用到的标量参数/返回"
"#;

/// `demo_v12` 的绑定（池名 → 夹具的寄存器名）。
const DEMO_BINDING: &str = r#"
isa = "demo_v12"
conv = "demo"
[pools]
int = ["X0", "X1", "X2", "X3"]
ret_int = ["X0"]
"#;

/// `demo8_v12`（1 字节寄存器）的测试本地约定：`int = A0-A3`、`ret_int = A0`、
/// `[stack] slot = 1 / align = 1`、无 callee-saved。
const DEMO8_RULES: &str = r#"
name = "demo8"
position = "by_class"
int_pool = "int"
stack_align = 1
stack = { slot_bytes = 1, first_offset_slots = 0 }
aliases = ["c"]
classify = [
  { when = { kind = "scalar" }, do = { direct = { pool = "int" } } },
]
ret_classify = [
  { when = { kind = "scalar" }, do = { direct = { pool = "ret_int" } } },
]
fallback = { stack = {} }
note = "测试夹具约定（demo8_v12）：1 字节寄存器的标量参数/返回"
"#;

/// `demo8_v12` 的绑定。
const DEMO8_BINDING: &str = r#"
isa = "demo8_v12"
conv = "demo8"
[pools]
int = ["A0", "A1", "A2", "A3"]
ret_int = ["A0"]
"#;

/// 幂等注册两份 demo 夹具的测试本地约定（编译夹具函数的测试先调它）。
///
/// 为什么必须显式调：注册表是进程级的、且**夹具谱里没有约定数据**——不注册的话
/// `Builtin(C)` 解析到内置 `c`（无 `(demo, c)` 绑定）⇒ `AbiPlan` 算不出来，夹具就
/// 只能走"无 plan 回退"（那正是 A5-3 要删掉的东西）。
pub fn ensure_demo_conventions() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        use forge_codegen::pipeline::conv_registry::{register_binding_toml, register_rules_toml};
        register_rules_toml(DEMO_RULES).expect("demo 夹具的规则应能注册");
        register_binding_toml(DEMO_BINDING).expect("demo 夹具的绑定应能注册");
        register_rules_toml(DEMO8_RULES).expect("demo8 夹具的规则应能注册");
        register_binding_toml(DEMO8_BINDING).expect("demo8 夹具的绑定应能注册");
    });
}
