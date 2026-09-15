//! Use-List 系统 — 增量维护的 def-use 链。
//!
//! 每次 IR 修改 (append_inst, remove_inst, replace_operand) 自动更新，
//! 不再需要 `rebuild_use_lists()`。
//!
//! **S4-a（2026-09-15）**：使用点宿主从"只能是指令"扩展为
//! 「指令操作数 | 块终结符实参」（见 [`UseSite`]）。终结符用值——分支条件与实参、
//! `ret` 返回值、`switch` 判别值与 case 实参、`invoke`/`resume` 用值——由此进入
//! use-def，于是 [`crate::Function::replace_all_uses`] 才名副其实地覆盖"所有使用"
//! （此前只覆盖指令操作数：pass 里对分支实参做 RAUW 会留下悬空实参，
//! 而 `Verifier` 的 use-list 检查也看不见这一类）。

use super::dfg::DataFlowGraph;
use super::entity::{Block, Inst, Value};
use crate::entity_map::SecondaryMap;
use crate::error::IrError;
use smallvec::SmallVec;

// ============================================================
// UseSite / Use
// ============================================================

/// 使用点的宿主 —— "谁在用这个值"。
///
/// `Term` 编码的是"终结符还不是指令"这一事实（S4 主体会把终结符并入指令流、
/// 把块实参变成操作数，届时本枚举退回单一 `Inst`）。它**不是兼容层**：
/// 没有任何旧格式与它共存，`Function::set_terminator` 是写入终结符的唯一入口，
/// 且 `Verifier` 按同一契约双向校验。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseSite {
    /// 指令 `Inst` 的操作数。
    Inst(Inst),
    /// 块 `Block` 终结符的实参/用值。
    Term(Block),
}

impl UseSite {
    /// 指令宿主则返回指令 id。
    pub fn as_inst(self) -> Option<Inst> {
        match self {
            UseSite::Inst(inst) => Some(inst),
            UseSite::Term(_) => None,
        }
    }

    /// 终结符宿主则返回块 id。
    pub fn as_block(self) -> Option<Block> {
        match self {
            UseSite::Inst(_) => None,
            UseSite::Term(block) => Some(block),
        }
    }

    /// 是否为终结符用值（非指令操作数）。
    pub fn is_terminator(self) -> bool {
        matches!(self, UseSite::Term(_))
    }
}

