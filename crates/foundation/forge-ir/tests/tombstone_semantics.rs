//! 墓碑语义守卫（v3 方案 S2：墓碑语义显式化）。
//!
//! 删除是"标墓碑"而非回收槽位。此前"是不是墓碑"靠 `opcode == Nop` 猜，而
//! `Opcode::Nop` **是合法指令**（`FunctionBuilder::nop()` 会发一条进 `inst_order`），
//! 于是同一个问题在全仓有两种答案（`Nop` / `Nop && results.is_empty()`），
//! 合法 Nop 会被当成墓碑跳过、而"带 results 的墓碑"（就地墓碑化）又会被当成活指令。
//!
//! 现在唯一事实源是 `Instruction::is_tombstone()`（`tombstone: bool` 字段）：
//!
//! - `Function::kill_inst` / `DataFlowGraph::remove_inst`（**删除语义**）：标墓碑 +
//!   摘掉 `inst_order` 条目 + 清空 operands/immediates/附件 + **清空 results 并把
//!   结果值 VOID 化**（调用方须保证结果已无活跃使用）。
//! - `Function::tombstone_inst`（**就地墓碑化**）：标墓碑 + 清空
//!   operands/immediates/附件 + use-lists 重登记，但**保留 `inst_order` 条目与
//!   `results`**——大聚合展开等调用方之后仍要读那条被作废指令的结果值。

use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::dfg::ValueDef;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::{
    CallConvId, Function, ImmStr, Immediate, Inst, InstFlags, IselStrategy, Opcode, TypeId,
};

/// 造一个 `ret(load(store(...)))` 形态的函数：一条有结果的指令 + 一条 Store。
fn fixture() -> (Function, Inst, Inst) {
    let ctx = TypeContext::new();
    let sig_ref = ctx.register_signature(FunctionSignature::new(&[], &[TypeId::I32]));
    let mut func = Function::new("f", ctx, sig_ref, CallConvId::default());
    let (b0, _) = func.dfg.make_block_with_params(&[]);
    func.entry_block = Some(b0);
    let c = func.dfg.make_inst(
        Opcode::Iconst,
        b0,
        smallvec::SmallVec::new(),
        smallvec::smallvec![Immediate::Const(func.constants.insert_int(7, 32))],
        &[TypeId::I32],
        InstFlags::NONE,
    );
    let v = func.dfg.inst_results(c)[0];
    let store = func.dfg.make_inst(
        Opcode::Store,
        b0,
        smallvec::smallvec![v],
        smallvec::SmallVec::new(),
        &[],
        InstFlags::NONE,
    );
    func.ret(b0, [v]);
    (func, c, store)
}

/// 删除语义：`kill_inst` 留下规范墓碑（标志 + 空表 + 结果清空 + 值 VOID + 退出顺序表）。
#[test]
fn kill_inst_leaves_canonical_tombstone() {
    let (mut func, c, _store) = fixture();
    let v = func.dfg.inst_results(c)[0];
    func.kill_inst(c);

    let inst = func.dfg.inst_data(c);
    assert!(inst.is_tombstone(), "标志必须置位");
    assert_eq!(inst.opcode, Opcode::Nop, "opcode 归一为 Nop");
    assert!(inst.operands.is_empty() && inst.immediates.is_empty());
    assert!(inst.results.is_empty(), "删除语义清空结果");
    assert_eq!(func.dfg.value_type(v), Some(TypeId::VOID), "结果值 VOID 化");
    // 顺序表里不再有它
    let entry = func.entry_block.expect("entry");
    assert!(!func.dfg.block(entry).inst_order.contains(&c));
}

/// 合法 `nop` **不是**墓碑（这正是显式标志要解决的歧义）。
#[test]
fn legit_nop_is_not_a_tombstone() {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[]);
    let mut fb = FunctionBuilder::new("g", ctx, sig);
    let (entry, _) = fb.create_entry_block();
    fb.nop();
    fb.ret(&[]);
    let func = fb.finish().expect("build");

    let nop = func
        .dfg
        .block(entry)
        .inst_order
        .iter()
        .copied()
        .find(|&i| func.dfg.inst_data(i).opcode == Opcode::Nop)
        .expect("合法 nop 应在顺序表里");
    assert!(!func.dfg.inst_data(nop).is_tombstone(), "合法 nop 非墓碑");
    // 无句柄迭代口也应看得到它
    assert_eq!(
        func.dfg
            .block_inst_iter(entry)
            .filter(|i| i.opcode == Opcode::Nop)
            .count(),
        1,
        "墓碑过滤不应误伤合法 nop"
    );
}

/// 就地墓碑化：保留 `results`（调用方仍要读），但标志与附件清理照做。
#[test]
fn in_place_tombstone_keeps_results_and_clears_attachments() {
    let (mut func, c, _store) = fixture();
    // 给将被作废的指令挂满附件
    func.dfg
        .inst_mut(c)
        .attach_metadata(forge_ir::ir::metadata::AttachedMetadata {
            kind: forge_ir::ir::metadata::MetadataKind::TBAA,
            node: forge_ir::ir::metadata::MetadataId(0),
        });
    func.dfg
        .inst_mut(c)
        .set_isel_strategy(IselStrategy::from_static("lea_sib:4"));

    func.tombstone_inst(c);

    let inst = func.dfg.inst_data(c);
    assert!(inst.is_tombstone());
    assert_eq!(inst.results.len(), 1, "就地墓碑化保留 results");
    assert!(inst.metadata().is_empty(), "附件必须清掉（S2 一致性）");
    assert!(
        inst.isel_strategy().is_none(),
        "isel_strategy 必须清掉（S2 一致性）"
    );
    // 顺序表条目保留（就地墓碑化的语义）
    let entry = func.entry_block.expect("entry");
    assert!(func.dfg.block(entry).inst_order.contains(&c));
    let _ = ImmStr::from("unused");
    let _ = ValueDef::Param(func.entry_block.expect("entry"), 0);
}

/// 源码断言：`opcode = Opcode::Nop` 只允许出现在 `src/ir/dfg.rs`（墓碑化的唯一实现）。
#[test]
fn no_tombstone_writes_outside_dfg_rs() {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, usize, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let Ok(text) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let rel = p
                    .strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel == "ir/dfg.rs" {
                    continue; // 唯一实现（目录归类后相对 `src/` 的路径）
                }
                let body = text.split("#[cfg(test)]").next().unwrap_or(&text);
                for (i, line) in body.lines().enumerate() {
                    let t = line.trim();
                    if t.starts_with("//") {
                        continue;
                    }
                    if t.contains("opcode = Opcode::Nop") {
                        out.push((rel.clone(), i + 1, t.to_string()));
                    }
                }
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    walk(&root, &root, &mut hits);
    assert!(
        hits.is_empty(),
        "墓碑化只能经 `DataFlowGraph::tombstone_inst_low`（删除走 `remove_inst`，\
         就地走 `Function::tombstone_inst`）：\n{}",
        hits.iter()
            .map(|(f, l, t)| format!("{f}:{l}: {t}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
