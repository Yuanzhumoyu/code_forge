//! Use-List 系统 — 增量维护的 def-use 链。
//!
//! 每次 IR 修改 (append_inst, remove_inst, replace_operand) 自动更新，
//! 不再需要 `rebuild_use_lists()`。

use super::dfg::DataFlowGraph;
use super::entity::{Inst, Value};
use crate::entity_map::SecondaryMap;
use crate::error::IrError;
use smallvec::SmallVec;

// ============================================================
// Use
// ============================================================

/// 单个使用点: 值 `value` 被指令 `user` 的第 `operand_idx` 个操作数引用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Use {
    /// 被使用的值。
    pub value: Value,
    /// 使用该值的指令。
    pub user: Inst,
    /// 在 `user.operands` 中的索引。
    pub operand_idx: u8,
}

// ============================================================
// UseLists
// ============================================================

/// Use-List 管理器 — 每个 Value 的所有使用点的集合。
///
/// 存储是 [`SecondaryMap`]（句柄即下标，`O(1)` 无哈希）——S2 之前是
/// `HashMap<Value, _>`，每次记录/查询都要哈希；改用密集索引后
/// `record_inst`/`uses` 都退化为一次 `Vec` 索引。
#[derive(Clone, Debug, Default)]
pub struct UseLists {
    /// 每个 Value → 所有 Use 的列表。
    uses: SecondaryMap<Value, SmallVec<[Use; 4]>>,
}

impl UseLists {
    pub fn new() -> Self {
        Self {
            // 无需预分配：句柄即下标，`SecondaryMap` 是 `Vec<Option<_>>`，
            // 增长摊还 O(1)（旧实现给 HashMap 预分配 8 是为了少几次 rehash）。
            uses: SecondaryMap::new(),
        }
    }

    // ============================================================
    // 记录 / 移除
    // ============================================================

    /// 记录一条新指令的所有操作数使用。
    /// 在 `DFG::make_inst` 后调用。
    pub fn record_inst(&mut self, inst: Inst, operands: &[Value]) {
        for (idx, &operand) in operands.iter().enumerate() {
            self.uses.get_mut_or_default(operand).push(Use {
                value: operand,
                user: inst,
                operand_idx: idx as u8,
            });
        }
    }

    /// 移除一条指令的所有使用记录。
    /// 在指令被删除前调用。
    pub fn remove_inst(&mut self, dfg: &DataFlowGraph, inst: Inst) {
        let operands = dfg.inst_operands(inst);
        for (idx, &operand) in operands.iter().enumerate() {
            if let Some(use_list) = self.uses.get_mut(operand) {
                use_list.retain(|u| u.user != inst || u.operand_idx != idx as u8);
            }
        }
    }

    /// 忘掉 `inst` 的**全部**使用记录（按 user 扫，不依赖当前操作数）。
    ///
    /// 与 [`UseLists::remove_inst`] 的区别：后者按指令**当前**操作数逐条删除，
    /// 因此必须在改写操作数**之前**调用；一旦操作数已被就地改写，旧记录就找不到了
    /// （留下陈旧 use 项）。本方法按 `user == inst` 清扫，可在改写后安全调用——
    /// `Function::refresh_inst_uses` 用它做"改完重登记"。
    ///
    /// 代价：一次全 use 表扫描（O(值数)）。用于 pass/codegen 的少量就地改写路径；
    /// S2 把 use 表换成 `SecondaryMap` 后可退化为直接索引。
    pub fn forget_inst(&mut self, inst: Inst) -> usize {
        let mut removed = 0usize;
        for list in self.uses.values_mut() {
            let before = list.len();
            list.retain(|u| u.user != inst);
            removed += before - list.len();
        }
        removed
    }

    // ============================================================
    // 查询
    // ============================================================

