//! `dfg` 私有化切片的守卫：**arena 只能经受限 API 访问**（v3 S5）。
//!
//! `DataFlowGraph` 的三个 arena 此前全 `pub`，下游可以绕过 use-lists 直改操作数
//! （S0 诊断里的"use-def 可被绕过"）。按 arena 分切片，每片一次提交内完成迁移：
//!
//! ## 切片 1：`values`（已私有）
//!
//! - 读：`value_data`（fail-closed，句柄不合法即 panic，与 `BlockData::terminator`
//!   同契约）/ `value_data_opt`（容忍坏 IR）/ `value_def` / `value_type` /
//!   `values()` / `value_count`；
//! - 写：`set_value_type`（pass 精化类型）——arena 内其余写入（创建/墓碑化）只在
//!   `dfg.rs` 内。
//!
//! 迁移面实测：全仓 31 处 `dfg.values[..]` + 7 处 `dfg.values.get(..)`，其中
//! 3 处写（`const_fold` 定宽、`gvn` 折叠类型、`algebraic` 结果类型）。
//!
//! ## 切片 2：`insts`（已私有）
//!
//! - 读：`inst_data`（fail-closed）/ `inst_data_opt` / `inst_opcode` /
//!   `inst_operands` / `inst_results` / `inst_block` / `insts()` / `inst_count`；
//! - 写：`inst_mut` / `inst_mut_opt`（`(dfg, Inst)` 寻址的就地编辑口）；
//!   结构性增删只有 `make_inst*` / `remove_inst`（`dfg.rs` 内）。
//!
//! 迁移面实测：全仓 216 处 `dfg.insts[..]` + 8 处裸 `dfg.insts[..]` + 11 处
//! `dfg.insts.get(..)` + 3 处 `get_mut(..)` + 1 处 `iter()` + 25 处 `&mut …`,
//! 共 39 个文件。**契约**：`inst_mut` 是就地编辑口，改 `operands`/`results` 后
//! 必须 `Function::refresh_inst_uses`（改操作数的推荐路径仍是
//! `replace_all_uses`/`apply_replacements`）；本文件把这条契约钉住。
//!
//! ## 切片 3：`blocks`（已私有）
//!
//! - 读：`block`（fail-closed）/ `block_opt`（容忍坏 IR）/ `block_data_iter` /
//!   `blocks()` / `block_count` / `block_params` / `block_param_values` /
//!   `block_terminator` / `block_inst_iter`；
//! - 写：`block_mut`（就地编辑口）——`inst_order` 是块内指令顺序的唯一事实源，
//!   只在"插入/搬移"实现里改；`params`/`param_values` 的改动必须与
//!   `Function::{add_block_param, remove_block_param}` 口径一致；结构性增删只有
//!   `make_block*` / `remove_block`。
//!
//! 迁移面实测：75 处 `dfg.blocks[..]` + 1 处裸 `dfg.blocks[..]` + 40 处 `len()` +
//! 23 处 `iter()` + 8 处 `&mut …` + 1 处 `get(..)`，另含 `benches/` 1 处。
//!
//! 三条源码断言（`dfg.values` / `dfg.insts` / `dfg.blocks` 字段零访问）覆盖边界：`dfg.rs`
//! 自身实现、`self.values`（别的结构体字段）、`dfg.values()`/`dfg.insts()`
//! 迭代访问器、注释行均不算。三个 arena 至此全部收口。

use forge_ir::ir::builder::FunctionBuilder;
use forge_ir::ir::types::{FunctionSignature, TypeContext};
use forge_ir::{CallConv, Function, ImmStr, Immediate, Inst, InstFlags, Opcode, TypeId, Value};

/// 造一个含两个 i64 常量 + 一条 Iadd 的函数（值句柄齐全，便于越界对照）。
fn fixture() -> (Function, Value, Value) {
    let ctx = TypeContext::new();
    let sig = FunctionSignature::new(&[], &[ctx.i64_ty()]);
    let mut fb = FunctionBuilder::new("f", ctx, sig);
    let (entry, _) = fb.create_entry_block();
    let a = fb.iconst_i64(1);
    let b = fb.iconst_i64(2);
    let sum = fb.iadd(a, b);
    fb.ret(&[sum]);
    let _ = entry;
    (fb.finish().expect("build"), a, sum)
}

