//! insert_preheader pass（P1 剩余项）：为缺失 preheader 的循环插入规范
//! preheader 块。
//!
//! `LoopInfo::preheader` 只在 header 的**唯一循环外 pred** 时取到；当多个
//! 循环外 pred（如 if 两边都进循环）直接落入 header 时，LICM 的外提目标
//! 回退到 header（每迭代执行一次的位置）。本 pass 新建一个空块 ph：
//!
//! ```text
//!   pred1 ─┐                 pred1 ─┐
//!          ├─→ header   ⇒          ├─→ ph ─→ header
//!   pred2 ─┘                 pred2 ─┘
//! ```
//!
//! - ph 的参数 = header 参数（pred 原传给 header 的 args 现在传给 ph，
//!   `Terminator::retarget` 保留 args）；
//! - ph 无条件 jump header，转发自己的参数；
//! - ph 是 header 的唯一循环外 pred 且支配 header → 重建后
//!   `LoopInfo::preheader == ph`，LICM 全量走 preheader 外提。
//!
//! 在 O2 管线中置于 LICM 之前（block_param_coalesce 之后）。

use crate::{OptimizationPass, PassResult};
use forge_ir::IrError;
use forge_ir::*;
use std::collections::HashSet;

/// 为缺失 preheader 的循环插入规范 preheader 块。
#[derive(Default)]
pub struct InsertPreheaderPass;

impl InsertPreheaderPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for InsertPreheaderPass {
    fn name(&self) -> &'static str {
        "insert-preheader"
    }

    fn description(&self) -> &'static str {
        "Insert a canonical preheader block for loops whose header has multiple outside predecessors"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, IrError> {
        insert_preheaders(func)
    }
}

