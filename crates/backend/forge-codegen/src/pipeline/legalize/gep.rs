//! 合法化 pass：**GEP 形状展开 / 常量折叠**。
//!
//! 从 `compiler.rs` 迁出（v23 M6 批次 2）。

use super::common::make_placeholder_ptr;
use crate::IrError;
use forge_ir::*;

/// GEP 展开（全 ISA 通用，编译期）：`getelementptr T, ptr %p, i32 i0, i32 i1...`
/// 逐级计算偏移——常量索引折叠（第一索引作用于 indexed_ty 自身；struct 用
/// field_offset；数组用 elem_size×idx），动态索引（数组下标）展开为
/// `mul i64 %idx, size` + `add`。GEP 指令原地改为 Copy（result 保留——
/// SSA 引用一致），展开的 mul/add 插在 GEP 位置之前。展开后不依赖机器
/// GEP 规则（旧 `lea [rs1+rs2*4+0]` 单索引规则已废弃）。
/// 局限：struct 索引必须常量（LLVM 语义）；索引按无符号处理（数组下标）。
pub(crate) fn expand_geps(func: &mut Function) -> Result<(), IrError> {
    // 取一次读锁（v3 S3 读路径纪律）：本函数只读类型，内部不再逐次取锁。
    // 先从 func.types clone 出 TypeContext（Arc，O(1)）再取锁——守卫借的是这个局部，
    // 不与 func 的 &mut 借用冲突。
    let types_ctx = func.types.clone();
    let store = types_ctx.borrow();
    let mut jobs: Vec<(Block, usize, Inst)> = Vec::new(); // (block, pos, inst)
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            if func.dfg.inst_data(ii).opcode == Opcode::GetElementPtr {
                jobs.push((b, pos, ii));
            }
        }
    }
    for (b, pos, ii) in jobs.into_iter().rev() {
        // 提前 copy 出 GEP 数据（inst 借用须在 &mut func 生成段前结束）
        let (indexed_ty, base, indices) = {
            let inst = &func.dfg.inst_data(ii);
            (
                inst.immediates
                    .iter()
                    .find_map(|i| match i {
                        Immediate::Type(t) => Some(*t),
                        _ => None,
                    })
                    .unwrap_or(TypeId::PTR),
                inst.operands.first().copied(),
                inst.operands.iter().skip(1).copied().collect::<Vec<_>>(),
            )
        };
        let base = base.unwrap_or_else(|| make_placeholder_ptr(func));
        // 索引常量折叠（Iconst 值 → i64）
        let fold = |func: &Function, v: Value| -> Option<i64> {
            let def = func.dfg.value_data(v).def;
            let ValueDef::Inst(ci, _) = def else {
                return None;
            };
            let id = func.dfg.inst_data_opt(ci)?;
            if id.opcode != Opcode::Iconst {
                return None;
            }
            let im = id.immediates.first()?;
            match im {
                Immediate::Uint(v) => Some(*v as i64),
                Immediate::Int(v) => Some(*v),
                Immediate::Const(c) => func.constants.bits(*c).map(|bv| bv.bits as i64),
                _ => None,
            }
        };
        let mut cur_ty = indexed_ty;
        let mut const_off: i64 = 0;
        let mut dyn_parts: Vec<(Value, u32)> = Vec::new(); // (idx 值, 元素字节)
        for (i, &idx) in indices.iter().enumerate() {
            let i = i + 1; // 原操作数下标（operands[0]=base）
            let ts = &store;
            if i == 1 {
                // 第一索引作用于 indexed_ty 自身（T* → T 数组）
                let size = ts.size_bytes(cur_ty) as i64;
                match fold(func, idx) {
                    Some(c) => const_off += c * size,
                    None => dyn_parts.push((idx, size as u32)),
                }
                continue;
            }
            if let TypeEntry::Struct { .. } = ts.get(cur_ty) {
                let c = fold(func, idx).ok_or_else(|| {
                    IrError::Internal("GEP struct 索引必须为常量（LLVM 语义）".into())
                })?;
                let off = ts.field_offset(cur_ty, c as u32).ok_or_else(|| {
                    IrError::Internal(format!("GEP struct 索引越界：{c}（type {cur_ty:?}）"))
                })?;
                const_off += off as i64;
                cur_ty = ts
                    .aggregate_elem_type(cur_ty, c as u32)
                    .ok_or_else(|| IrError::Internal("GEP 字段类型缺失".into()))?;
            } else {
                let elem_ty = ts
                    .element_type(cur_ty)
                    .ok_or_else(|| IrError::Internal("GEP 索引作用于非数组/结构体类型".into()))?;
                let elem_size = ts.size_bytes(elem_ty) as i64;
                match fold(func, idx) {
                    Some(c) => const_off += c * elem_size,
                    None => dyn_parts.push((idx, elem_size as u32)),
                }
                cur_ty = elem_ty;
            }
        }
        // 生成展开序列：动态部分 mul+add，最后常量偏移 add（const_off!=0 或全零时
        // 仍需 add 保持链尾——全零无动态时直接 Copy base）
        let mut prev = base;
        let mut new_count = 0usize;
        for (idx_v, size) in dyn_parts {
            // %t = mul i64 %idx, size
            let size_cid = func.constants.intern_int(size as i128, 64);
            let size_inst = func.make_inst(
                Opcode::Iconst,
                b,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(size_cid)],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let sval = func.dfg.inst_data(size_inst).results[0];
            let mul = func.make_inst(
                Opcode::Imul,
                b,
                smallvec::smallvec![idx_v, sval],
                smallvec::smallvec![],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let mval = func.dfg.inst_data(mul).results[0];
            // %p = add i64 %prev, %t（结果类型 PTR——内部指针算术，verify 不跑）
            let add = func.make_inst(
                Opcode::Iadd,
                b,
                smallvec::smallvec![prev, mval],
                smallvec::smallvec![],
                &[TypeId::PTR],
                InstFlags::NONE,
            );
            prev = func.dfg.inst_data(add).results[0];
            new_count += 3;
        }
        if const_off != 0 || new_count > 0 {
            let off_cid = func.constants.intern_int(const_off as i128, 64);
            let off_inst = func.make_inst(
                Opcode::Iconst,
                b,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(off_cid)],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let off_v = func.dfg.inst_data(off_inst).results[0];
            let add = func.make_inst(
                Opcode::Iadd,
                b,
                smallvec::smallvec![prev, off_v],
                smallvec::smallvec![],
                &[TypeId::PTR],
                InstFlags::NONE,
            );
            prev = func.dfg.inst_data(add).results[0];
            new_count += 2;
        }
        // GEP 原地改 Copy（result 保留——SSA 引用一致）
        let __cp = ii;
        {
            let inst = func.dfg.inst_mut(__cp);
            inst.opcode = Opcode::Copy;
            inst.operands = smallvec::smallvec![prev];
            inst.immediates = smallvec::smallvec![];
        }
        func.refresh_inst_uses(__cp);
        // 新指令（块尾）移到 GEP 位置前（逆序处理 jobs：尾即本 GEP 的新指令）
        let mut new_insts: Vec<Inst> = Vec::with_capacity(new_count);
        for _ in 0..new_count {
            let order = &mut func.dfg.block_mut(b).inst_order;
            new_insts.push(order.pop().unwrap());
        }
        new_insts.reverse();
        for (k, &ni) in new_insts.iter().enumerate() {
            func.dfg.block_mut(b).inst_order.insert(pos + k, ni);
        }
    }
    Ok(())
}
