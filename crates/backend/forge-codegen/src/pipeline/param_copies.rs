//! **块参数拷贝**（v27 M6）：块参数拿独立寄存器，终结符处发显式搬移。
//!
//! ## 治什么
//!
//! lowering 过去靠"让块参数复用跳转实参的寄存器"省掉参数拷贝
//! （`lowering.rs::map_terminator_args_to_params`）。这条捷径在**同一个值喂多个
//! 块参数**时不成立：两个参数会被绑到同一个 xreg，语义直接塌掉。实测形状——
//! `let mut acc = 0; let mut i = 0; while i < n { acc += ...; i += 1 }` 里
//! 入口跳转是 `jump(loop, &[0, 0])`，于是 `acc` 与 `i` 共用寄存器，循环体里
//! `acc + 1` 与 `acc + a` 互相覆盖（`n = 1` 时结果多一个 1）。
//!
//! ## 判据与回退
//!
//! 只有同时满足两条才走拷贝路径：① 本 ISA 能造搬移指令
//! （`[machine] reg_move` ⇒ `MachineInst::make_reg_move`）；② 指向带参数块的入边
//! 全是 Jump/Branch。否则**整体**退回旧的别名路径——两条路混用会让某些参数寄存器
//! 没人写，比错值更难查。
//!
//! ## 并行拷贝
//! 两阶段：实参 → 临时 → 参数。第一版直接两两搬会互相覆盖（`[a,b] → [b,a]`）。
//! 另一条硬约束：**只对 `Jump` 入边**发拷贝（两条边的 `Branch` 需要 edge splitting，
//! 否则 else 边的拷贝会污染 then 路径的寄存器——2026-10-19 实测的错值就出在这）。

use crate::pipeline::compiler::CompileState;
use forge_ir::{Block, Function, TypeId, Value, XReg};

