//! 宿主接入（v19 V2）：**不用 proc-macro** 的最小宿主。
//!
//! 生成物的唯一写者是 build script——三条硬约束（RA 只加载分析开始前存在的文件、
//! `TokenStream::to_string()` 上下文相关、单一写者）见
//! `docs/guides/rust-analyzer-notes.md` §1。
//!
//! 本 crate 刻意走最裸的一条路：`isa_from_file!` 展开出来只是一句
//! `include!(concat!(env!("OUT_DIR"), "/<参数哈希文件名>"))`，这里直接调它背后的
//! 公开函数（`pregenerate` + `generated_file_name`），于是**运行期依赖只有
//! `forge-isa-runtime`**（连 proc-macro crate 都不需要）。一般宿主写
//! `forge_dsl::isa_from_file!("isa/…toml", parts = [...])` 即可——两条路落在
//! 同一个文件名、同一份生成物上（都经 `gen_file`），所以这里能钉住"名字一致"。

use forge_isa_dsl::ExpandOptions;
use forge_isa_dsl::Parts;
use forge_isa_dsl::gen_file::{MacroArgs, generated_dir, generated_file_name, pregenerate};

/// 谱路径：相对本 crate 根（build script 的 cwd 与 `CARGO_MANIFEST_DIR` 都是 crate 根，
/// `read_isa_file` 的候选顺序必然命中）。
const SPEC: &str = "isa/toy16.toml";

/// 只承载「编解码 + 汇编」：不要 TargetMachine 集成层（ABI/lowering/帧布局），
/// 也就不需要任何编译管线。`spec_tests = true` 仍然成立——生成期自测只需要
/// encode/decode/asm（`Parts::supports_spec_tests`，v19 V2 放宽）。
fn options() -> ExpandOptions {
    ExpandOptions {
        spec_tests: true,
        name: None,
        parts: Parts::from_names(&[
            "encode".to_string(),
            "decode".to_string(),
            "asm".to_string(),
        ])
        .expect("部件名合法"),
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // 谱（含 `include` 分片）必须登记为构建依赖：生成物是**文件**，只靠 lib 侧
    // `include_bytes!` 只会重编 lib、不会让 build script 重跑 ⇒ 文件停在旧谱上。
    for src in forge_isa_dsl::spec_source_files(SPEC).expect("读取谱来源失败") {
        println!("cargo:rerun-if-changed={}", src.display());
    }

    let args = MacroArgs {
        path: SPEC.to_string(),
        opts: options(),
    };
    let dir = generated_dir().expect("OUT_DIR（cargo 为 build script 提供）");
    let name = pregenerate(&args, &dir).expect("ISA 预生成失败");
    // 宏侧（`expand_file_emitted`）用同一个函数算名字——不一致就会让 lib.rs 的
    // `include!` 找不到文件。这里显式对一次，漂移时立刻炸在 build script 上。
    assert_eq!(name, generated_file_name(SPEC, &options()));
    println!("cargo:rustc-env=ISA_HOST_DEMO_GEN={name}");
}