    /// 获取某个值的所有使用点。
    pub fn uses(&self, value: Value) -> &[Use] {
        self.uses.get(value).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// 获取某个值的使用计数。
    pub fn use_count(&self, value: Value) -> usize {
        self.uses(value).len()
    }

    /// 检查值是否有任何使用者。
    pub fn has_uses(&self, value: Value) -> bool {
        self.use_count(value) > 0
    }

    /// 获取所有使用某个值的指令。
    pub fn user_insts(&self, value: Value) -> Vec<Inst> {
        self.uses(value).iter().map(|u| u.user).collect()
    }

    // ============================================================
    // 替换
    // ============================================================

    /// 替换特定指令中的一个操作数。
    pub fn replace_operand(&mut self, old: Value, new: Value, inst: Inst, operand_idx: u8) {
        // 从 old 的 use-list 中移除
        if let Some(old_list) = self.uses.get_mut(old) {
            old_list.retain(|u| u.user != inst || u.operand_idx != operand_idx);
        }
        // 添加到 new 的 use-list
        self.uses.get_mut_or_default(new).push(Use {
            value: new,
            user: inst,
            operand_idx,
        });
    }

    /// 替换某个值的所有使用 (RAUW: Replace All Uses With)。
    /// 返回被替换的使用数量。
    ///
    /// 注意：只维护 use-list 侧；DFG 指令中的 operands 由调用方
    /// （`Function::replace_all_uses`）同步更新。
    pub fn replace_all_uses(&mut self, old: Value, new: Value) -> usize {
        let uses: Vec<Use> = self.uses(old).to_vec();
        let count = uses.len();
        for u in uses {
            self.replace_operand(u.value, new, u.user, u.operand_idx);
        }
        // 清空 old 的 use-list
        if let Some(old_list) = self.uses.get_mut(old) {
            old_list.clear();
        }
        count
    }

    /// 移除某个值的所有使用记录（值死亡时清理 use-list 条目）。
    /// 返回被移除的数量。
    pub fn remove_value(&mut self, value: Value) -> usize {
        self.uses.remove(value).map(|l| l.len()).unwrap_or(0)
    }

    /// 清空所有使用信息。
    pub fn clear(&mut self) {
        self.uses.clear();
    }

    // ============================================================
    // 验证 (debug)
    // ============================================================

    /// 验证 UseLists 与 DFG 一致。
    pub fn verify(&self, dfg: &DataFlowGraph) -> Result<(), Vec<IrError>> {
        let mut errors = Vec::new();

        // 检查每条指令的所有操作数都在 use-lists 中
        for (inst, instruction) in dfg.insts() {
            for (idx, &operand) in instruction.operands.iter().enumerate() {
                let uses = self.uses(operand);
                if !uses
                    .iter()
                    .any(|u| u.user == inst && u.operand_idx == idx as u8)
                {
                    errors.push(IrError::Internal(format!(
                        "inst {} operand {} (value {}) not found in use-lists for {}",
                        inst, idx, operand, operand
                    )));
                }
            }
        }

        // 检查 use-lists 中的每条记录都在 DFG 中存在
        for (value, uses) in self.uses.iter() {
            for u in uses {
                if u.user.0 as usize >= dfg.inst_count() {
                    errors.push(IrError::Internal(format!(
                        "use-list for {} references non-existent inst {}",
                        value, u.user
                    )));
                }
                let operands = dfg.inst_operands(u.user);
                if u.operand_idx as usize >= operands.len()
                    || operands[u.operand_idx as usize] != value
                {
                    errors.push(IrError::Internal(format!(
                        "use-list for {}: inst {} operand {} mismatch",
                        value, u.user, u.operand_idx
                    )));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Block;
    use crate::dfg::DataFlowGraph;
    use crate::entity::TypeId;
    use crate::inst_flags::InstFlags;
    use crate::opcode::Opcode;
    use smallvec::SmallVec;

    fn make_test_dfg() -> (DataFlowGraph, Block, Value, Value, Value) {
        let mut dfg = DataFlowGraph::new();
        let i32_ty = TypeId(2);

        let (block, params) = dfg.make_block_with_params(&[i32_ty, i32_ty]);
        let inst = dfg.make_inst(
            Opcode::Iadd,
            block,
            smallvec::smallvec![params[0], params[1]],
            SmallVec::new(),
            &[i32_ty],
            InstFlags::NONE,
        );
        let result = dfg.inst_results(inst)[0];
        (dfg, block, params[0], params[1], result)
    }

    #[test]
    fn test_record_and_query() {
        let (dfg, _block, a, b, result) = make_test_dfg();
        let mut use_lists = UseLists::new();

        for (inst, instruction) in dfg.insts() {
            use_lists.record_inst(inst, &instruction.operands);
        }

        assert!(use_lists.has_uses(a));
        assert!(use_lists.has_uses(b));
        assert_eq!(use_lists.use_count(a), 1);
        assert!(!use_lists.has_uses(result));
    }

    #[test]
    fn test_replace_operand() {
        let (dfg, _block, a, _b, result) = make_test_dfg();
        let mut use_lists = UseLists::new();
        let mut iadd_inst = Inst(0);
        for (inst, instruction) in dfg.insts() {
            use_lists.record_inst(inst, &instruction.operands);
            iadd_inst = inst;
        }

        use_lists.replace_operand(a, result, iadd_inst, 0);

        assert!(!use_lists.has_uses(a));
        assert!(use_lists.has_uses(result));
    }

    #[test]
    fn test_replace_all_uses() {
        let (dfg, _block, a, _b, result) = make_test_dfg();
        let mut use_lists = UseLists::new();
        for (inst, instruction) in dfg.insts() {
            use_lists.record_inst(inst, &instruction.operands);
        }

        let count = use_lists.replace_all_uses(a, result);
        assert_eq!(count, 1);
        assert!(!use_lists.has_uses(a));
        assert!(use_lists.has_uses(result));
    }
}