/// 读口：`value_data` fail-closed（越界句柄 panic，不静默返回默认值）。
#[test]
#[should_panic(expected = "值句柄不合法")]
fn value_data_panics_on_invalid_handle() {
    let (func, _a, _sum) = fixture();
    let bogus = Value::new(func.dfg.value_count() as u32 + 7);
    let _ = func.dfg.value_data(bogus);
}

/// 读口：`value_data_opt` 容忍坏 IR（越界 → `None`，不 panic）。
#[test]
fn value_data_opt_tolerates_invalid_handle() {
    let (func, a, _sum) = fixture();
    assert!(func.dfg.value_data_opt(a).is_some());
    assert_eq!(func.dfg.value_data_opt(a).map(|d| d.ty), Some(TypeId::I64));

    let bogus = Value::new(func.dfg.value_count() as u32 + 7);
    assert!(func.dfg.value_data_opt(bogus).is_none());
    // 与既有 Option 读口口径一致
    assert!(func.dfg.value_type(bogus).is_none());
    assert!(func.dfg.value_def(bogus).is_none());
}

/// 写口：`set_value_type` 是值 arena 的唯一外部写入口，越界不写也不 panic。
#[test]
fn set_value_type_is_the_only_write_entry() {
    let (mut func, a, _sum) = fixture();
    assert_eq!(func.dfg.value_type(a), Some(TypeId::I64));

    assert!(func.dfg.set_value_type(a, TypeId::I32), "合法句柄应写入");
    assert_eq!(func.dfg.value_type(a), Some(TypeId::I32));

    let bogus = Value::new(func.dfg.value_count() as u32 + 3);
    assert!(!func.dfg.set_value_type(bogus, TypeId::I64), "越界不写");
}

/// 读口与迭代口口径一致（`values()` / `value_count` / `value_data` 同一事实源）。
#[test]
fn arena_readers_agree() {
    let (func, _a, _sum) = fixture();
    let by_iter: Vec<(Value, TypeId)> = func.dfg.values().map(|(v, d)| (v, d.ty)).collect();
    assert_eq!(by_iter.len(), func.dfg.value_count());
    for (v, ty) in by_iter {
        assert_eq!(func.dfg.value_data(v).ty, ty, "迭代口与点查口必须同源");
    }
}

