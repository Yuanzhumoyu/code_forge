//! 最小别名分析（P1-5）。
//!
//! 按地址值的基址来源把内存访问分类为 栈槽 / 全局 / alloca / 未知 四类，
//! 提供 `NoAlias` / `MayAlias` 判定。保守假设：
//!
//! - 不同栈槽、不同全局、不同 alloca 实例互不重叠；
//! - 栈槽 / alloca 与全局永不重叠；
//! - 无法追溯基址的指针（函数实参、load 结果、inttoptr 等）可能指向任何位置；
//! - 同一基址内的偏移（GEP）不追踪 → 同基址视为 MayAlias（含自身）。
//!
//! 消费方：CSE / GVN（load 消重的 kill 精度）、LICM（load 外提判定）。
//! 解析是惰性 memo 化的（按 Value 缓存），同一函数内多次查询无重复工作。

use crate::*;
use std::cell::RefCell;
use std::collections::HashMap;

/// 内存访问的分类位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemoryLocation {
    /// `stack_addr` 槽（`Immediate::Int` 槽号）。
    Stack(i64),
    /// `global_addr` 全局（`GlobalId`）。
    Global(u32),
    /// `alloca` 实例（按结果 `Value` 区分）。
    Alloca(Value),
    /// 未知位置（无法追溯基址的指针）。
    Unknown,
}

/// 两个位置间的别名关系。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AliasResult {
    /// 确定不重叠（可安全跨过做 load 消重 / 外提）。
    NoAlias,
    /// 可能重叠（保守：按重叠处理）。
    MayAlias,
    /// 确定同一位置（预留；当前不追踪偏移，保守不判定）。
    MustAlias,
}

/// 最小别名分析：惰性 memo 化 地址 → 位置 解析。
#[derive(Default)]
pub struct AliasAnalysis {
    memo: RefCell<HashMap<Value, MemoryLocation>>,
}

impl AliasAnalysis {
    pub fn new() -> Self {
        Self::default()
    }

    /// 地址值 → 位置。沿 GEP / Bitcast 追到基址
    /// （`StackAddr` / `GlobalAddr` / `Alloca`），其余（实参、load 结果、
    /// inttoptr…）视为 `Unknown`。
    pub fn location_of_addr(&self, func: &Function, addr: Value) -> MemoryLocation {
        if let Some(&loc) = self.memo.borrow().get(&addr) {
            return loc;
        }
        let loc = self.compute_addr_loc(func, addr);
        self.memo.borrow_mut().insert(addr, loc);
        loc
    }

    fn compute_addr_loc(&self, func: &Function, addr: Value) -> MemoryLocation {
        match func.dfg.value_def(addr) {
            Some(ValueDef::Inst(i, _)) => {
                let inst = &func.dfg.insts[i.0 as usize];
                match &inst.opcode {
                    Opcode::StackAddr => inst
                        .immediates
                        .first()
                        .and_then(|im| match im {
                            Immediate::Int(off) => Some(*off),
                            _ => None,
                        })
                        .map(MemoryLocation::Stack)
                        .unwrap_or(MemoryLocation::Unknown),
                    Opcode::GlobalAddr => inst
                        .immediates
                        .first()
                        .and_then(|im| match im {
                            Immediate::Global(g) => Some(g.0),
                            _ => None,
                        })
                        .map(MemoryLocation::Global)
                        .unwrap_or(MemoryLocation::Unknown),
                    Opcode::Alloca => MemoryLocation::Alloca(addr),
                    // 沿地址计算链追基址；无操作数时保守 Unknown
                    Opcode::GetElementPtr | Opcode::Bitcast => inst
                        .operands
                        .first()
                        .map(|&base| self.location_of_addr(func, base))
                        .unwrap_or(MemoryLocation::Unknown),
                    _ => MemoryLocation::Unknown,
                }
            }
            // 块参数（函数实参等）与未定义值 → 未知
            _ => MemoryLocation::Unknown,
        }
    }

    /// 内存访问指令 → 位置（`Load`/`Fload`/`Store`/`Fstore`/`AtomicRmw`/
    /// `Cmpxchg`）。其余指令返回 `None`。
    pub fn location_of_access(
        &self,
        func: &Function,
        inst: &Instruction,
    ) -> Option<MemoryLocation> {
        let addr = match inst.opcode {
            Opcode::Load | Opcode::Fload => inst.operands.first()?,
            // Store/Fstore 操作数布局 = [value, addr]
            Opcode::Store | Opcode::Fstore => inst.operands.get(1)?,
            // AtomicRmw = [ptr, val]；Cmpxchg = [ptr, cmp, new]
            Opcode::AtomicRmw | Opcode::Cmpxchg => inst.operands.first()?,
            _ => return None,
        };
        Some(self.location_of_addr(func, *addr))
    }