/// 单个使用点: 值 `value` 被宿主 `site` 的第 `operand_idx` 个槽位引用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Use {
    /// 被使用的值。
    pub value: Value,
    /// 使用该值的宿主（指令或某块的终结符）。
    pub site: UseSite,
    /// 在宿主操作数序列中的下标。
    ///
    /// - [`UseSite::Inst`]：`Instruction::operands` 下标；
    /// - [`UseSite::Term`]：[`Terminator::for_each_value`] 的平坦序下标。
    pub operand_idx: u32,
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
        let site = UseSite::Inst(inst);
        for (idx, &operand) in operands.iter().enumerate() {
            self.uses.get_mut_or_default(operand).push(Use {
                value: operand,
                site,
                operand_idx: idx as u32,
            });
        }
    }

    /// 记录一个块终结符的所有用值（下标序 = `DataFlowGraph::for_each_term_value`）。
    ///
    /// 调用时机与 [`UseLists::record_inst`] 对称：写完终结符之后。
    pub fn record_terminator(&mut self, block: Block, dfg: &DataFlowGraph) {
        let site = UseSite::Term(block);
        dfg.for_each_term_value(block, |idx, value| {
            self.uses.get_mut_or_default(value).push(Use {
                value,
                site,
                operand_idx: idx,
            });
        });
    }

    /// 移除一条指令的所有使用记录。
    /// 在指令被删除前调用。
    pub fn remove_inst(&mut self, dfg: &DataFlowGraph, inst: Inst) {
        let operands = dfg.inst_operands(inst);
        for (idx, &operand) in operands.iter().enumerate() {
            if let Some(use_list) = self.uses.get_mut(operand) {
                use_list.retain(|u| u.site != UseSite::Inst(inst) || u.operand_idx != idx as u32);
            }
        }
    }

    /// 按**旧终结符的用值**精确摘除该块的终结符 use 项（`O(用值数 × 该值的使用数)`）。
    ///
    /// 必须在终结符被**改写之前**调用；改写后请用
    /// [`UseLists::forget_terminator`]（按宿主清扫，与顺序无关）。
    pub fn remove_terminator(&mut self, block: Block, dfg: &DataFlowGraph) {
        let site = UseSite::Term(block);
        dfg.for_each_term_value(block, |_, value| {
            if let Some(use_list) = self.uses.get_mut(value) {
                use_list.retain(|u| u.site != site);
            }
        });
    }

    /// 忘掉 `inst` 的**全部**使用记录（按 user 扫，不依赖当前操作数）。
    ///
    /// 与 [`UseLists::remove_inst`] 的区别：后者按指令**当前**操作数逐条删除，
    /// 因此必须在改写操作数**之前**调用；一旦操作数已被就地改写，旧记录就找不到了
    /// （留下陈旧 use 项）。本方法按 `site == Inst(inst)` 清扫，可在改写后安全调用——
    /// `Function::refresh_inst_uses` 用它做"改完重登记"。
    ///
    /// 代价：一次全 use 表扫描（O(值数)）。用于 pass/codegen 的少量就地改写路径；
    /// S2 把 use 表换成 `SecondaryMap` 后可退化为直接索引。
    pub fn forget_inst(&mut self, inst: Inst) -> usize {
        let site = UseSite::Inst(inst);
        let mut removed = 0usize;
        for list in self.uses.values_mut() {
            let before = list.len();
            list.retain(|u| u.site != site);
            removed += before - list.len();
        }
        removed
    }

    /// 忘掉 `block` 终结符的**全部**使用记录（按宿主扫，不依赖当前用值）。
    ///
    /// [`UseLists::remove_terminator`] 的"顺序不敏感"版本：就地改写终结符
    /// （`retarget`/`replace_args`/`remove_arg`）之后调用，随后用
    /// [`crate::Function::refresh_terminator_uses`] 重登记。
    pub fn forget_terminator(&mut self, block: Block) -> usize {
        let site = UseSite::Term(block);
        let mut removed = 0usize;
        for list in self.uses.values_mut() {
            let before = list.len();
            list.retain(|u| u.site != site);
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

    /// 该值的**指令**使用者（去重、保序；终结符宿主不在内，见
    /// [`UseLists::user_blocks`]）。同一指令用同一值两次只出现一次。
    pub fn user_insts(&self, value: Value) -> Vec<Inst> {
        let mut out: Vec<Inst> = Vec::new();
        for u in self.uses(value) {
            if let Some(inst) = u.site.as_inst()
                && !out.contains(&inst)
            {
                out.push(inst);
            }
        }
        out
    }

    /// 该值的**终结符**宿主块（去重、保序；用该值作实参/条件/返回值的块）。
    /// 同一块的终结符用同一值两次（如 `br %c, %b(%x), %b(%x)`）只出现一次。
    pub fn user_blocks(&self, value: Value) -> Vec<Block> {
        let mut out: Vec<Block> = Vec::new();
        for u in self.uses(value) {
            if let Some(block) = u.site.as_block()
                && !out.contains(&block)
            {
                out.push(block);
            }
        }
        out
    }

    /// 值 `value` 是否被宿主 `site` 的第 `idx` 个槽位使用。
    pub fn has_use_at(&self, value: Value, site: UseSite, idx: u32) -> bool {
        self.uses(value)
            .iter()
            .any(|u| u.site == site && u.operand_idx == idx)
    }

    // ============================================================
    // 替换
    // ============================================================

    /// 替换特定宿主中的一个槽位。
    pub fn replace_operand(&mut self, old: Value, new: Value, site: UseSite, operand_idx: u32) {
        // 从 old 的 use-list 中移除
        if let Some(old_list) = self.uses.get_mut(old) {
            old_list.retain(|u| u.site != site || u.operand_idx != operand_idx);
        }
        // 添加到 new 的 use-list
        self.uses.get_mut_or_default(new).push(Use {
            value: new,
            site,
            operand_idx,
        });
    }

    /// 替换某个值的所有使用 (RAUW: Replace All Uses With)。
    /// 返回被替换的使用数量。
    ///
    /// 注意：只维护 use-list 侧；DFG 指令操作数与终结符实参由调用方
    /// （`Function::replace_all_uses`）同步更新。
    pub fn replace_all_uses(&mut self, old: Value, new: Value) -> usize {
        let uses: Vec<Use> = self.uses(old).to_vec();
        let count = uses.len();
        for u in uses {
            self.replace_operand(u.value, new, u.site, u.operand_idx);
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

    /// 验证 UseLists 与 DFG 一致（**双向**，指令与终结符都查）。
    pub fn verify(&self, dfg: &DataFlowGraph) -> Result<(), Vec<IrError>> {
        let mut errors = Vec::new();

        // 1. 每条指令的所有操作数都在 use-lists 中
        for (inst, instruction) in dfg.insts() {
            for (idx, &operand) in instruction.operands.iter().enumerate() {
                if !self.has_use_at(operand, UseSite::Inst(inst), idx as u32) {
                    errors.push(IrError::Internal(format!(
                        "inst {} operand {} (value {}) not found in use-lists for {}",
                        inst, idx, operand, operand
                    )));
                }
            }
        }

        // 2. 每个块终结符的所有用值都在 use-lists 中（S4-a 新增覆盖）
        //（未终止的块没有终结符用值，`None` 无需检查）
        for (i, _bd) in dfg.blocks.iter().enumerate() {
            let block = Block(i as u32);
            dfg.for_each_term_value(block, |idx, value| {
                if !self.has_use_at(value, UseSite::Term(block), idx) {
                    errors.push(IrError::Internal(format!(
                        "terminator of {} operand {} (value {}) not found in use-lists",
                        block, idx, value
                    )));
                }
            });
        }

        // 3. use-lists 中的每条记录都在 DFG 中存在且值一致
        for (value, uses) in self.uses.iter() {
            for u in uses {
                match u.site {
                    UseSite::Inst(user) => {
                        if user.0 as usize >= dfg.inst_count() {
                            errors.push(IrError::Internal(format!(
                                "use-list for {} references non-existent inst {}",
                                value, user
                            )));
                            continue;
                        }
                        let operands = dfg.inst_operands(user);
                        if u.operand_idx as usize >= operands.len()
                            || operands[u.operand_idx as usize] != value
                        {
                            errors.push(IrError::Internal(format!(
                                "use-list for {}: inst {} operand {} mismatch",
                                value, user, u.operand_idx
                            )));
                        }
                    }
                    UseSite::Term(block) => {
                        let mut slot = None;
                        dfg.for_each_term_value(block, |idx, v| {
                            if idx == u.operand_idx {
                                slot = Some(v);
                            }
                        });
                        match slot {
                            None => errors.push(IrError::Internal(format!(
                                "use-list for {}: block {} terminator operand {} does not exist",
                                value, block, u.operand_idx
                            ))),
                            Some(v) if v != value => errors.push(IrError::Internal(format!(
                                "use-list for {}: block {} terminator operand {} mismatch",
                                value, block, u.operand_idx
                            ))),
                            Some(_) => {}
                        }
                    }
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
    use crate::entity::{Inst, TypeId};
    use crate::inst_flags::InstFlags;
    use crate::opcode::Opcode;
    use crate::terminator::Terminator;
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

        use_lists.replace_operand(a, result, UseSite::Inst(iadd_inst), 0);

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

    /// 终结符用值进入 use-list：记录、查询、精确摘除。
    #[test]
    fn test_record_and_remove_terminator() {
        let mut dfg = DataFlowGraph::new();
        let b0 = dfg.make_block();
        let mut use_lists = UseLists::new();
        let cond = Value(1);
        let arg = Value(2);
        dfg.set_terminator(
            b0,
            Terminator::Branch {
                cond,
                then_block: Block(1),
                then_args: smallvec::smallvec![arg],
                else_block: Block(2),
                else_args: smallvec::smallvec![],
                metadata: SmallVec::new(),
            },
        );
        use_lists.record_terminator(b0, &dfg);

        assert_eq!(use_lists.use_count(cond), 1);
        assert_eq!(use_lists.use_count(arg), 1);
        assert!(use_lists.has_use_at(cond, UseSite::Term(b0), 0));
        assert!(use_lists.has_use_at(arg, UseSite::Term(b0), 1));
        assert_eq!(use_lists.user_blocks(cond), vec![b0]);
        assert!(use_lists.user_insts(cond).is_empty());

        use_lists.remove_terminator(b0, &dfg);
        assert!(!use_lists.has_uses(cond));
        assert!(!use_lists.has_uses(arg));
    }

    /// `forget_terminator` 对顺序不敏感（就地改写后用）。
    #[test]
    fn test_forget_terminator_is_order_insensitive() {
        let mut dfg = DataFlowGraph::new();
        let b0 = dfg.make_block();
        let mut use_lists = UseLists::new();
        dfg.set_terminator(
            b0,
            Terminator::Jump {
                target: Block(1),
                args: smallvec::smallvec![Value(1), Value(2)],
                metadata: SmallVec::new(),
            },
        );
        use_lists.record_terminator(b0, &dfg);
        // 就地改写：先把旧值换掉（此时按旧用值摘除已找不到），再刷新
        if let Some(term) = dfg.block_terminator_mut(b0) {
            term.for_each_value_mut(|_, slot| *slot = Value(9));
        }
        assert_eq!(use_lists.use_count(Value(9)), 0);
        assert_eq!(use_lists.forget_terminator(b0), 2);
        use_lists.record_terminator(b0, &dfg);
        assert_eq!(use_lists.use_count(Value(9)), 2);
        assert!(!use_lists.has_uses(Value(1)));
    }
}