/// 源码断言：`src/` 里不得再出现 `dfg.values` 字段访问（只许走受限 API）。
#[test]
fn no_direct_values_arena_access_in_src() {
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
                let body = text.split("#[cfg(test)]").next().unwrap_or(&text);
                for (i, line) in body.lines().enumerate() {
                    let t = line.trim();
                    if t.starts_with("//") {
                        continue;
                    }
                    // `dfg.values` / `dfg.insts` 等对 arena **字段**的访问；
                    // 迭代访问器 `dfg.values()`/`dfg.insts()` 不算（括号紧跟其后）。
                    // 本文件内的 `self.values`/`self.insts`（dfg.rs 实现体、
                    // display 的另一个结构体字段）也不在其列。
                    let rest = t;
                    let mut offending = false;
                    for field in ["dfg.values", "dfg.insts", "dfg.blocks"] {
                        let mut r = rest;
                        while let Some(pos) = r.find(field) {
                            let after = &r[pos + field.len()..];
                            // `(` = 迭代/方法访问器；`_` = 更长的方法名
                            // （如 `insts_iter_mut`）——都不是字段访问
                            if !after.starts_with('(') && !after.starts_with('_') {
                                offending = true;
                            }
                            r = after;
                        }
                    }
                    if offending {
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
        "arena 只能经受限 API 访问（values: value_data/value_data_opt/value_def/\
         value_type/values()/set_value_type；insts: inst_data/inst_data_opt/\
         inst_opcode/inst_operands/inst_results/insts()/inst_mut/inst_mut_opt；\
         blocks: block/block_opt/block_data_iter/block_count/block_mut）：\n{}",
        hits.iter()
            .map(|(f, l, t)| format!("{f}:{l}: {t}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 指令 arena 读口：`inst_data` fail-closed、`inst_data_opt` 容忍坏 IR。
#[test]
fn inst_data_is_fail_closed_and_opt_tolerates() {
    let (func, _a, _sum) = fixture();
    let iadd = first_inst(&func, Opcode::Iadd);
    assert_eq!(func.dfg.inst_data(iadd).opcode, Opcode::Iadd);
    assert!(func.dfg.inst_data_opt(iadd).is_some());

    let bogus = Inst::new(func.dfg.inst_count() as u32 + 9);
    assert!(func.dfg.inst_data_opt(bogus).is_none());
    assert_eq!(
        func.dfg.inst_count(),
        func.dfg.insts().count(),
        "迭代口同源"
    );
}

/// `inst_data` 越界 panic（fail-closed 契约）。
#[test]
#[should_panic(expected = "指令句柄不合法")]
fn inst_data_panics_on_invalid_handle() {
    let (func, _a, _sum) = fixture();
    let bogus = Inst::new(func.dfg.inst_count() as u32 + 9);
    let _ = func.dfg.inst_data(bogus);
}

/// 指令写口：`inst_mut` 就地改非 use-list 字段，句柄不合法 panic / `opt` 给 `None`。
#[test]
fn inst_mut_is_the_only_edit_entry() {
    let (mut func, _a, _sum) = fixture();
    let iadd = first_inst(&func, Opcode::Iadd);

    func.dfg.inst_mut(iadd).flags = InstFlags::MAY_UB;
    assert!(func.dfg.inst_data(iadd).flags.contains(InstFlags::MAY_UB));

    let bogus = Inst::new(func.dfg.inst_count() as u32 + 9);
    assert!(func.dfg.inst_mut_opt(bogus).is_none());
}

/// 契约：`inst_mut` 改 `operands` 后必须 `refresh_inst_uses`，use-def 才新鲜。
#[test]
fn inst_mut_operand_edit_needs_refresh() {
    let ctx = TypeContext::new();
    let sig_ref = ctx.register_signature(FunctionSignature::new(&[], &[TypeId::I32]));
    let mut func = Function::new("h", ctx.clone(), sig_ref, CallConv::Default);
    let (b0, _) = func.dfg.make_block_with_params(&[]);
    func.entry_block = Some(b0);
    let a = func.dfg.make_inst(
        Opcode::Iconst,
        b0,
        smallvec::SmallVec::new(),
        smallvec::smallvec![Immediate::Const(func.constants.insert_int(1, 32))],
        &[TypeId::I32],
        InstFlags::NONE,
    );
    let b = func.dfg.make_inst(
        Opcode::Iconst,
        b0,
        smallvec::SmallVec::new(),
        smallvec::smallvec![Immediate::Const(func.constants.insert_int(2, 32))],
        &[TypeId::I32],
        InstFlags::NONE,
    );
    let va = func.dfg.inst_results(a)[0];
    let vb = func.dfg.inst_results(b)[0];
    let add = func.dfg.make_inst(
        Opcode::Iadd,
        b0,
        smallvec::smallvec![va, va],
        smallvec::SmallVec::new(),
        &[TypeId::I32],
        InstFlags::NONE,
    );
    func.ret(b0, [func.dfg.inst_results(add)[0]]);
    func.refresh_inst_uses(add);

    assert_eq!(func.use_lists.use_count(va), 2, "初始两条 use");
    assert_eq!(func.use_lists.use_count(vb), 0);

    // 就地改写第二条操作数 + 重登记（`inst_mut` 的既有用法）
    func.dfg.inst_mut(add).operands[1] = vb;
    func.refresh_inst_uses(add);
    assert_eq!(func.use_lists.use_count(va), 1, "重登记后旧 use 应消失");
    assert_eq!(func.use_lists.use_count(vb), 1, "新 use 应登记");

    let mut verifier = forge_ir::verify::Verifier::with_ctx(ctx);
    let outcome = verifier.verify(&func);
    assert!(
        outcome.is_ok(),
        "就地改写 + 重登记后 IR 应有效：{outcome:?}"
    );
}

/// 在 fixture 里按 opcode 找第一条指令。
fn first_inst(func: &Function, opcode: Opcode) -> Inst {
    func.dfg
        .insts()
        .find(|(_, i)| i.opcode == opcode)
        .unwrap_or_else(|| panic!("找不到 {opcode:?} 指令"))
        .0
}

/// 受限写入口在真实语义下可用：精化值类型后 verifier 仍接受。
#[test]
fn refined_value_type_stays_verifiable() {
    let ctx = TypeContext::new();
    let sig_ref = ctx.register_signature(FunctionSignature::new(&[], &[TypeId::I32]));
    let mut func = Function::new("g", ctx.clone(), sig_ref, CallConv::Default);
    let (b0, _) = func.dfg.make_block_with_params(&[]);
    func.entry_block = Some(b0);
    let inst: Inst = func.dfg.make_inst(
        Opcode::Iconst,
        b0,
        smallvec::SmallVec::new(),
        smallvec::smallvec![Immediate::Const(func.constants.insert_int(7, 64))],
        &[TypeId::I64],
        InstFlags::NONE,
    );
    let v = func.dfg.inst_results(inst)[0];
    // 折叠：把 i64 常量值的类型精化为 i32（受限写入口）
    assert!(func.dfg.set_value_type(v, TypeId::I32));
    func.ret(b0, [v]);

    let mut verifier = forge_ir::verify::Verifier::with_ctx(ctx);
    let outcome = verifier.verify(&func);
    assert!(outcome.is_ok(), "精化值类型后 IR 仍应通过校验：{outcome:?}");
    let _ = ImmStr::from("unused-import-guard");
}

/// 块 arena 读口：`block` fail-closed、`block_opt` 容忍坏 IR、迭代口同源。
#[test]
fn block_accessors_are_fail_closed_and_agree() {
    let (func, _a, _sum) = fixture();
    let entry = func.entry_block.expect("entry");
    assert!(
        !func.dfg.block(entry).inst_order.is_empty(),
        "入口块应有指令"
    );

    assert_eq!(
        func.dfg.block_data_iter().count(),
        func.dfg.block_count(),
        "无句柄迭代口与计数同源"
    );
    assert_eq!(func.dfg.blocks().count(), func.dfg.block_count());

    let bogus = forge_ir::Block::new(func.dfg.block_count() as u32 + 5);
    assert!(func.dfg.block_opt(bogus).is_none());
}

/// `block` 越界 panic（fail-closed 契约）。
#[test]
#[should_panic(expected = "块句柄不合法")]
fn block_panics_on_invalid_handle() {
    let (func, _a, _sum) = fixture();
    let bogus = forge_ir::Block::new(func.dfg.block_count() as u32 + 5);
    let _ = func.dfg.block(bogus);
}

/// 块写口：`block_mut` 就地编辑（块名/顺序表），越界 panic。
#[test]
fn block_mut_is_the_only_edit_entry() {
    let (mut func, _a, _sum) = fixture();
    let entry = func.entry_block.expect("entry");
    let before = func.dfg.block(entry).inst_order.len();
    assert!(before > 0);

    // 就地清空块内顺序表（结构性口径由 make_inst*/remove_block 维护；此处只验证写口可用）
    func.dfg.block_mut(entry).inst_order.clear();
    assert_eq!(func.dfg.block(entry).inst_order.len(), 0);

    let bogus = forge_ir::Block::new(func.dfg.block_count() as u32 + 5);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        func.dfg.block_mut(bogus);
    }));
    assert!(panicked.is_err(), "越界句柄必须 fail-closed");
}
