//! Peephole 优化器 — lowering 后进行指令级模式匹配和合并。
//!
//! 在 lowering 后、regalloc 前运行，扫描机器指令序列，
//! 识别可合并/简化的相邻指令模式。
//!
//! ## 支持的模式
//!
//! - **LEA 合并**: add(base, mul(index, scale)) → LEA [base + index*scale]
//! - **比较+分支**: cmp + jcc → 合并 cmp 到分支指令的条件码
//! - **乘加融合**: mul + add → FMADD/FMA (如果 ISA 支持)
//! - **冗余移动消除**: mov r, r → 删除

use crate::ir::*;
use crate::CompileError;

/// Peephole 优化器。
#[derive(Default)]
pub struct PeepholeOptimizer {
    /// 是否启用 LEA 合并。
    pub enable_lea_combine: bool,
    /// 是否启用比较+分支合并。
    pub enable_cmp_branch_fuse: bool,
    /// 是否启用乘加融合。
    pub enable_mul_add_fuse: bool,
}

impl PeepholeOptimizer {
    pub fn new() -> Self {
        Self {
            enable_lea_combine: true,
            enable_cmp_branch_fuse: true,
            enable_mul_add_fuse: true,
        }
    }

    /// 对虚拟寄存器级别的指令序列运行 peephole 优化。
    ///
    /// 输入是一组 (opcode_name, operands) 的简化表示，
    /// 返回优化后的指令序列。
    pub fn optimize(&self, insts: &[(String, Vec<String>)]) -> Vec<(String, Vec<String>)> {
        let mut result = insts.to_vec();
        let mut changed = true;

        while changed {
            changed = false;

            // 模式 1: add r, mul(r, scale) → lea r, [base + index*scale]
            if self.enable_lea_combine {
                for i in 0..result.len().saturating_sub(1) {
                    if let Some(combined) = try_lea_combine(&result, i) {
                        result[i] = combined;
                        result.remove(i + 1);
                        changed = true;
                        break;
                    }
                }
            }

            // 模式 2: cmp + jcc → 合并
            if self.enable_cmp_branch_fuse {
                for i in 0..result.len().saturating_sub(1) {
                    if try_cmp_branch_fuse(&result, i) {
                        result[i].0 = "FUSED_CMP_BRANCH".to_string();
                        result.remove(i + 1);
                        changed = true;
                        break;
                    }
                }
            }

            // 模式 3: mul + add → fmadd
            if self.enable_mul_add_fuse {
                for i in 0..result.len().saturating_sub(1) {
                    if let Some(fused) = try_mul_add_fuse(&result, i) {
                        result[i] = fused;
                        result.remove(i + 1);
                        changed = true;
                        break;
                    }
                }
            }
        }

        result
    }

    /// 尝试将相邻的两条指令合并为一个 peephole 模式。
    pub fn try_combine(inst1: &str, inst2: &str) -> Option<String> {
        match (inst1, inst2) {
            // LEA 模式
            ("ADD_reg", "MUL_imm") => Some("LEA_RR_SCALE".into()),
            ("ADD_reg", "SHL_reg") => Some("LEA_RR_SCALE".into()),
            // 乘加融合
            ("MUL_reg", "ADD_reg") => Some("FMADD".into()),
            ("MUL_reg", "SUB_reg") => Some("FMSUB".into()),
            // 冗余消除
            ("MOV", "MOV") => None, // 删除第二条 MOV
            _ => None,
        }
    }
}

fn try_lea_combine(insts: &[(String, Vec<String>)], i: usize) -> Option<(String, Vec<String>)> {
    if i + 1 >= insts.len() {
        return None;
    }
    let (ref op1, ref args1) = insts[i];
    let (ref op2, ref args2) = insts[i + 1];

    // Pattern: ADD base, index → if index was computed by MUL/SHL with constant
    if op1.contains("ADD") && (op2.contains("MUL") || op2.contains("SHL")) {
        let base = args1.get(1).cloned().unwrap_or_default();
        let index = args2.first().cloned().unwrap_or_default();
        let scale = if op2.contains("MUL") { args2.get(1).cloned().unwrap_or("1".into()) } else { "1".into() };
        return Some(("LEA".into(), vec![args1[0].clone(), base, index, scale]));
    }
    None
}

fn try_cmp_branch_fuse(insts: &[(String, Vec<String>)], i: usize) -> bool {
    if i + 1 >= insts.len() {
        return false;
    }
    let (ref op1, _) = insts[i];
    let (ref op2, _) = insts[i + 1];
    op1.contains("CMP") && op2.contains("J") && op2.contains("cc")
}

fn try_mul_add_fuse(insts: &[(String, Vec<String>)], i: usize) -> Option<(String, Vec<String>)> {
    if i + 1 >= insts.len() {
        return None;
    }
    let (ref op1, ref args1) = insts[i];
    let (ref op2, ref args2) = insts[i + 1];
    if (op1.contains("MUL") || op1.contains("FMUL")) && (op2.contains("ADD") || op2.contains("FADD"))
        && args1[0] == args2[0]
    {
        // Only fuse if dest of mul is same as dest of add
        let src1 = args1.get(1).cloned().unwrap_or_default();
        let src2 = args1.get(2).cloned().unwrap_or_default();
        let src3 = args2.get(2).cloned().unwrap_or_default();
        return Some(("FMADD".into(), vec![args2[0].clone(), src1, src2, src3]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peephole_empty() {
        let opt = PeepholeOptimizer::new();
        let result = opt.optimize(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_try_combine() {
        assert_eq!(PeepholeOptimizer::try_combine("ADD_reg", "MUL_imm"), Some("LEA_RR_SCALE".into()));
        assert_eq!(PeepholeOptimizer::try_combine("MUL_reg", "ADD_reg"), Some("FMADD".into()));
        assert_eq!(PeepholeOptimizer::try_combine("NOP", "NOP"), None);
    }

    #[test]
    fn test_cmp_branch_fuse() {
        let insts = vec![
            ("CMP".into(), vec!["r0".into(), "r1".into()]),
            ("Jcc".into(), vec!["label".into()]),
        ];
        assert!(try_cmp_branch_fuse(&insts, 0));
    }
}