impl<I: crate::machine::inst::MachineInst + 'static> CompileState<I> {
    /// 记下某块参数的 XReg 与类（终结符发拷贝时按这张表查）。
    pub(crate) fn record_block_param_xregs(
        &mut self,
        block: Block,
        xregs: &[XReg],
        tys: &[TypeId],
    ) {
        let pairs = xregs
            .iter()
            .copied()
            .zip(tys.iter().map(|ty| self.ctx.reg_class_for(ty)))
            .collect();
        self.block_param_xregs.insert(block, pairs);
    }

    /// 本 ISA 能不能造"寄存器 ← 寄存器"搬移（`[machine] reg_move`）？
    pub(crate) fn can_emit_moves(&self) -> bool {
        <I as crate::machine::inst::MachineInst>::make_reg_move(0, 1, self.ctx.value_gpr_class)
            .is_some()
    }

    /// 本块能不能走"独立参数 + 显式拷贝"路径？
    ///
    /// 两个前提：① 本 ISA 能造搬移；② 本块的**所有入边都是 `Jump`**。
    ///
    /// 为什么排除 `Branch` 入边：边拷贝是在**终结符之前**无条件发射的，而 `Branch` 有两条边
    /// ——把两条边的拷贝都发出去，等于让 else 边的拷贝去污染 then 路径的寄存器（反之亦然）。
    /// 正确做法是给每条边插入跳板块（edge splitting）或条件化拷贝，那是另一件事；
    /// 在那之前 `Branch` 入边一律退回别名路径（安全，且别名在该块内不会塌：塌只发生在
    /// "同一实参喂多个参数"，而 Branch 边上的参数搬运本来就少）。
    pub(crate) fn block_can_copy(&self, func: &Function, block: Block) -> bool {
        if !self.can_emit_moves() {
            return false;
        }
        let preds = func.predecessors();
        let list = preds.get(block).map(|p| p.as_slice()).unwrap_or(&[]);
        list.iter()
            .all(|p| matches!(func.dfg.term_kind(*p), Some(forge_ir::TermKind::Jump)))
    }

    /// `Jump` 终结符处发**块参数拷贝**（并行拷贝语义：先把每个实参搬进临时，再搬进参数）。
    ///
    /// 只处理 `Jump`：`Branch` 的两条边需要 edge splitting（见 [`Self::block_can_copy`]），
    /// 在那之前那些块走别名路径，不会被调用到这里。
    pub(crate) fn emit_block_arg_copies(&mut self, dfg: &forge_ir::DataFlowGraph, block: Block) {
        let mut pairs: Vec<(Block, Vec<Value>)> = Vec::new();
        match dfg.term_kind(block) {
            Some(forge_ir::TermKind::Jump) => {
                if let Some((target, args)) = dfg.term_jump(block) {
                    pairs.push((target, args.to_vec()));
                }
            }
            _ => return,
        }
        for (succ, args) in pairs {
            let Some(params) = self.block_param_xregs.get(succ).cloned() else {
                continue;
            };
            // 第一阶段：实参 → 临时（避免第二条拷贝覆盖第一条的源）。
            let mut back: Vec<(XReg, XReg, crate::prelude::RegClass)> = Vec::new();
            for (i, arg) in args.iter().enumerate() {
                let Some(&(dst, class)) = params.get(i) else {
                    continue;
                };
                let Some(&src) = self.value_to_xreg.get(*arg) else {
                    continue;
                };
                if src == dst {
                    continue;
                }
                let tmp = self.ctx.alloc_xreg(class);
                self.push_move(tmp, src, class);
                back.push((dst, tmp, class));
            }
            // 第二阶段：临时 → 参数。
            for (dst, tmp, class) in back {
                self.push_move(dst, tmp, class);
            }
        }
    }

    /// 发一条 `dst <- src` 的搬移（内含它的操作数绑定，分配器据此建区间）。
    fn push_move(&mut self, dst: XReg, src: XReg, class: crate::prelude::RegClass) {
        // **占位符语义**：谱里各条 lowering 规则造指令时字段里放的是 lowering 期的
        // 占位寄存器，值的身份由 `inst_ops` 的绑定表达，回写阶段再把字段改成物理寄存器。
        // 所以这里也传占位索引（0/0）——真正要的是"按 `class` 选对宽度变体"。
        // 传 vreg 索引会直接炸在 `Reg::from_index` 的越界校验上（实测 id 1548）。
        let Some(inst) = <I as crate::machine::inst::MachineInst>::make_reg_move(0, 0, class)
        else {
            return;
        };
        let binds: smallvec::SmallVec<[crate::machine::lowering::OperandBind; 2]> = smallvec::smallvec![
            crate::machine::lowering::OperandBind {
                vreg: dst,
                field: 0,
                role: crate::machine::inst::OperandRole::Def,
                class: Some(class),
            },
            crate::machine::lowering::OperandBind {
                vreg: src,
                field: 1,
                role: crate::machine::inst::OperandRole::Use,
                class: Some(class),
            },
        ];
        self.vcode.push_inst(inst);
        self.inst_ops.push(binds);
        self.inst_clobbers.push(Vec::new());
    }

    pub(crate) fn pre_allocate_block_param_xregs(&mut self, func: &Function) {
        struct BlockParamInfo {
            block: Block,
            param_values: Vec<Value>,
            param_tys: Vec<TypeId>,
        }

        let mut block_params_list: Vec<BlockParamInfo> = Vec::new();
        for (block, block_data) in func.dfg.blocks() {
            if Some(block) == func.entry_block {
                continue;
            }
            let param_values: Vec<Value> = block_data.param_values.iter().copied().collect();
            if !param_values.is_empty() {
                block_params_list.push(BlockParamInfo {
                    block,
                    param_values,
                    param_tys: block_data.params.iter().copied().collect(),
                });
            }
        }

        if block_params_list.is_empty() {
            return;
        }

        let preds = func.predecessors();
        // 是否走拷贝路径是**逐块**决定（`block_can_copy`）：`Branch` 入边的块必须退回别名
        // 路径，因为边拷贝无法条件化（见 `block_can_copy` 的说明）。

        for info in &block_params_list {
            let mut param_xregs: Vec<XReg> = info
                .param_tys
                .iter()
                .map(|ty| {
                    let class = self.ctx.reg_class_for(ty);
                    let xreg = self.ctx.alloc_xreg(class);
                    self.ctx.set_value_desc(xreg, *ty);
                    xreg
                })
                .collect();

            if self.block_can_copy(func, info.block) {
                // 拷贝路径：参数**各拿独立 xreg**，终结符负责搬（`emit_block_arg_copies`）。
                self.record_block_param_xregs(info.block, &param_xregs, &info.param_tys);
            } else if let Some(pred_list) = preds.get(info.block) {
                // 别名路径（旧行为）：先处理所有 pred 的传参映射（map_terminator 可能
                // 复用 arg 的已有寄存器到 param_xregs，如循环中块参数 value 作为跳转 arg）
                for pred in pred_list {
                    self.map_terminator_args_to_params(
                        &func.dfg,
                        *pred,
                        info.block,
                        &mut param_xregs,
                        &info.param_tys,
                    );
                }
            }

            // 最后：块参数 value 映射到（复用后的）param_xregs——必须与
            // 传参寄存器一致，否则目标块读取参数时用旧寄存器拿到错误值
            // （write_bytes 循环 body 的 bi 读到 dst 的寄存器，[WA-14]）。
            // 注：多 pred 且各 pred 的 arg 寄存器不同时，param_xregs 取
            // 最后处理的 pred（预先存在的限制，单 pred 循环为实际修复点）。
            for (i, &pv) in info.param_values.iter().enumerate() {
                if i < param_xregs.len() {
                    self.value_to_xreg.insert(pv, param_xregs[i]);
                }
            }
        }
    }
}
