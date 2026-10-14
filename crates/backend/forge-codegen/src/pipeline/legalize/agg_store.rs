//! 合法化 pass：**聚合字面量 store 展开**（→ 打包 i64 常量 store）。
//!
//! 从 `compiler.rs` 迁出（v23 M6 批次 2）。

use crate::IrError;
use forge_ir::*;

/// 聚合常量打包：聚合树 → 小端 u64（≤8 字节聚合；字段按内存布局偏移排列）。
/// 聚合常量 → 内存字节（小端布局：标量按 field_offset 写入 LE 字节，
/// 未覆盖区零——对齐填充/空洞）。任意大小（>8 字节聚合分段 store 用）。
/// 聚合 store 展开（全 ISA 通用，编译期）：聚合常量 → 内存字节（小端），
/// 按 8 字节段展开为普通标量 store——`store {i32,i32} {i32 7, i32 9}, ptr %p`
/// → `store i64 <7|9<<32>, ptr %p`；>8 字节（如 {i64,i64} 16 字节）→ 逐段
/// `store i64 <seg>, ptr <addr>` + `add ptr, 8` 地址推进（尾段按 8/4/2/1
/// 字节宽 store，不越界）。展开使 store 源成为普通 Iconst 值（修复原 SEGV：
/// 聚合常量从不加载进寄存器 + 聚合类型 bits()=0 → store 宽度 0）。
/// - 聚合 load 结果 >8 字节与聚合常量传参 >8 字节：Phase 2 处理（此处保留
///   Unsupported——大聚合值内存化传播）。
/// - 仅小端数据布局（x86_64/aarch64 默认）；大端需反向打包（暂不支持）。
pub(crate) fn expand_agg_stores(func: &mut Function) -> Result<(), IrError> {
    // 取一次读锁（v3 S3 读路径纪律）：本函数只读类型，内部不再逐次取锁。
    // 先从 func.types clone 出 TypeContext（Arc，O(1)）再取锁——守卫借的是这个局部，
    // 不与 func 的 &mut 借用冲突。
    let types_ctx = func.types.clone();
    let store = types_ctx.borrow();
    // 第一遍：收集（聚合常量 → 字节布局）
    let mut jobs: Vec<(Block, usize, usize, Vec<u8>, bool)> = Vec::new(); // (block, pos, operand 索引, 内存字节, 是否 Store)
    for (b, bd) in func.dfg.blocks() {
        for (pos, &ii) in bd.inst_order.iter().enumerate() {
            let inst = &func.dfg.inst_data(ii);
            match inst.opcode {
                Opcode::Store => {
                    if let Some(v) = inst.operands.first()
                        && let ValueDef::AggConst(aid) = func.dfg.value_data(*v).def
                    {
                        let agg = func.constants.get_aggregate(aid).ok_or_else(|| {
                            IrError::Unsupported("聚合常量 store：常量池缺失 AggId".into())
                        })?;
                        let bytes = crate::pipeline::agg_const::pack_agg_bytes(func, agg)?;
                        jobs.push((b, pos, 0, bytes, true));
                    }
                }
                // 大聚合 load 由 expand_large_aggs 处理（值传播展开）——此处不拦截
                // Call/CallIndirect 的聚合常量参数：与 Store 同理打包展开（否则
                // AggConst 值从不加载——传参 0/垃圾）。builder.call 的 operands
                // 全是实参（callee 在 Immediate::Func）；CallIndirect 的
                // operands[0] 是 callee 指针（skip 1）。
                Opcode::Call | Opcode::CallIndirect => {
                    let skip = usize::from(inst.opcode == Opcode::CallIndirect);
                    for (op_idx, v) in inst.operands.iter().enumerate().skip(skip) {
                        if let ValueDef::AggConst(aid) = func.dfg.value_data(*v).def {
                            let agg = func.constants.get_aggregate(aid).ok_or_else(|| {
                                IrError::Unsupported("聚合常量参数：常量池缺失 AggId".into())
                            })?;
                            let size = store.size_bytes(agg.ty);
                            if size > 8 {
                                // >8 字节由 expand_agg_call_args 拆段处理——跳过
                                continue;
                            }
                            let bytes = crate::pipeline::agg_const::pack_agg_bytes(func, agg)?;
                            jobs.push((b, pos, op_idx, bytes, false));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // 逆序替换：inst_order.insert(pos) 移动后续下标——从后往前插入不受影响。
    for (b, pos, op_idx, bytes, is_store) in jobs.into_iter().rev() {
        let store_ii = func.dfg.block(b).inst_order[pos];
        // Store：分段展开（operands[1]=addr）；Call 参数：单打包值替换 operand
        let addr = if is_store {
            let inst = &func.dfg.inst_data(store_ii);
            inst.operands.get(op_idx + 1).copied()
        } else {
            None
        };
        let mut new_insts: Vec<Inst> = Vec::new();
        if let Some(addr) = addr {
            let mut seg_off = 0usize;
            let mut prev_addr = addr;
            while seg_off < bytes.len() {
                let seg_len = (bytes.len() - seg_off).min(8);
                // v23 M6：打包口径单点化（`agg_const::pack_segment`）。
                let seg_val = crate::pipeline::agg_const::pack_segment(&bytes, seg_off, seg_len);
                let store_ty = match seg_len {
                    1 => TypeId::I8,
                    2 => TypeId::I16,
                    4 => TypeId::I32,
                    _ => TypeId::I64,
                };
                // 段常量（复用 make_inst 的 results[0]——不可再 make_value）
                let cid = func
                    .constants
                    .insert_int(seg_val as i128, (seg_len * 8) as u32);
                let iconst = func.make_inst(
                    Opcode::Iconst,
                    b,
                    smallvec::smallvec![],
                    smallvec::smallvec![Immediate::Const(cid)],
                    &[store_ty],
                    InstFlags::NONE,
                );
                let cv = func.dfg.inst_data(iconst).results[0];
                new_insts.push(iconst);
                let st = func.make_inst(
                    Opcode::Store,
                    b,
                    smallvec::smallvec![cv, prev_addr],
                    smallvec::smallvec![],
                    &[],
                    InstFlags::SIDE_EFFECT,
                );
                new_insts.push(st);
                seg_off += seg_len;
                if seg_off < bytes.len() {
                    // prev_addr = add ptr prev_addr, seg_len（非 GEP——expand_geps 不碰）
                    let off_cid = func.constants.insert_int(seg_len as i128, 64);
                    let off_inst = func.make_inst(
                        Opcode::Iconst,
                        b,
                        smallvec::smallvec![],
                        smallvec::smallvec![Immediate::Const(off_cid)],
                        &[TypeId::I64],
                        InstFlags::NONE,
                    );
                    let off_v = func.dfg.inst_data(off_inst).results[0];
                    new_insts.push(off_inst);
                    let add = func.make_inst(
                        Opcode::Iadd,
                        b,
                        smallvec::smallvec![prev_addr, off_v],
                        smallvec::smallvec![],
                        &[TypeId::PTR],
                        InstFlags::NONE,
                    );
                    prev_addr = func.dfg.inst_data(add).results[0];
                    new_insts.push(add);
                }
            }
        }
        if !is_store {
            // Call 参数（≤8 字节）：打包为单 i64 常量替换 operand
            // v23 M6：打包口径单点化（`agg_const::pack_segment`）。
            let packed = crate::pipeline::agg_const::pack_segment(&bytes, 0, bytes.len().min(8));
            let cid = func.constants.insert_int(packed as i128, 64);
            let iconst = func.make_inst(
                Opcode::Iconst,
                b,
                smallvec::smallvec![],
                smallvec::smallvec![Immediate::Const(cid)],
                &[TypeId::I64],
                InstFlags::NONE,
            );
            let cv = func.dfg.inst_data(iconst).results[0];
            {
                let inst = func.dfg.inst_mut(store_ii);
                inst.operands[op_idx] = cv;
            }
            // 就地改单个操作数也要重登记 use-lists
            func.refresh_inst_uses(store_ii);
            new_insts.push(iconst);
        } else {
            // 原 Store 标 Nop（无结果——不生成机器指令；lowering 跳过 Nop）
            let __tomb = store_ii;
            func.tombstone_inst(__tomb);
        }
        // 新指令（块尾）移到 Store 位置前（逆序处理 jobs：尾即本 Store 的新指令）
        let mut moved: Vec<Inst> = Vec::with_capacity(new_insts.len());
        for _ in 0..new_insts.len() {
            let order = &mut func.dfg.block_mut(b).inst_order;
            moved.push(order.pop().unwrap());
        }
        moved.reverse();
        for (k, &ni) in moved.iter().enumerate() {
            func.dfg.block_mut(b).inst_order.insert(pos + k, ni);
        }
    }
    Ok(())
}