    /// 位置对判定。
    pub fn alias(&self, a: MemoryLocation, b: MemoryLocation) -> AliasResult {
        match (a, b) {
            (MemoryLocation::Unknown, _) | (_, MemoryLocation::Unknown) => AliasResult::MayAlias,
            (MemoryLocation::Stack(x), MemoryLocation::Stack(y)) => same_base(x == y),
            (MemoryLocation::Global(x), MemoryLocation::Global(y)) => same_base(x == y),
            (MemoryLocation::Alloca(x), MemoryLocation::Alloca(y)) => same_base(x == y),
            // 栈槽 / alloca 与全局互不重叠；不同类别间 NoAlias
            (MemoryLocation::Stack(_) | MemoryLocation::Alloca(_), MemoryLocation::Global(_))
            | (
                MemoryLocation::Global(_),
                MemoryLocation::Stack(_) | MemoryLocation::Alloca(_),
            ) => AliasResult::NoAlias,
            (MemoryLocation::Stack(_), MemoryLocation::Alloca(_))
            | (MemoryLocation::Alloca(_), MemoryLocation::Stack(_)) => AliasResult::NoAlias,
        }
    }
}

fn same_base(same: bool) -> AliasResult {
    if same {
        AliasResult::MayAlias
    } else {
        AliasResult::NoAlias
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造探针函数：返回 (func, 实参 p, slot0, slot1, gep(slot0), global0, alloca0)。
    fn build_probe() -> (Function, Value, Value, Value, Value, Value, Value) {
        let sig = FunctionSignature::new(&[(TypeId::PTR, "p")], &[]);
        let mut b = FunctionBuilder::new("alias_probe", TypeContext::new(), sig);
        let (entry, params) = b.create_block_with_params(&[(TypeId::PTR, "p")]);
        b.switch_to_block(entry);
        let s0 = b.stack_addr(0);
        let s1 = b.stack_addr(1);
        let idx = b.iconst_i32(0);
        let gs0 = b.gep(s0, &[idx], TypeId::I32);
        let g0 = b.global_addr(GlobalId(0));
        let al0 = b.alloca(TypeId::I32, 1);
        b.ret(&[]);
        let func = b.finish().expect("build");
        (func, params[0], s0, s1, gs0, g0, al0)
    }

    #[test]
    fn alias_classification_and_rules() {
        let (func, p, s0, s1, gs0, g0, al0) = build_probe();
        let aa = AliasAnalysis::new();
        let l0 = aa.location_of_addr(&func, s0);
        let l1 = aa.location_of_addr(&func, s1);
        let lg = aa.location_of_addr(&func, gs0);
        let lgo = aa.location_of_addr(&func, g0);
        let la = aa.location_of_addr(&func, al0);
        let lp = aa.location_of_addr(&func, p);

        // 分类
        assert_eq!(l0, MemoryLocation::Stack(0));
        assert_eq!(l1, MemoryLocation::Stack(1));
        assert_eq!(lg, MemoryLocation::Stack(0), "GEP 应追到基址槽");
        assert_eq!(lgo, MemoryLocation::Global(0));
        assert!(matches!(la, MemoryLocation::Alloca(_)));
        assert_eq!(lp, MemoryLocation::Unknown);

        // 规则
        assert_eq!(aa.alias(l0, l0), AliasResult::MayAlias);
        assert_eq!(aa.alias(l0, l1), AliasResult::NoAlias);
        assert_eq!(aa.alias(l0, lg), AliasResult::MayAlias);
        assert_eq!(aa.alias(l0, lgo), AliasResult::NoAlias);
        assert_eq!(aa.alias(lgo, MemoryLocation::Global(1)), AliasResult::NoAlias);
        assert_eq!(aa.alias(l0, la), AliasResult::NoAlias);
        assert_eq!(aa.alias(la, lgo), AliasResult::NoAlias);
        // 未知与一切 MayAlias
        assert_eq!(aa.alias(lp, l0), AliasResult::MayAlias);
        assert_eq!(aa.alias(lp, MemoryLocation::Unknown), AliasResult::MayAlias);
        // memo：重复查询结果一致
        assert_eq!(aa.location_of_addr(&func, s0), MemoryLocation::Stack(0));
    }

    #[test]
    fn alias_location_of_access() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut b = FunctionBuilder::new("alias_access", TypeContext::new(), sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let s0 = b.stack_addr(0);
        let v = b.load(s0, TypeId::I32);
        b.store(v, s0);
        b.ret(&[v]);
        let func = b.finish().expect("build");
        let aa = AliasAnalysis::new();
        let mut load_loc = None;
        let mut store_loc = None;
        for inst in func.dfg.insts.iter() {
            match inst.opcode {
                Opcode::Load => load_loc = aa.location_of_access(&func, inst),
                Opcode::Store => store_loc = aa.location_of_access(&func, inst),
                _ => {}
            }
        }
        assert_eq!(load_loc, Some(MemoryLocation::Stack(0)));
        assert_eq!(store_loc, Some(MemoryLocation::Stack(0)));
    }
}
