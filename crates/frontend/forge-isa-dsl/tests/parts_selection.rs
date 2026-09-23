//! `parts = [...]` 部件选择（v18 S7d）——**普通 lib 侧**证据。
//!
//! 断言对象是**生成出来的 token 文本**（去空白后做子串匹配）：受限的部件确实
//! **没有被发射**，而不是"恰好没被用到"。配套的编译级证据在
//! `forge-codegen/tests/include_v12_tests.rs`（只有一个 `encode` 的模块真的能编过）。
//!
//! 同一份谱（`include_root_v12.toml`，多文件）在这里也顺带钉住 `include`/`[[override]]`
//! 的合并结果。

use forge_isa_dsl::{ExpandOptions, Parts, expand_file};

const SPEC: &str = "crates/backend/forge-codegen/tests/isa/include_root_v12.toml";

fn opts(parts: Parts) -> ExpandOptions {
    ExpandOptions {
        spec_tests: false, // 部件受限时必须关（见下）
        name: None,
        parts,
    }
}

/// 去掉所有空白：`TokenStream::to_string()` 会在 token 之间插空格。
fn flat(ts: &proc_macro2::TokenStream) -> String {
    ts.to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn expand(parts: Parts) -> String {
    flat(&expand_file(SPEC, &opts(parts)).expect("展开多文件谱"))
}

// ── 1. 缺省（全开）= 四块都在 ──

#[test]
fn full_parts_emits_all_four() {
    let t = expand(Parts::all());
    for m in [
        "pubfnencode(",
        "pubfndecode(",
        "pubfndecode_partial(",
        "pubfndisassemble(",
        "pubfnassemble(",
        "pubstructTargetMachine",
    ] {
        assert!(t.contains(m), "全开时应有 `{m}`");
    }
    // 全开 = 历史行为：`generate_with` 与 `generate_with_parts(all)` 同一条路径。
    assert_eq!(t, expand(Parts::default()), "缺省 = 全开");
}

// ── 2. 逐件开关 ──

#[test]
fn encode_only_drops_the_other_three() {
    let t = expand(Parts {
        encode: true,
        decode: false,
        asm: false,
        tm: false,
    });
    assert!(t.contains("pubfnencode("), "encode 在");
    assert!(!t.contains("pubfndecode("), "decode 不应发射");
    assert!(!t.contains("pubfndisassemble("), "disassemble 不应发射");
    assert!(!t.contains("pubfnassemble("), "assemble 不应发射");
    assert!(!t.contains("pubstructTargetMachine"), "tm 不应发射");
    // 公共前提仍在（`Inst` 字段类型是 `Reg`）。
    assert!(t.contains("pubenumInst"), "Inst 枚举恒定发射");
    assert!(
        t.contains("pubenumReg"),
        "Reg 枚举恒定发射（encode 也要它）"
    );
}

#[test]
fn tm_only_drops_the_function_parts() {
    let t = expand(Parts {
        encode: false,
        decode: false,
        asm: false,
        tm: true,
    });
    assert!(!t.contains("pubfnencode("), "encode 不应发射");
    assert!(!t.contains("pubfnassemble("), "asm 不应发射");
    assert!(t.contains("pubstructTargetMachine"), "tm 在");
    // `tm` 在时 `Reg` 由集成层发射（只有一份，不重复）。
    assert_eq!(t.matches("pubenumReg").count(), 1, "Reg 只应出现一次");
}

#[test]
fn asm_only_keeps_text_roundtrip_pair() {
    let t = expand(Parts {
        encode: false,
        decode: false,
        asm: true,
        tm: false,
    });
    assert!(t.contains("pubfndisassemble(") && t.contains("pubfnassemble("));
    assert!(!t.contains("pubfnencode("), "编码器不在");
    assert!(!t.contains("pubfndecode("), "解码器不在");
}

// ── 3. include / override 的合并结果 ──

/// 多文件谱展开出的模块里两条指令都在（`IADD` 来自片段、`ISUB` 来自根），
/// 且根的 `[[override]]` 生效（`meta.version` 取根文件的值）。
#[test]
fn included_files_are_merged_into_one_module() {
    let t = expand(Parts::all());
    assert!(t.contains("Iadd"), "片段里的 IADD 应在");
    assert!(t.contains("Isub"), "根文件里的 ISUB 应在");
    assert!(t.contains("18.0-include"), "[[override]] 应覆盖版本");
    // 根文件的 `[meta].name` 覆盖片段的（片段没有 name，故唯一值来自根）。
    assert!(t.contains("demo_include_v12"), "名字来自根文件");
}

// ── 4. 参数校验 ──

/// 部件受限 + 生成期自测 = 矛盾：明确报错，而不是生成一份编译不过的自测。
#[test]
fn restricted_parts_reject_spec_tests() {
    let mut o = opts(Parts {
        encode: true,
        decode: false,
        asm: false,
        tm: false,
    });
    o.spec_tests = true;
    let e = match expand_file(SPEC, &o) {
        Ok(_) => panic!("部件受限 + 自测必须报错"),
        Err(e) => e,
    };
    assert!(e.contains("不能生成生成期自测"), "{e}");
    assert!(e.contains("parts = [encode]"), "错误里给出当前部件：{e}");
    assert!(e.contains("spec_tests = false"), "给出修法：{e}");
    // 去掉 parts（或关掉自测）即恢复。
    assert!(expand_file(SPEC, &opts(Parts::all())).is_ok());
}

#[test]
fn unknown_part_name_is_rejected() {
    let e = Parts::from_names(&["encoder".to_string()]).unwrap_err();
    assert!(e.contains("未知部件 `encoder`"), "{e}");
}
