//! 指令调度器 — 列表调度算法。
//!
//! 在 regalloc 前重排机器指令以最小化流水线停顿。
//! 构建依赖 DAG，按关键路径优先级调度指令。
//!
//! ## 延迟表 (Skylake 微架构参考)
//!
//! | 指令类别 | Issue Latency |
//! |---------|---------------|
//! | ALU (add/sub/and/or/xor) | 1 |
//! | IMUL | 3 |
//! | IDIV | 20-80 |
//! | Load (L1 hit) | 4 |
//! | Store | 1 |
//! | FP add/sub | 4 |
//! | FP mul | 4 |
//! | FP div | 10-20 |

use std::collections::{BinaryHeap, HashMap, HashSet};

/// 调度器中的指令节点。
#[derive(Clone, Debug)]
pub struct SchedInst {
    /// 原始指令索引。
    pub orig_index: usize,
    /// 指令类别（用于延迟查询）。
    pub category: InstCategory,
    /// 产出寄存器。
    pub defs: Vec<usize>,
    /// 使用寄存器。
    pub uses: Vec<usize>,
    /// 关键路径长度（从该节点到 exit 的最长延迟）。
    pub critical_path: u32,
}

/// 指令类别及其发出延迟。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstCategory {
    Alu,
    Mul,
    Div,
    Load,
    Store,
    FpAlu,
    FpMul,
    FpDiv,
    Branch,
    Other,
}

impl InstCategory {
    /// 返回此类别指令的结果可用延迟（周期数）。
    pub fn latency(&self) -> u32 {
        match self {
            InstCategory::Alu | InstCategory::Store | InstCategory::Branch => 1,
            InstCategory::Mul => 3,
            InstCategory::Div => 40,
            InstCategory::Load => 4,
            InstCategory::FpAlu => 4,
            InstCategory::FpMul => 4,
            InstCategory::FpDiv => 15,
            InstCategory::Other => 2,
        }
    }

    /// 从 opcode 名称解析类别。
    pub fn from_opcode(op: &str) -> Self {
        let op_upper = op.to_uppercase();
        if op_upper.contains("ADD") || op_upper.contains("SUB") || op_upper.contains("AND")
            || op_upper.contains("OR") || op_upper.contains("XOR") || op_upper.contains("MOV")
            || op_upper.contains("SHL") || op_upper.contains("SHR")
        {
            InstCategory::Alu
        } else if op_upper.contains("MUL") || op_upper.contains("IMUL") {
            if op_upper.starts_with('F') { InstCategory::FpMul } else { InstCategory::Mul }
        } else if op_upper.contains("DIV") || op_upper.contains("IDIV") {
            if op_upper.starts_with('F') { InstCategory::FpDiv } else { InstCategory::Div }
        } else if op_upper.contains("LD") || op_upper.contains("LOAD") || op_upper.contains("LDR") {
            InstCategory::Load
        } else if op_upper.contains("ST") || op_upper.contains("STORE") || op_upper.contains("STR") {
            InstCategory::Store
        } else if op_upper.contains("FADD") || op_upper.contains("FSUB") {
            InstCategory::FpAlu
        } else if op_upper.contains("J") || op_upper.contains("B") || op_upper.contains("RET")
            || op_upper.contains("CALL")
        {
            InstCategory::Branch
        } else {
            InstCategory::Other
        }
    }
}

/// 列表调度器。
pub struct ListScheduler {
    insts: Vec<SchedInst>,
    /// 每个指令的前驱数量（依赖计数）。
    dep_count: Vec<usize>,
    /// 每个指令的后继列表。
    successors: Vec<Vec<usize>>,
}

impl ListScheduler {
    /// 从指令和寄存器使用信息创建调度器。
    pub fn new(
        insts: Vec<(usize, InstCategory, Vec<usize>, Vec<usize>)>,
    ) -> Self {
        let n = insts.len();
        let sched_insts: Vec<SchedInst> = insts
            .iter()
            .map(|(idx, cat, defs, uses)| SchedInst {
                orig_index: *idx,
                category: *cat,
                defs: defs.clone(),
                uses: uses.clone(),
                critical_path: cat.latency(),
            })
            .collect();

        let mut dep_count = vec![0usize; n];
        let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];

