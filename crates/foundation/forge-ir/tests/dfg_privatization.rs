//! `dfg` 私有化切片的守卫：**值 arena（`values`）只能经受限 API 访问**（v3 S5）。
//!
//! `DataFlowGraph` 的三个 arena 此前全 `pub`，下游可以绕过 use-lists 直改操作数
//! （S0 诊断里的"use-def 可被绕过"）。本切片先收 **`values`**：
//!
//! - 读：`value_data`（fail-closed，句柄不合法即 panic，与 `BlockData::terminator`
//!   同契约）/ `value_data_opt`（容忍坏 IR）/ `value_def` / `value_type` /
//!   `values()` / `value_count`；
//! - 写：`set_value_type`（pass 精化类型）——arena 内其余写入（创建/墓碑化）只在
//!   `dfg.rs` 内。
//!
//! 迁移面实测：全仓 31 处 `dfg.values[..]` + 7 处 `dfg.values.get(..)`，其中
//! 3 处写（`const_fold` 定宽、`gvn` 折叠类型、`algebraic` 结果类型）——全部改走
//! 上表入口后，`dfg.values` 在 `src/` 里应为 0 处（源码断言）。
//! `insts`/`blocks` 两个 arena 的私有化是后续切片。

use forge_ir::builder::FunctionBuilder;
use forge_ir::types::{FunctionSignature, TypeContext};
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
    let bogus = Value(func.dfg.value_count() as u32 + 7);
    let _ = func.dfg.value_data(bogus);
}

/// 读口：`value_data_opt` 容忍坏 IR（越界 → `None`，不 panic）。
#[test]
fn value_data_opt_tolerates_invalid_handle() {
    let (func, a, _sum) = fixture();
    assert!(func.dfg.value_data_opt(a).is_some());
    assert_eq!(func.dfg.value_data_opt(a).map(|d| d.ty), Some(TypeId::I64));

    let bogus = Value(func.dfg.value_count() as u32 + 7);
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

    let bogus = Value(func.dfg.value_count() as u32 + 3);
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
                    // `dfg.values` / `func.dfg.values` 等对 arena **字段**的访问；
                    // 迭代访问器 `dfg.values()` 不算（括号紧跟其后）。
                    // 本文件内的 `self.values`（dfg.rs 实现体、display 的另一个
                    // 结构体字段）也不在其列。
                    let mut rest = t;
                    let mut offending = false;
                    while let Some(pos) = rest.find("dfg.values") {
                        let after = &rest[pos + "dfg.values".len()..];
                        if !after.starts_with('(') {
                            offending = true;
                        }
                        rest = after;
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
        "值 arena 只能经受限 API 访问（value_data / value_data_opt / value_def / \
         value_type / values() / set_value_type）：\n{}",
        hits.iter()
            .map(|(f, l, t)| format!("{f}:{l}: {t}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
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
