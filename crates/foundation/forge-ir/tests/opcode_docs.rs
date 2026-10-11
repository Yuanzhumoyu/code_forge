//! IR 操作码文档的**运行期守卫**（2026-10-08）。
//!
//! `ops.toml` 的 `doc` 是操作码语义/UB/副作用的**单一事实源**：`build.rs` 把它投影成
//! `Opcode` 变体的 `#[doc]` 与 [`Opcode::INFOS`] 的 `doc` 字段，并额外生成一行
//! **自动属性行**（操作数/结果/UB/副作用/条件通道/类型规则/LLVM 名）。
//!
//! 构建期已经断言过门槛（长度、`may_ub` 必提 UB）；这里从**运行期投影**再守一遍——
//! 投影链路断了（例如生成器改了字段）会在这里红，而不是等读者在 rustdoc 里发现空白。

use forge_ir::ir::opcode::Opcode;

#[test]
fn every_opcode_doc_is_informative() {
    assert!(
        Opcode::INFOS.len() >= 100,
        "INFOS 条数异常（{}）——ops.toml 或生成器出问题",
        Opcode::INFOS.len()
    );
    for info in Opcode::INFOS.iter() {
        let d = info.doc.trim();
        assert!(
            d.chars().count() >= 24,
            "`{}` 的 doc 太短（{} 字符）：要写清语义 + 操作数/结果 + UB/副作用",
            info.name,
            d.chars().count()
        );
        assert!(
            !d.contains("TODO") && !d.contains("待补"),
            "`{}` 的 doc 里留有占位文字：{d}",
            info.name
        );
    }
}

#[test]
fn may_ub_opcodes_say_so_in_their_docs() {
    let mut flagged = 0;
    for info in Opcode::INFOS.iter() {
        if !info.may_ub {
            continue;
        }
        flagged += 1;
        let d = info.doc;
        assert!(
            d.contains("UB") || d.contains("未定义行为"),
            "`{}` 声明 may_ub = true，但 doc 没提 UB：{d}",
            info.name
        );
    }
    assert!(flagged >= 10, "may_ub 条目太少（{flagged}）——检查 ops.toml 是否被改坏");
}

#[test]
fn side_effecting_opcodes_document_their_effect() {
    for info in Opcode::INFOS.iter() {
        if !info.side_effect {
            continue;
        }
        let d = info.doc;
        assert!(
            d.contains("副作用") || d.contains("DCE"),
            "`{}` 声明 side_effect = true，但 doc 没说清副作用/DCE 语义：{d}",
            info.name
        );
    }
}