        // 构建依赖 DAG: RAW (Read-After-Write) 依赖
        for i in 0..n {
            for j in (i + 1)..n {
                // 如果 i 定义了一个寄存器，j 使用了它 → i → j 依赖
                for d in &sched_insts[i].defs {
                    if sched_insts[j].uses.contains(d) {
                        successors[i].push(j);
                        dep_count[j] += 1;
                        break;
                    }
                }
                // WAR (Write-After-Read): j 定义的寄存器被 i 使用
                for u in &sched_insts[i].uses {
                    if sched_insts[j].defs.contains(u) {
                        successors[i].push(j);
                        dep_count[j] += 1;
                        break;
                    }
                }
            }
        }

        ListScheduler {
            insts: sched_insts,
            dep_count,
            successors,
        }
    }

    /// 执行列表调度，返回指令的新顺序（原始索引列表）。
    pub fn schedule(&mut self) -> Vec<usize> {
        let n = self.insts.len();

        // 计算关键路径（从 exit 反向）
        self.compute_critical_paths();

        // 就绪队列：依赖计数为 0 的指令
        let mut ready: BinaryHeap<SchedPriority> = BinaryHeap::new();
        for i in 0..n {
            if self.dep_count[i] == 0 {
                ready.push(SchedPriority {
                    inst_idx: i,
                    critical_path: self.insts[i].critical_path,
                });
            }
        }

        let mut scheduled = Vec::with_capacity(n);
        let mut local_dep = self.dep_count.clone();

        while let Some(next) = ready.pop() {
            let i = next.inst_idx;
            scheduled.push(self.insts[i].orig_index);

            // 更新后继的依赖计数
            for &succ in &self.successors[i] {
                local_dep[succ] -= 1;
                if local_dep[succ] == 0 {
                    ready.push(SchedPriority {
                        inst_idx: succ,
                        critical_path: self.insts[succ].critical_path,
                    });
                }
            }
        }

        scheduled
    }

    /// 反向计算关键路径（从每个节点到出口的最长延迟）。
    fn compute_critical_paths(&mut self) {
        let n = self.insts.len();
        // 从出口节点反向 BFS
        for i in (0..n).rev() {
            let my_lat = self.insts[i].category.latency();
            let max_succ = self.successors[i]
                .iter()
                .map(|&s| self.insts[s].critical_path)
                .max()
                .unwrap_or(0);
            self.insts[i].critical_path = my_lat + max_succ;
        }
    }
}

/// 调度优先级 — 关键路径长的优先。
#[derive(PartialEq, Eq)]
struct SchedPriority {
    inst_idx: usize,
    critical_path: u32,
}

impl Ord for SchedPriority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.critical_path.cmp(&other.critical_path)
    }
}

impl PartialOrd for SchedPriority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduler_empty() {
        let mut sched = ListScheduler::new(vec![]);
        let result = sched.schedule();
        assert!(result.is_empty());
    }

    #[test]
    fn test_scheduler_chain() {
        // i0: R0 = add R1, R2  (ALU, latency 1)
        // i1: R3 = mul R0, R4  (MUL, latency 3, depends on i0)
        let insts = vec![
            (0, InstCategory::Alu, vec![0], vec![1, 2]),
            (1, InstCategory::Mul, vec![3], vec![0, 4]),
        ];
        let mut sched = ListScheduler::new(insts);
        let result = sched.schedule();
        assert_eq!(result, vec![0, 1]); // i0 before i1 due to RAW dependency
    }

    #[test]
    fn test_scheduler_independent() {
        // Two independent instruction chains
        let insts = vec![
            (0, InstCategory::Alu, vec![0], vec![1, 2]),
            (1, InstCategory::Alu, vec![1], vec![3, 4]),
        ];
        let mut sched = ListScheduler::new(insts);
        let result = sched.schedule();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_inst_category_latency() {
        assert_eq!(InstCategory::Alu.latency(), 1);
        assert_eq!(InstCategory::Mul.latency(), 3);
        assert_eq!(InstCategory::Div.latency(), 40);
        assert_eq!(InstCategory::Load.latency(), 4);
        assert_eq!(InstCategory::FpAlu.latency(), 4);
    }
}