/// 对全部缺失 preheader 的循环插入 preheader；返回改动统计。
pub fn insert_preheaders(func: &mut Function) -> Result<PassResult, IrError> {
    let mut result = PassResult::default();

    // 快照：缺失 preheader 且循环外 pred > 1 的循环 (header, 循环外 preds)。
    // loop_forest/predecessors 均为惰性只读分析——在借用结束后执行插入。
    let to_fix: Vec<(Block, Vec<Block>)> = {
        let loops = func.loop_forest();
        let preds_map = func.predecessors().clone();
        let mut v = Vec::new();
        for l in loops.all_loops() {
            if l.preheader.is_some() {
                continue;
            }
            let block_set: HashSet<Block> = l.blocks.iter().copied().collect();
            let outside: Vec<Block> = preds_map
                .get(&l.header)
                .map(|ps| {
                    ps.iter()
                        .copied()
                        .filter(|p| !block_set.contains(p))
                        .collect()
                })
                .unwrap_or_default();
            // 仅多 pred 需要插入（单 pred 已由 LoopInfo 认定为 preheader；
            // 0 pred = entry 即 header，无循环外路径，跳过）
            if outside.len() > 1 {
                v.push((l.header, outside));
            }
        }
        v
    };

    for (header, outside) in to_fix {
        // ph 参数 = header 参数（pred 传给 ph 的 args 与 header 参数同型）
        let param_tys: Vec<TypeId> = func.dfg.blocks[header.0 as usize].params.to_vec();
        let (ph, ph_params) = func.dfg.make_block_with_params(&param_tys);
        // 循环外 pred 的边改指 ph（retarget 保留原 args——现在成为 ph 的参数实参）
        for p in &outside {
            func.dfg.blocks[p.0 as usize]
                .terminator
                .retarget(header, ph);
        }
        // ph → header：转发自身参数
        func.dfg.set_terminator(
            ph,
            Terminator::Jump {
                target: header,
                args: ph_params.into_iter().collect(),
                metadata: smallvec::SmallVec::new(),
            },
        );
        result.instructions_added += 1;
        result.changed = true;
    }

    if result.changed {
        // CFG 结构变化 → 支配树/循环森林缓存失效，后续 pass 使用新鲜分析
        func.analysis_mut().invalidate();
    }
    Ok(result)
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loops::licm::LicmPass;

    /// 构造两循环外 pred 的循环：
    /// entry →(branch) left / right；left/right → header（两个循环外 pred）；
    /// header →(branch) body / exit；body → header（latch）。
    fn build_multi_pred_loop() -> Function {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        let left = b.create_block();
        let right = b.create_block();
        let header = b.create_block();
        let body = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let zero = b.iconst_i32(0);
        let c1 = b.icmp(IntCC::NotEqual, one, zero);
        b.branch(c1, left, &[], right, &[]);

        b.switch_to_block(left);
        b.jump(header, &[]);

        b.switch_to_block(right);
        b.jump(header, &[]);

        b.switch_to_block(header);
        let c2 = b.icmp(IntCC::NotEqual, one, zero);
        b.branch(c2, body, &[], exit, &[]);

        b.switch_to_block(body);
        b.jump(header, &[]); // latch

        b.switch_to_block(exit);
        b.ret(&[zero]);
        b.finish().expect("build")
    }

    /// 插入后：left/right 跳新块、新块 jump header、LoopForest 重建
    /// preheader == 新块。
    #[test]
    fn insert_preheader_for_multi_pred_loop() {
        let mut func = build_multi_pred_loop();
        let pass = InsertPreheaderPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "两循环外 pred 的循环应插入 preheader");

        // 找到新块：唯一一个 terminator 是 Jump 且 target 是 header、非 left/right 的块
        // （原 left/right 也是 Jump——按 retarget 后它们的 target 不再是 header 来识别）
        let header = Block(3);
        let mut ph = None;
        for (bi, blk) in func.dfg.blocks.iter().enumerate() {
            if let Terminator::Jump { target, .. } = &blk.terminator
                && *target == header
            {
                ph = Some(Block(bi as u32));
            }
        }
        let ph = ph.expect("应存在跳 header 的新 preheader 块");

        // left(1)/right(2) 的 Jump 目标改为 ph
        for p in [Block(1), Block(2)] {
            assert!(
                matches!(
                    &func.dfg.blocks[p.0 as usize].terminator,
                    Terminator::Jump { target, .. } if *target == ph
                ),
                "循环外 pred {p:?} 应改跳 preheader"
            );
        }

        // 重建循环森林：preheader == ph
        let lf = func.loop_forest();
        let the_loop = &lf.all_loops()[0];
        assert_eq!(the_loop.preheader, Some(ph), "重建后 preheader 应为新块");
    }

    /// LICM 联动：多 pred 循环 + 循环体不变 load → insert_preheader 后
    /// load 外提到新 preheader（旧行为：无 preheader 回退 header）。
    #[test]
    fn licm_hoists_to_inserted_preheader() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("test", TypeContext::new(), sig);
        let entry = b.create_block();
        let left = b.create_block();
        let right = b.create_block();
        let header = b.create_block();
        let body = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        let p = b.stack_addr(0); // 地址在循环外
        let one = b.iconst_i32(1);
        let zero = b.iconst_i32(0);
        let c1 = b.icmp(IntCC::NotEqual, one, zero);
        b.branch(c1, left, &[], right, &[]);

        b.switch_to_block(left);
        b.jump(header, &[]);

        b.switch_to_block(right);
        b.jump(header, &[]);

        b.switch_to_block(header);
        let c2 = b.icmp(IntCC::NotEqual, one, zero);
        b.branch(c2, body, &[], exit, &[]);

        b.switch_to_block(body);
        let v = b.load(p, TypeId::I32); // 不变 load
        let _used = b.iadd(v, one);
        b.jump(header, &[]);

        b.switch_to_block(exit);
        b.ret(&[zero]);
        let mut func = b.finish().expect("build");

        // insert_preheader → licm（管线同序）
        InsertPreheaderPass::new()
            .run_on_function(&mut func)
            .unwrap();
        let licm = LicmPass::new();
        let r = licm.run_on_function(&mut func).unwrap();
        assert!(r.changed, "LICM 应外提不变 load");

        // 循环体不再含 load；全函数仅 1 条 load，且在 preheader 中
        let mut loads: Vec<Block> = Vec::new();
        for (bi, blk) in func.dfg.blocks.iter().enumerate() {
            for &inst_id in &blk.inst_order {
                let inst = &func.dfg.insts[inst_id.0 as usize];
                if matches!(inst.opcode, Opcode::Load) {
                    loads.push(Block(bi as u32));
                }
            }
        }
        assert_eq!(loads.len(), 1, "load 应被外提（只剩 1 条）");
        // 定位 preheader（重建循环森林后 LoopInfo.preheader）
        let lf = func.loop_forest();
        let the_loop = &lf.all_loops()[0];
        let preheader = the_loop.preheader.expect("应有 preheader");
        assert_eq!(loads[0], preheader, "load 应位于 preheader");
    }
}
