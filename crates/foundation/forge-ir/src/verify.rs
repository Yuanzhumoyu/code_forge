//! IR 验证器 — 检查 SSA 属性、类型一致性、CFG 完整性。

use super::dfg::{DataFlowGraph, Instruction, ValueDef};
use super::entity::*;
use super::function::Function;
use super::opcode::Opcode;
use super::terminator::Terminator;
use super::types::{TypeContext, TypeEntry};
use crate::Immediate;
use crate::error::IrError;
use std::collections::{HashMap, HashSet};

// ============================================================
// VerifyError
// ============================================================

#[derive(Clone, Debug)]
pub enum VerifyError {
    MissingEntry,
    UndefinedValue {
        value: Value,
        user: Inst,
        block: Block,
    },
    TypeMismatch {
        inst: Inst,
        expected: String,
        found: String,
    },
    MissingTerminator {
        block: Block,
    },
    BlockParamCountMismatch {
        block: Block,
        expected: usize,
        found: usize,
    },
    BlockArgTypeMismatch {
        block: Block,
        param_idx: usize,
        expected: TypeId,
        found: TypeId,
    },
    UnreachableBlock {
        block: Block,
    },
    OperandCountMismatch {
        inst: Inst,
        opcode: String,
        expected: usize,
        found: usize,
    },
    ResultCountMismatch {
        inst: Inst,
        opcode: String,
        expected: usize,
        found: usize,
    },
    ReturnTypeMismatch {
        block: Block,
        expected: usize,
        found: usize,
    },
    /// Multiple blocks have no predecessors — ambiguous entry points.
    /// The IR model uses a single designated entry block.
    MultipleEntryBlocks {
        blocks: Vec<Block>,
    },
    /// Use-list verification failed — indicates an IR manipulation bug.
    UseListInconsistency {
        details: Vec<IrError>,
    },
    /// An SSA value is used in a block not dominated by its definition.
    DominanceViolation {
        value: Value,
        user: Inst,
        user_block: Block,
        def_block: Block,
    },
    /// Instruction result value has wrong def info.
    ValueDefMismatch {
        value: Value,
        expected_def: String,
        found_def: String,
    },
    /// Function with non-void return has a path that doesn't return.
    PathWithoutReturn {
        last_block: Block,
    },
    /// Return 值数量与签名匹配但类型不符。
    ReturnValueTypeMismatch {
        block: Block,
        ret_idx: usize,
        expected: TypeId,
        found: TypeId,
    },
    /// 终结符目标块不存在（与 MissingTerminator 区分：终结符存在但目标非法）。
    InvalidTerminatorTarget {
        block: Block,
        target: Block,
    },
    /// SSA 顺序违规：值在同一块内定义于使用之后（块间支配由 DominanceViolation 负责）。
    InstOrderViolation {
        value: Value,
        user: Inst,
        block: Block,
    },
    /// 终结符使用了不被支配的值（与指令级 DominanceViolation 区分）。
    TerminatorDominanceViolation {
        value: Value,
        block: Block,
        def_block: Block,
    },
    /// select 的条件必须是 bool（i1）。
    SelectCondNotBool {
        inst: Inst,
    },
    /// icmp 操作数须为整型。
    IcmpOperandNotInt {
        inst: Inst,
    },
    /// fcmp 操作数须为浮点。
    FcmpOperandNotFloat {
        inst: Inst,
    },
    /// 转换指令位宽关系非法（sext/zext 源须窄于目标；ireduce 反之；bitcast 同宽）。
    ConversionBitWidthMismatch {
        inst: Inst,
        expected: String,
        found: String,
    },
    /// 常量/立即数非法（ConstId 无效、位宽不匹配、extract_value 越界、switch case 重复等）。
    InvalidImmediate {
        inst: Inst,
        detail: String,
    },
    /// load 的地址操作数不是指针。
    LoadAddrNotPointer {
        inst: Inst,
    },
    /// store 的地址操作数不是指针。
    StoreAddrNotPointer {
        inst: Inst,
    },
    /// call 操作数不满足最低要求（无 callee 操作数 / call_indirect 首操作数非指针）。
    CallInvalidTarget {
        inst: Inst,
        detail: String,
    },
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::MissingEntry => write!(f, "function has no entry block"),
            VerifyError::UndefinedValue { value, user, block } => {
                write!(
                    f,
                    "block {}: inst {} uses undefined value {}",
                    block, user, value
                )
            }
            VerifyError::TypeMismatch {
                inst,
                expected,
                found,
            } => {
                write!(
                    f,
                    "inst {}: type mismatch (expected {}, found {})",
                    inst, expected, found
                )
            }
            VerifyError::MissingTerminator { block } => {
                write!(f, "block {}: missing terminator", block)
            }
            VerifyError::BlockParamCountMismatch {
                block,
                expected,
                found,
            } => {
                write!(
                    f,
                    "block {}: jump args count {} != block params {}",
                    block, found, expected
                )
            }
            VerifyError::BlockArgTypeMismatch {
                block,
                param_idx,
                expected,
                found,
            } => {
                write!(
                    f,
                    "block {}: param {} type mismatch (expected t{}, found t{})",
                    block, param_idx, expected.0, found.0
                )
            }
            VerifyError::UnreachableBlock { block } => {
                write!(f, "block {}: unreachable from entry", block)
            }
            VerifyError::OperandCountMismatch {
                inst,
                opcode,
                expected,
                found,
            } => {
                write!(
                    f,
                    "inst {}: {} operand count {} != expected {}",
                    inst, opcode, found, expected
                )
            }
            VerifyError::ResultCountMismatch {
                inst,
                opcode,
                expected,
                found,
            } => {
                write!(
                    f,
                    "inst {}: {} result count {} != expected {}",
                    inst, opcode, found, expected
                )
            }
            VerifyError::ReturnTypeMismatch {
                block,
                expected,
                found,
            } => {
                write!(
                    f,
                    "block {}: return value count {} != signature {}",
                    block, found, expected
                )
            }
            VerifyError::MultipleEntryBlocks { blocks } => {
                write!(f, "multiple entry blocks: {:?}", blocks)
            }
            VerifyError::UseListInconsistency { details } => {
                write!(
                    f,
                    "use-list inconsistency: {}",
                    details
                        .first()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                )
            }
            VerifyError::DominanceViolation {
                value,
                user,
                user_block,
                def_block,
            } => {
                write!(
                    f,
                    "dominance violation: value {} defined in {} is used in {} (inst {})",
                    value, def_block, user_block, user
                )
            }
            VerifyError::ValueDefMismatch {
                value,
                expected_def,
                found_def,
            } => {
                write!(
                    f,
                    "value {} def mismatch: expected {}, found {}",
                    value, expected_def, found_def
                )
            }
            VerifyError::PathWithoutReturn { last_block } => {
                write!(
                    f,
                    "path without return: block {} does not terminate with return in non-void function",
                    last_block
                )
            }
            VerifyError::ReturnValueTypeMismatch {
                block,
                ret_idx,
                expected,
                found,
            } => {
                write!(
                    f,
                    "block {}: return value {} type mismatch (expected t{}, found t{})",
                    block, ret_idx, expected.0, found.0
                )
            }
            VerifyError::InvalidTerminatorTarget { block, target } => {
                write!(
                    f,
                    "block {}: terminator targets nonexistent block {}",
                    block, target
                )
            }
            VerifyError::InstOrderViolation { value, user, block } => {
                write!(
                    f,
                    "block {}: value {} defined after its use in inst {}",
                    block, value, user
                )
            }
            VerifyError::TerminatorDominanceViolation {
                value,
                block,
                def_block,
            } => {
                write!(
                    f,
                    "dominance violation: terminator in block {} uses value {} defined in {}",
                    block, value, def_block
                )
            }
            VerifyError::SelectCondNotBool { inst } => {
                write!(f, "select cond must be bool: inst {}", inst)
            }
            VerifyError::IcmpOperandNotInt { inst } => {
                write!(f, "icmp operands must be integer: inst {}", inst)
            }
            VerifyError::FcmpOperandNotFloat { inst } => {
                write!(f, "fcmp operands must be float: inst {}", inst)
            }
            VerifyError::ConversionBitWidthMismatch {
                inst,
                expected,
                found,
            } => {
                write!(
                    f,
                    "conversion bit-width mismatch at inst {}: expected {}, found {}",
                    inst, expected, found
                )
            }
            VerifyError::InvalidImmediate { inst, detail } => {
                write!(f, "invalid immediate at inst {}: {}", inst, detail)
            }
            VerifyError::LoadAddrNotPointer { inst } => {
                write!(f, "load address must be pointer: inst {}", inst)
            }
            VerifyError::StoreAddrNotPointer { inst } => {
                write!(f, "store address must be pointer: inst {}", inst)
            }
            VerifyError::CallInvalidTarget { inst, detail } => {
                write!(f, "invalid call target at inst {}: {}", inst, detail)
            }
        }
    }
}

// ============================================================
// Verifier
// ============================================================

pub struct Verifier {
    pub errors: Vec<VerifyError>,
    ctx: Option<TypeContext>,
}

impl Verifier {
    pub fn new() -> Self {
        Self {
            errors: Vec::new(),
            ctx: None,
        }
    }

    pub fn with_ctx(ctx: TypeContext) -> Self {
        Self {
            errors: Vec::new(),
            ctx: Some(ctx),
        }
    }

    fn bool_ty(&self) -> TypeId {
        self.ctx.as_ref().map(|c| c.bool_ty()).unwrap_or(TypeId(1))
    }

    /// 验证函数。
    pub fn verify(&mut self, func: &Function) -> Result<(), Vec<VerifyError>> {
        self.errors.clear();

        self.check_entry(func);
        if !self.errors.is_empty() {
            return Err(self.errors.clone());
        }

        self.check_operand_counts(&func.dfg);
        self.check_types(&func.dfg);
        self.check_uses(&func.dfg);
        self.check_immediates(func);
        self.check_use_lists(func);
        self.check_block_params(func);
        self.check_terminators(&func.dfg);
        self.check_reachability(func);
        self.check_value_defs(func);
        self.check_dominance(func);
        self.check_inst_order(func);
        self.check_path_termination(func);

        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self.errors.clone())
        }
    }

    fn check_entry(&mut self, func: &Function) {
        if func.entry_block.is_none() {
            self.errors.push(VerifyError::MissingEntry);
            return;
        }
        let entry = func.entry_block.unwrap();
        if func.dfg.blocks.get(entry.0 as usize).is_none() {
            self.errors.push(VerifyError::MissingEntry);
            return;
        }

        // Check for multiple entry blocks: blocks with no predecessors
        // that aren't the designated entry block.复用 Function 的惰性
        // predecessors 缓存（避免每块全函数线性扫描的 O(B²)）。
        //
        // P0-14 修复：仅报"无前驱且**带参数**"的块为多入口——无前驱的
        // 空块（无参数，如 if-else 双方都 return 时结构化的空 merge 块）
        // 是合法不可达死块（LLVM 亦允许 unreachable 块）；带参数的无前驱
        // 块才真正异常（参数永不初始化 → 读未初始化值）。
        let preds = func.predecessors();
        let mut no_pred_blocks: Vec<Block> = Vec::new();
        for (block, block_data) in func.dfg.blocks() {
            let has_preds = preds.get(&block).is_some_and(|p| !p.is_empty());
            if !has_preds && !block_data.param_values.is_empty() {
                no_pred_blocks.push(block);
            }
        }
        if no_pred_blocks.len() > 1 {
            self.errors.push(VerifyError::MultipleEntryBlocks {
                blocks: no_pred_blocks,
            });
        }
    }

    fn check_operand_counts(&mut self, dfg: &DataFlowGraph) {
        for (inst, instruction) in dfg.insts() {
            let expected = instruction.opcode.expected_operand_count();
            let found = instruction.operands.len();
            if expected > 0 && found != expected {
                // Skip variable-count opcodes (Call, CallIndirect, GEP, ShuffleVector,
                // Vbroadcast, Vconcat — 后两者 expected_operand_count 返回 0 占位)
                match instruction.opcode {
                    Opcode::Call
                    | Opcode::CallIndirect
                    | Opcode::GetElementPtr
                    | Opcode::ShuffleVector
                    | Opcode::Vbroadcast
                    | Opcode::Vconcat => continue,
                    _ => {}
                }
                self.errors.push(VerifyError::OperandCountMismatch {
                    inst,
                    opcode: instruction.opcode.mnemonic().to_string(),
                    expected,
                    found,
                });
            }
        }
    }

    fn check_types(&mut self, dfg: &DataFlowGraph) {
        // Collect all defined values
        let mut defined: HashMap<Value, TypeId> = HashMap::new();
        for (v, vd) in dfg.values() {
            defined.insert(v, vd.ty);
        }

        for (inst, instruction) in dfg.insts() {
            // Check that instruction results have types
            for &result in &instruction.results {
                if !defined.contains_key(&result) {
                    self.errors.push(VerifyError::UndefinedValue {
                        value: result,
                        user: inst,
                        block: instruction.block,
                    });
                }
            }

            // Verify icmp/fcmp results are Bool
            if matches!(
                instruction.opcode,
                Opcode::Icmp { .. } | Opcode::Fcmp { .. }
            ) {
                for &r in &instruction.results {
                    if let Some(&ty) = defined.get(&r)
                        && ty != self.bool_ty()
                    {
                        self.errors.push(VerifyError::TypeMismatch {
                            inst,
                            expected: "bool".to_string(),
                            found: format!("t{}", ty.0),
                        });
                    }
                }
            }

            // Operand type consistency for binary ops
            self.check_operand_types(inst, instruction, &defined);

            // Overflow arithmetic: results[1] must be bool (i1)
            if matches!(
                instruction.opcode,
                Opcode::SaddOverflow
                    | Opcode::UaddOverflow
                    | Opcode::SsubOverflow
                    | Opcode::UsubOverflow
                    | Opcode::SmulOverflow
                    | Opcode::UmulOverflow
            ) && instruction.results.len() >= 2
                && let Some(&ty) = defined.get(&instruction.results[1])
                && ty != self.bool_ty()
            {
                self.errors.push(VerifyError::TypeMismatch {
                    inst,
                    expected: "bool (overflow flag)".to_string(),
                    found: format!("t{}", ty.0),
                });
            }

            // IsNull/IsNotNull result must be bool
            if matches!(instruction.opcode, Opcode::IsNull | Opcode::IsNotNull) {
                for &r in &instruction.results {
                    if let Some(&ty) = defined.get(&r)
                        && ty != self.bool_ty()
                    {
                        self.errors.push(VerifyError::TypeMismatch {
                            inst,
                            expected: "bool".to_string(),
                            found: format!("t{}", ty.0),
                        });
                    }
                }
            }
        }
        // NOTE: Branch condition type check is intentionally lenient —
        // the builder may use integer values as branch conditions.
    }

    /// Check that operand types are consistent for binary/ternary ops.
    fn check_operand_types(
        &mut self,
        inst: Inst,
        instruction: &super::dfg::Instruction,
        defined: &HashMap<Value, TypeId>,
    ) {
        let op = &instruction.opcode;
        // Binary ops where both operands must have the same type
        let is_binary_same_type = matches!(
            op,
            Opcode::Iadd
                | Opcode::Isub
                | Opcode::Imul
                | Opcode::Udiv
                | Opcode::Sdiv
                | Opcode::Urem
                | Opcode::Srem
                | Opcode::Band
                | Opcode::Bor
                | Opcode::Bxor
                | Opcode::Ishl
                | Opcode::Ushr
                | Opcode::Sshr
                | Opcode::Rotl
                | Opcode::Rotr
                | Opcode::Smin
                | Opcode::Smax
                | Opcode::Umin
                | Opcode::Umax
                | Opcode::SaddSat
                | Opcode::SsubSat
                | Opcode::UaddSat
                | Opcode::UsubSat
                | Opcode::SaddOverflow
                | Opcode::UaddOverflow
                | Opcode::SsubOverflow
                | Opcode::UsubOverflow
                | Opcode::SmulOverflow
                | Opcode::UmulOverflow
        );

        let is_float_binary = matches!(
            op,
            Opcode::Fadd
                | Opcode::Fsub
                | Opcode::Fmul
                | Opcode::Fdiv
                | Opcode::Frem
                | Opcode::Fmin
                | Opcode::Fmax
                | Opcode::Fcopysign
        );

        if (is_binary_same_type || is_float_binary) && instruction.operands.len() >= 2 {
            let ty0 = defined.get(&instruction.operands[0]);
            let ty1 = defined.get(&instruction.operands[1]);
            if let (Some(&t0), Some(&t1)) = (ty0, ty1)
                && t0 != t1
            {
                self.errors.push(VerifyError::TypeMismatch {
                    inst,
                    expected: format!("t{}", t0.0),
                    found: format!("t{}", t1.0),
                });
            }
        }

        // cmpxchg：cmp（operands[1]）与 new（operands[2]）类型必须一致（opaque-ptr-cmpxchg）
        if op == &Opcode::Cmpxchg && instruction.operands.len() >= 3 {
            let t1 = defined.get(&instruction.operands[1]);
            let t2 = defined.get(&instruction.operands[2]);
            if let (Some(&a), Some(&b)) = (t1, t2)
                && a != b
            {
                self.errors.push(VerifyError::TypeMismatch {
                    inst,
                    expected: format!("t{}", a.0),
                    found: format!("t{}", b.0),
                });
            }
        }

        // 3.1 聚合类型指令分类：算术/转换等指令的操作数必须是标量（load/store
        // 聚合值合法——内存访问；算术指令聚合 operand 拒绝）。
        if (is_binary_same_type || is_float_binary) && instruction.operands.len() >= 2 {
            for &opd in instruction.operands.iter().take(2) {
                if let Some(&t) = defined.get(&opd)
                    && self
                        .ctx
                        .as_ref()
                        .map(|c| c.borrow().is_aggregate(t))
                        .unwrap_or(false)
                {
                    self.errors.push(VerifyError::TypeMismatch {
                        inst,
                        expected: "scalar".to_string(),
                        found: format!("t{}", t.0),
                    });
                }
            }
        }

        // Fma: all 3 operands same type
        if matches!(op, Opcode::Fma) && instruction.operands.len() >= 3 {
            let ty0 = defined.get(&instruction.operands[0]);
            let ty1 = defined.get(&instruction.operands[1]);
            let ty2 = defined.get(&instruction.operands[2]);
            if let (Some(&t0), Some(&t1), Some(&t2)) = (ty0, ty1, ty2)
                && (t0 != t1 || t0 != t2)
            {
                self.errors.push(VerifyError::TypeMismatch {
                    inst,
                    expected: format!("t{} (all same)", t0.0),
                    found: format!("t{}, t{}, t{}", t0.0, t1.0, t2.0),
                });
            }
        }

        // Select: operand[1] and operand[2] same type, result same type
        if matches!(op, Opcode::Select)
            && instruction.operands.len() >= 3
            && let (Some(&t1), Some(&t2)) = (
                defined.get(&instruction.operands[1]),
                defined.get(&instruction.operands[2]),
            )
        {
            if t1 != t2 {
                self.errors.push(VerifyError::TypeMismatch {
                    inst,
                    expected: format!("t{}", t1.0),
                    found: format!("t{}", t2.0),
                });
            }
            // Result must match
            if let Some(&r) = instruction.results.first()
                && let Some(&rt) = defined.get(&r)
                && rt != t1
            {
                self.errors.push(VerifyError::TypeMismatch {
                    inst,
                    expected: format!("t{}", t1.0),
                    found: format!("t{}", rt.0),
                });
            }
        }

        // Icmp/Fcmp: both operands same type
        if matches!(op, Opcode::Icmp { .. } | Opcode::Fcmp { .. })
            && instruction.operands.len() >= 2
        {
            let ty0 = defined.get(&instruction.operands[0]);
            let ty1 = defined.get(&instruction.operands[1]);
            if let (Some(&t0), Some(&t1)) = (ty0, ty1)
                && t0 != t1
            {
                self.errors.push(VerifyError::TypeMismatch {
                    inst,
                    expected: format!("t{}", t0.0),
                    found: format!("t{}", t1.0),
                });
            }
            // 类别检查：icmp 须整型、fcmp 须浮点（补类别维度，同宽不足以保证语义）
            if let (Some(&t0), Some(&t1)) = (ty0, ty1) {
                if matches!(op, Opcode::Icmp { .. }) && (!t0.is_int() || !t1.is_int()) {
                    self.errors.push(VerifyError::IcmpOperandNotInt { inst });
                }
                if matches!(op, Opcode::Fcmp { .. }) && (!t0.is_float() || !t1.is_float()) {
                    self.errors.push(VerifyError::FcmpOperandNotFloat { inst });
                }
            }
        }

        // Select: cond 必须 bool（i1）
        if matches!(op, Opcode::Select)
            && instruction.operands.len() >= 3
            && let Some(&cond_ty) = defined.get(&instruction.operands[0])
            && cond_ty != TypeId::BOOL
        {
            self.errors.push(VerifyError::SelectCondNotBool { inst });
        }

        // 转换指令位宽/类别：sext/zext 源整型且窄于目标；ireduce 源整型且宽于目标；
        // bitcast 同位宽（int↔float）；vbitcast 向量总位宽相同
        if matches!(
            op,
            Opcode::Sextend
                | Opcode::Uextend
                | Opcode::Ireduce
                | Opcode::Fptrunc
                | Opcode::Fpext
                | Opcode::Fptosi
                | Opcode::Sitofp
                | Opcode::Fptoui
                | Opcode::Uitofp
                | Opcode::Ptrtoint
                | Opcode::Inttoptr
                | Opcode::Bitcast
                | Opcode::Vbitcast
        ) && let Some(&src_ty) = instruction.operands.first().and_then(|v| defined.get(v))
            && let Some(&dst_ty) = instruction.results.first().and_then(|v| defined.get(v))
        {
            match op {
                Opcode::Sextend | Opcode::Uextend => {
                    if !src_ty.is_int() || !dst_ty.is_int() || src_ty.bits() >= dst_ty.bits() {
                        self.errors.push(VerifyError::ConversionBitWidthMismatch {
                            inst,
                            expected: format!("int src ({}) < int dst ({})", src_ty.0, dst_ty.0),
                            found: format!("t{} → t{}", src_ty.0, dst_ty.0),
                        });
                    }
                }
                Opcode::Ireduce => {
                    if !src_ty.is_int() || !dst_ty.is_int() || src_ty.bits() <= dst_ty.bits() {
                        self.errors.push(VerifyError::ConversionBitWidthMismatch {
                            inst,
                            expected: format!("int src ({}) > int dst ({})", src_ty.0, dst_ty.0),
                            found: format!("t{} → t{}", src_ty.0, dst_ty.0),
                        });
                    }
                }
                Opcode::Fptrunc => {
                    if !src_ty.is_float() || !dst_ty.is_float() || src_ty.bits() <= dst_ty.bits() {
                        self.errors.push(VerifyError::ConversionBitWidthMismatch {
                            inst,
                            expected: format!(
                                "float src ({}) > float dst ({})",
                                src_ty.0, dst_ty.0
                            ),
                            found: format!("t{} → t{}", src_ty.0, dst_ty.0),
                        });
                    }
                }
                Opcode::Fpext => {
                    if !src_ty.is_float() || !dst_ty.is_float() || src_ty.bits() >= dst_ty.bits() {
                        self.errors.push(VerifyError::ConversionBitWidthMismatch {
                            inst,
                            expected: format!(
                                "float src ({}) < float dst ({})",
                                src_ty.0, dst_ty.0
                            ),
                            found: format!("t{} → t{}", src_ty.0, dst_ty.0),
                        });
                    }
                }
                Opcode::Fptosi | Opcode::Fptoui => {
                    if !src_ty.is_float() || !dst_ty.is_int() {
                        self.errors.push(VerifyError::ConversionBitWidthMismatch {
                            inst,
                            expected: "float src → int dst".to_string(),
                            found: format!("t{} → t{}", src_ty.0, dst_ty.0),
                        });
                    }
                }
                Opcode::Sitofp | Opcode::Uitofp => {
                    if !src_ty.is_int() || !dst_ty.is_float() {
                        self.errors.push(VerifyError::ConversionBitWidthMismatch {
                            inst,
                            expected: "int src → float dst".to_string(),
                            found: format!("t{} → t{}", src_ty.0, dst_ty.0),
                        });
                    }
                }
                Opcode::Ptrtoint => {
                    if !self.is_pointer_ty(src_ty) || !dst_ty.is_int() {
                        self.errors.push(VerifyError::ConversionBitWidthMismatch {
                            inst,
                            expected: "ptr src → int dst".to_string(),
                            found: format!("t{} → t{}", src_ty.0, dst_ty.0),
                        });
                    }
                }
                Opcode::Inttoptr => {
                    if !src_ty.is_int() || !self.is_pointer_ty(dst_ty) {
                        self.errors.push(VerifyError::ConversionBitWidthMismatch {
                            inst,
                            expected: "int src → ptr dst".to_string(),
                            found: format!("t{} → t{}", src_ty.0, dst_ty.0),
                        });
                    }
                }
                Opcode::Bitcast => {
                    // 同位宽（int↔float）；向量/指针用 size_bytes（bits() 对复合类型返回 0）
                    let (s, d) = if let Some(ctx) = &self.ctx {
                        let store = ctx.borrow();
                        (
                            store.size_bytes(src_ty) as u64,
                            store.size_bytes(dst_ty) as u64,
                        )
                    } else {
                        (src_ty.bits() as u64, dst_ty.bits() as u64)
                    };
                    if s != d {
                        self.errors.push(VerifyError::ConversionBitWidthMismatch {
                            inst,
                            expected: format!("same bit-width ({} = {})", src_ty.0, dst_ty.0),
                            found: format!("t{} → t{}", src_ty.0, dst_ty.0),
                        });
                    }
                }
                Opcode::Vbitcast => {
                    // 向量总位宽相同（经 TypeStore 查 len × elem bits）
                    if let Some(ctx) = &self.ctx {
                        let store = ctx.borrow();
                        let vector_bits = |ty: TypeId| match store.get(ty) {
                            TypeEntry::Vector { elem, len } => match store.get(*elem) {
                                TypeEntry::Int { bits } => Some(bits * len),
                                TypeEntry::Float { bits } => Some(*bits as u32 * len),
                                _ => None,
                            },
                            _ => None,
                        };
                        if let (Some(s), Some(d)) = (vector_bits(src_ty), vector_bits(dst_ty))
                            && s != d
                        {
                            self.errors.push(VerifyError::ConversionBitWidthMismatch {
                                inst,
                                expected: format!("same total width ({} = {})", s, d),
                                found: format!("t{} → t{}", src_ty.0, dst_ty.0),
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        // load/store 指针语义
        if matches!(op, Opcode::Load | Opcode::Fload)
            && let Some(&addr_ty) = instruction.operands.first().and_then(|v| defined.get(v))
            && !self.is_pointer_ty(addr_ty)
        {
            self.errors.push(VerifyError::LoadAddrNotPointer { inst });
        }
        // load 结果类型无大小（opaque/metadata 等占位类型）——LLVM 拒绝
        if matches!(op, Opcode::Load | Opcode::Fload)
            && let Some(rt) = instruction.results.first().and_then(|v| defined.get(v))
            && let Some(ctx) = &self.ctx
            && ctx.borrow().size_bytes(*rt) == 0
        {
            self.errors.push(VerifyError::TypeMismatch {
                inst,
                expected: "loadable type with nonzero size".into(),
                found: format!("type {rt:?} has no size (opaque/placeholder)"),
            });
        }
        if matches!(op, Opcode::Store | Opcode::Fstore)
            && instruction.operands.len() >= 2
            && let Some(&addr_ty) = instruction.operands.get(1).and_then(|v| defined.get(v))
            && !self.is_pointer_ty(addr_ty)
        {
            self.errors.push(VerifyError::StoreAddrNotPointer { inst });
        }

        // Call 低配检查：call 至少 1 个操作数（callee 指针）；call_indirect 首操作数须指针
        if matches!(op, Opcode::Call | Opcode::CallIndirect) {
            if instruction.operands.is_empty() {
                self.errors.push(VerifyError::CallInvalidTarget {
                    inst,
                    detail: "no callee operand".to_string(),
                });
            } else if matches!(op, Opcode::CallIndirect)
                && let Some(&callee_ty) = defined.get(&instruction.operands[0])
                && !self.is_pointer_ty(callee_ty)
            {
                self.errors.push(VerifyError::CallInvalidTarget {
                    inst,
                    detail: "call_indirect callee must be a pointer".to_string(),
                });
            }
        }
    }

    /// 类型是否指针（经 TypeStore 查 TypeEntry::Pointer；无 ctx 时按预设 PTR 判断）。
    fn is_pointer_ty(&self, ty: TypeId) -> bool {
        match &self.ctx {
            Some(ctx) => matches!(ctx.borrow().get(ty), TypeEntry::Pointer { .. }),
            None => ty == TypeId::PTR,
        }
    }

    /// 立即数/常量有效性：ConstId 可解析、extract_value/insert_value 索引越界、
    /// switch case 值重复。
    /// GEP struct 索引校验：索引遍历 indexed_ty（immediates[0]），struct 位置
    /// 的索引必须是 i32（LLVM LangRef）；常量索引推进类型，非常量索引后无法
    /// 继续推导（跳过后续 struct 检查——保守不误报）。
    fn check_gep_indices(&mut self, func: &Function, inst: crate::Inst, instruction: &Instruction) {
        let Some(Immediate::Type(indexed)) = instruction.immediates.first() else {
            return;
        };
        let ts = func.types.borrow();
        let total_idx = instruction.operands.len().saturating_sub(1);
        let mut cur = *indexed;
        for (k, v) in instruction.operands.iter().skip(1).enumerate() {
            // 标量位置：LLVM 拒绝继续索引（"indexing into scalar"）——只允许
            // 作为最后一个索引（数组元素访问的末位索引到达标量合法）。
            if !ts.is_aggregate(cur) {
                if k + 1 < total_idx {
                    self.errors.push(VerifyError::TypeMismatch {
                        inst,
                        expected: "aggregate type for further index".into(),
                        found: format!("scalar {cur:?}"),
                    });
                }
                return;
            }
            let is_struct = ts.element_type(cur).is_none();
            // struct 位置：索引必须是 i32（LLVM LangRef：结构体索引仅 i32 常量）
            if is_struct
                && let Some(ty) = func.dfg.value_type(*v)
                && ty != TypeId::I32
            {
                self.errors.push(VerifyError::TypeMismatch {
                    inst,
                    expected: "i32 struct index".into(),
                    found: format!("{ty:?}"),
                });
                return;
            }
            // 推进类型（常量索引；非常量索引后无法推导——保守跳过）
            let idx_const = match func.dfg.values[v.0 as usize].def {
                crate::ValueDef::Inst(ii, 0) => func.dfg.insts[ii.0 as usize]
                    .immediates
                    .first()
                    .and_then(|i| match i {
                        Immediate::Int(x) => Some(*x),
                        Immediate::Uint(x) => Some(*x as i64),
                        Immediate::Const(c) => func.constants.get_int(*c).map(|(x, _)| x as i64),
                        _ => None,
                    }),
                _ => None,
            };
            let next = match idx_const {
                Some(c) if is_struct => ts.aggregate_elem_type(cur, c as u32),
                Some(_) => ts.element_type(cur),
                None if is_struct => {
                    // S4.1：struct 位置非常量索引——LLVM 拒绝（LangRef：
                    // struct 索引必须是 i32 常量）
                    self.errors.push(VerifyError::TypeMismatch {
                        inst,
                        expected: "constant i32 struct index".into(),
                        found: "non-constant index".into(),
                    });
                    return;
                }
                // 数组/向量位置非常量索引合法（`i8*` 指针运算）——类型不可
                // 推导，保守跳过
                None => None,
            };
            match next {
                Some(t) => cur = t,
                None => return,
            }
        }
    }

    fn check_immediates(&mut self, func: &Function) {
        let dfg = &func.dfg;
        for (inst, instruction) in dfg.insts() {
            // GEP struct 索引必须是 i32（LLVM LangRef：getelementptr 的结构体
            // 索引只能是 i32 常量——i64 等其他整数类型被 llvm-as 拒绝）
            if instruction.opcode == Opcode::GetElementPtr {
                self.check_gep_indices(func, inst, instruction);
            }
            // atomicrmw 操作数类型约束（LLVM：add/sub/xchg 等须整数，
            // fadd/fsub 须浮点）
            if instruction.opcode == Opcode::AtomicRmw
                && let Some(Immediate::Uint(op)) = instruction.immediates.first()
            {
                use crate::opcode::AtomicRmwOp as R;
                let float_ops = matches!(
                    *op,
                    o if o == R::Fadd as u64 || o == R::Fsub as u64
                );
                if let Some(&val_ty) = instruction.operands.get(1)
                    && let Some(ty) = dfg.value_type(val_ty)
                    && let Some(val_is_float) = self.ctx.as_ref().map(|c| c.borrow().is_float(ty))
                {
                    let mismatch = if float_ops {
                        !val_is_float
                    } else {
                        val_is_float
                    };
                    if mismatch {
                        self.errors.push(VerifyError::TypeMismatch {
                            inst,
                            expected: format!(
                                "{} value",
                                if float_ops { "float" } else { "integer" }
                            ),
                            found: format!("t{}", ty.0),
                        });
                    }
                }
            }
            // 原子指令内存序合法性（LLVM：cmpxchg succ/fail 不能是 unordered(0)，
            // 且 fail 不能是 release(4)/acq_rel(5)）
            if matches!(
                instruction.opcode,
                Opcode::AtomicRmw | Opcode::Cmpxchg | Opcode::Fence
            ) {
                let ord_from = |i: usize| {
                    instruction
                        .immediates
                        .get(i)
                        .and_then(|im| im.as_u64())
                        .map(|v| v as i64)
                };
                let bad = match instruction.opcode {
                    Opcode::Cmpxchg => {
                        // Unordered(1) 不允许（LLVM：cmpxchg 须 monotonic 或更强）；
                        // fail 序（immediates[1]）不能是 release(4)/acq_rel(5)
                        ord_from(1).is_some_and(|v| v == 4 || v == 5)
                            || ord_from(0) == Some(1)
                            || ord_from(1) == Some(1)
                    }
                    _ => false,
                };
                if bad {
                    self.errors.push(VerifyError::InvalidImmediate {
                        inst,
                        detail: "cmpxchg ordering must be monotonic or stronger (fail cannot be release/acq_rel)".to_string(),
                    });
                }
            }
            // 算术标志合法性：nsw/nuw 仅 add/sub/mul/shl；exact 仅 udiv/sdiv/lshr/ashr
            //（LLVM：`add nsw i32 %a, i32 %b` / `udiv exact i32 %a, i32 %b`）
            if instruction
                .flags
                .intersects(crate::InstFlags::NSW | crate::InstFlags::NUW)
                && !matches!(
                    instruction.opcode,
                    Opcode::Iadd
                        | Opcode::Isub
                        | Opcode::Imul
                        | Opcode::Ishl
                        | Opcode::Vadd
                        | Opcode::Vsub
                        | Opcode::Vmul
                )
            {
                self.errors.push(VerifyError::InvalidImmediate {
                    inst,
                    detail: "nsw/nuw only valid on add/sub/mul/shl".to_string(),
                });
            }
            if instruction.flags.contains(crate::InstFlags::EXACT)
                && !matches!(
                    instruction.opcode,
                    Opcode::Udiv | Opcode::Sdiv | Opcode::Ushr | Opcode::Sshr | Opcode::Vdiv
                )
            {
                self.errors.push(VerifyError::InvalidImmediate {
                    inst,
                    detail: "exact only valid on udiv/sdiv/lshr/ashr".to_string(),
                });
            }
            // fast-math 标志仅浮点运算指令（LLVM：`fmul fast float %a, %b`）
            if instruction.flags.intersects(crate::InstFlags::FMF_FAST)
                && !matches!(
                    instruction.opcode,
                    Opcode::Fadd
                        | Opcode::Fsub
                        | Opcode::Fmul
                        | Opcode::Fdiv
                        | Opcode::Frem
                        | Opcode::Fsqrt
                        | Opcode::Fma
                        | Opcode::Fmin
                        | Opcode::Fmax
                        | Opcode::Fneg
                        | Opcode::Vadd
                        | Opcode::Vsub
                        | Opcode::Vmul
                        | Opcode::Vdiv
                        | Opcode::Fcopysign
                )
            {
                self.errors.push(VerifyError::InvalidImmediate {
                    inst,
                    detail: "fast-math flags only valid on float arithmetic".to_string(),
                });
            }
            // ConstId 有效性（跨 tag 池解析）
            for imm in &instruction.immediates {
                if let Immediate::Const(cid) = imm {
                    let valid = func.constants.get_int(*cid).is_some()
                        || func.constants.get_float128(*cid).is_some()
                        || func.constants.get_big(*cid).is_some()
                        || func.constants.get_vector(*cid).is_some();
                    if !valid {
                        self.errors.push(VerifyError::InvalidImmediate {
                            inst,
                            detail: format!("invalid ConstId {}", cid),
                        });
                    }
                }
            }
            // vconst：常量池字节长度必须等于向量类型大小（lane 数量与类型匹配，
            // 防止 lane 缺失/多余导致降级错位）
            if instruction.opcode == Opcode::Vconst
                && let Some(Immediate::Const(cid)) = instruction.immediates.first()
                && let Some(&r) = instruction.results.first()
                && let Some(ty) = dfg.value_type(r)
                && let Some(ctx) = &self.ctx
            {
                let expected = ctx.borrow().size_bytes(ty) as usize;
                if let Some(data) = func.constants.get_vector(*cid)
                    && data.len() != expected
                {
                    self.errors.push(VerifyError::InvalidImmediate {
                        inst,
                        detail: format!(
                            "vconst data size {} != vector type size {expected}",
                            data.len()
                        ),
                    });
                }
            }
            // extract_value/insert_value 索引越界（聚合类型字段数）
            // agg 类型来源：普通路径取操作数类型；聚合常量路径取 immediates[2]=Type
            if matches!(
                instruction.opcode,
                Opcode::ExtractValue | Opcode::InsertValue
            ) && let Some(Immediate::Uint(idx)) = instruction.immediates.first()
                && let Some(agg_ty) = instruction
                    .operands
                    .first()
                    .and_then(|v| dfg.value_type(*v))
                    .or_else(|| match instruction.immediates.get(2) {
                        Some(Immediate::Type(t)) => Some(*t),
                        _ => None,
                    })
            {
                let field_count = if let Some(ctx) = &self.ctx {
                    let store = ctx.borrow();
                    match store.get(agg_ty) {
                        TypeEntry::Struct { fields, .. } => Some(fields.len() as u64),
                        TypeEntry::Array { len, .. } => Some(*len),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(n) = field_count
                    && *idx >= n
                {
                    self.errors.push(VerifyError::InvalidImmediate {
                        inst,
                        detail: format!(
                            "extract/insert index {} out of bounds ({} fields)",
                            idx, n
                        ),
                    });
                }
            }
            // insert_value 值类型匹配（LLVM：插入值类型 = 目标字段类型）
            if instruction.opcode == Opcode::InsertValue
                && let Some(&val) = instruction.operands.get(1)
                && let Some(val_ty) = dfg.value_type(val)
                && let Some(agg_ty) = instruction
                    .operands
                    .first()
                    .and_then(|v| dfg.value_type(*v))
                    .or_else(|| match instruction.immediates.get(2) {
                        Some(Immediate::Type(t)) => Some(*t),
                        _ => None,
                    })
                && let Some(Immediate::Uint(idx)) = instruction.immediates.first()
                && let Some(ctx) = &self.ctx
            {
                let store = ctx.borrow();
                let field_ty = match store.get(agg_ty) {
                    TypeEntry::Struct { fields, .. } => fields.get(*idx as usize).map(|f| f.ty),
                    TypeEntry::Array { elem, .. } => Some(*elem),
                    _ => None,
                };
                if let Some(ft) = field_ty
                    && ft != val_ty
                {
                    self.errors.push(VerifyError::TypeMismatch {
                        inst,
                        expected: format!("t{}", ft.0),
                        found: format!("t{}", val_ty.0),
                    });
                }
            }
        }
        // switch case 值重复
        for (_, bd) in dfg.blocks() {
            if let Terminator::Switch { cases, .. } = &bd.terminator {
                let mut seen: HashSet<i64> = HashSet::new();
                for (val, _, _) in cases {
                    if !seen.insert(*val) {
                        self.errors.push(VerifyError::InvalidImmediate {
                            inst: Inst(u32::MAX), // 终结符无指令句柄，detail 说明
                            detail: format!("duplicate switch case value {}", val),
                        });
                    }
                }
            }
        }
    }

    fn check_uses(&mut self, dfg: &DataFlowGraph) {
        // Collect all defined values
        let mut defined: HashSet<Value> = HashSet::new();
        for (v, _) in dfg.values() {
            defined.insert(v);
        }

        for (inst, instruction) in dfg.insts() {
            for &operand in &instruction.operands {
                if !defined.contains(&operand) {
                    self.errors.push(VerifyError::UndefinedValue {
                        value: operand,
                        user: inst,
                        block: instruction.block,
                    });
                }
            }
        }

        // Check terminator values
        for (block, block_data) in dfg.blocks() {
            for val in block_data.terminator.used_values() {
                if !defined.contains(&val) {
                    // Create a fake inst to report the error
                    self.errors.push(VerifyError::UndefinedValue {
                        value: val,
                        user: Inst(u32::MAX), // terminator reference
                        block,
                    });
                }
            }
        }
    }

    fn check_block_params(&mut self, func: &Function) {
        let dfg = &func.dfg;
        let signature_rets = func.return_types();
        // 复用 Function 的惰性 predecessors 缓存（避免每块全函数扫描的 O(B²)）
        let preds = func.predecessors();

        for (block, block_data) in dfg.blocks() {
            let expected_params = block_data.params.len();

            // 仅遍历本块的实际前驱，检查其终结符传给本块的参数
            if let Some(block_preds) = preds.get(&block) {
                for &pred in block_preds {
                    let pred_data = match dfg.blocks.get(pred.0 as usize) {
                        Some(pd) => pd,
                        None => continue,
                    };
                    match &pred_data.terminator {
                        Terminator::Branch {
                            then_block,
                            then_args,
                            else_block,
                            else_args,
                            ..
                        } => {
                            // Then branch
                            if *then_block == block {
                                if then_args.len() != expected_params {
                                    self.errors.push(VerifyError::BlockParamCountMismatch {
                                        block: pred,
                                        expected: expected_params,
                                        found: then_args.len(),
                                    });
                                } else {
                                    Self::check_arg_types(
                                        &mut self.errors,
                                        dfg,
                                        *then_block,
                                        then_args,
                                        block_data,
                                    );
                                }
                            }
                            // Else branch
                            if *else_block == block {
                                if else_args.len() != expected_params {
                                    self.errors.push(VerifyError::BlockParamCountMismatch {
                                        block: pred,
                                        expected: expected_params,
                                        found: else_args.len(),
                                    });
                                } else {
                                    Self::check_arg_types(
                                        &mut self.errors,
                                        dfg,
                                        *else_block,
                                        else_args,
                                        block_data,
                                    );
                                }
                            }
                        }
                        Terminator::Jump { target, args, .. } if *target == block => {
                            if args.len() != expected_params {
                                self.errors.push(VerifyError::BlockParamCountMismatch {
                                    block: pred,
                                    expected: expected_params,
                                    found: args.len(),
                                });
                            } else {
                                Self::check_arg_types(
                                    &mut self.errors,
                                    dfg,
                                    *target,
                                    args,
                                    block_data,
                                );
                            }
                        }
                        Terminator::Switch {
                            default_block,
                            default_args,
                            cases,
                            ..
                        } => {
                            // Default case
                            if *default_block == block {
                                if default_args.len() != expected_params {
                                    self.errors.push(VerifyError::BlockParamCountMismatch {
                                        block: pred,
                                        expected: expected_params,
                                        found: default_args.len(),
                                    });
                                } else {
                                    Self::check_arg_types(
                                        &mut self.errors,
                                        dfg,
                                        *default_block,
                                        default_args,
                                        block_data,
                                    );
                                }
                            }
                            // Case branches
                            for (_, case_block, case_args) in cases.iter() {
                                if *case_block == block {
                                    if case_args.len() != expected_params {
                                        self.errors.push(VerifyError::BlockParamCountMismatch {
                                            block: pred,
                                            expected: expected_params,
                                            found: case_args.len(),
                                        });
                                    } else {
                                        Self::check_arg_types(
                                            &mut self.errors,
                                            dfg,
                                            *case_block,
                                            case_args,
                                            block_data,
                                        );
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Check Return: 数量与类型都须匹配签名
            if let Terminator::Return { values, .. } = &block_data.terminator {
                if values.len() != signature_rets.len() {
                    self.errors.push(VerifyError::ReturnTypeMismatch {
                        block,
                        expected: signature_rets.len(),
                        found: values.len(),
                    });
                } else {
                    for (i, (&v, &expected)) in values.iter().zip(signature_rets.iter()).enumerate()
                    {
                        if let Some(found) = dfg.value_type(v)
                            && found != expected
                        {
                            self.errors.push(VerifyError::ReturnValueTypeMismatch {
                                block,
                                ret_idx: i,
                                expected,
                                found,
                            });
                        }
                    }
                }
            }
        }
    }

    /// Check that jump/branch arguments match target block parameter types.
    fn check_arg_types(
        errors: &mut Vec<VerifyError>,
        dfg: &DataFlowGraph,
        target_block: Block,
        args: &[Value],
        target_data: &super::dfg::BlockData,
    ) {
        for (idx, (&arg, &param_ty)) in args.iter().zip(target_data.params.iter()).enumerate() {
            if let Some(arg_ty) = dfg.value_type(arg)
                && arg_ty != param_ty
            {
                errors.push(VerifyError::BlockArgTypeMismatch {
                    block: target_block,
                    param_idx: idx,
                    expected: param_ty,
                    found: arg_ty,
                });
            }
        }
    }

    fn check_terminators(&mut self, dfg: &DataFlowGraph) {
        for (block, block_data) in dfg.blocks() {
            // 块从未设置终结符（构建遗漏）→ MissingTerminator。
            // FunctionBuilder::finish 有防御，但手构 dfg 的函数可绕过，verify 必须兜底。
            if !block_data.has_terminator {
                self.errors.push(VerifyError::MissingTerminator { block });
            }
            // All blocks must have a non-default terminator
            // (Unreachable is allowed as an explicit terminator)
            match &block_data.terminator {
                Terminator::Jump { target, .. } if dfg.blocks.get(target.0 as usize).is_none() => {
                    self.errors.push(VerifyError::InvalidTerminatorTarget {
                        block,
                        target: *target,
                    });
                }
                Terminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => {
                    if dfg.blocks.get(then_block.0 as usize).is_none() {
                        self.errors.push(VerifyError::InvalidTerminatorTarget {
                            block,
                            target: *then_block,
                        });
                    }
                    if dfg.blocks.get(else_block.0 as usize).is_none() {
                        self.errors.push(VerifyError::InvalidTerminatorTarget {
                            block,
                            target: *else_block,
                        });
                    }
                }
                Terminator::Switch {
                    default_block,
                    cases,
                    ..
                } => {
                    if dfg.blocks.get(default_block.0 as usize).is_none() {
                        self.errors.push(VerifyError::InvalidTerminatorTarget {
                            block,
                            target: *default_block,
                        });
                    }
                    for (_, target, _) in cases.iter() {
                        if dfg.blocks.get(target.0 as usize).is_none() {
                            self.errors.push(VerifyError::InvalidTerminatorTarget {
                                block,
                                target: *target,
                            });
                            break;
                        }
                    }
                }
                Terminator::Invoke {
                    normal_block,
                    unwind_block,
                    ..
                } => {
                    if dfg.blocks.get(normal_block.0 as usize).is_none() {
                        self.errors.push(VerifyError::InvalidTerminatorTarget {
                            block,
                            target: *normal_block,
                        });
                    }
                    if dfg.blocks.get(unwind_block.0 as usize).is_none() {
                        self.errors.push(VerifyError::InvalidTerminatorTarget {
                            block,
                            target: *unwind_block,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn check_use_lists(&mut self, func: &Function) {
        if let Err(errors) = func.use_lists.verify(&func.dfg) {
            self.errors
                .push(VerifyError::UseListInconsistency { details: errors });
        }
    }

    fn check_reachability(&mut self, func: &Function) {
        // BFS from the entry block — if no entry block, already reported in check_entry
        let entry = match func.entry_block {
            Some(e) => e,
            None => return,
        };

        let mut reachable = HashSet::new();
        let mut worklist = vec![entry];
        reachable.insert(entry);

        while let Some(block) = worklist.pop() {
            if let Some(block_data) = func.dfg.blocks.get(block.0 as usize) {
                for succ in block_data.terminator.successors() {
                    if reachable.insert(succ) {
                        worklist.push(succ);
                    }
                }
            }
        }

        for (block, block_data) in func.dfg.blocks() {
            if !reachable.contains(&block) {
                // P0-14 修复：豁免**近空**不可达块——结构化控制流（if-else
                // 双方 return）合法产生死 merge 块（至多 1 条内部指令/无参数，
                // LLVM 亦允许 unreachable 块）。多条指令的不可达块才是可疑
                // 产物（死代码泄漏 / 未初始化）。
                let is_dead_merge = block_data.inst_order.len() <= 1
                    && block_data.param_values.is_empty();
                if !is_dead_merge {
                    self.errors.push(VerifyError::UnreachableBlock { block });
                }
            }
        }
    }

    /// Check that every ValueDef is consistent with the DFG state.
    /// - ValueDef::Inst(i, idx): i exists and idx < opcode.result_count()
    /// - ValueDef::Param(b, idx): b exists and idx < block.params.len()
    ///
    /// 已删除指令（Nop 墓碑）的残留值：仅当**仍被使用**时才是错误
    /// （`Function::kill_inst` 的正常结果是留下无使用者的 VOID 残留值）。
    fn check_value_defs(&mut self, func: &Function) {
        let dfg = &func.dfg;
        for (value, vd) in dfg.values() {
            match vd.def {
                ValueDef::Inst(inst, result_idx) => {
                    if (inst.0 as usize) >= dfg.insts.len() {
                        self.errors.push(VerifyError::ValueDefMismatch {
                            value,
                            expected_def: format!("inst {} result {}", inst, result_idx),
                            found_def: "inst out of bounds".to_string(),
                        });
                        continue;
                    }
                    let inst_data = &dfg.insts[inst.0 as usize];
                    if matches!(inst_data.opcode, Opcode::Nop) && inst_data.results.is_empty() {
                        if func.use_lists.use_count(value) > 0 {
                            self.errors.push(VerifyError::ValueDefMismatch {
                                value,
                                expected_def: format!("inst {} result {}", inst, result_idx),
                                found_def: "inst is deleted (Nop) but value is still used"
                                    .to_string(),
                            });
                        }
                    } else if result_idx as usize >= inst_data.results.len() {
                        self.errors.push(VerifyError::ValueDefMismatch {
                            value,
                            expected_def: format!(
                                "inst {} result {} (opcode {} produces {} results)",
                                inst,
                                result_idx,
                                inst_data.opcode.mnemonic(),
                                inst_data.results.len()
                            ),
                            found_def: "result index out of bounds".to_string(),
                        });
                    }
                }
                ValueDef::Param(block, param_idx) => {
                    if (block.0 as usize) >= dfg.blocks.len() {
                        self.errors.push(VerifyError::ValueDefMismatch {
                            value,
                            expected_def: format!("block {} param {}", block, param_idx),
                            found_def: "block out of bounds".to_string(),
                        });
                        continue;
                    }
                    let block_data = &dfg.blocks[block.0 as usize];
                    if param_idx as usize >= block_data.params.len() {
                        self.errors.push(VerifyError::ValueDefMismatch {
                            value,
                            expected_def: format!(
                                "block {} param {} (block has {} params)",
                                block,
                                param_idx,
                                block_data.params.len()
                            ),
                            found_def: "param index out of bounds".to_string(),
                        });
                    }
                }
                ValueDef::AggConst(agg_id) => {
                    // 聚合常量值：常量池引用（越界防御——池外 AggId 报错）
                    if func.constants.get_aggregate(agg_id).is_none() {
                        self.errors.push(VerifyError::ValueDefMismatch {
                            value,
                            expected_def: format!("agg const {}", agg_id.0),
                            found_def: "aggregate id out of bounds".to_string(),
                        });
                    }
                }
                // 未定义引用占位（前向引用/故意未定义——合法,不校验）
                ValueDef::UndefNamed(_) => {}
            }
        }
    }

    /// Check that every use of a value is dominated by its definition.
    fn check_dominance(&mut self, func: &Function) {
        let entry = match func.entry_block {
            Some(e) => e,
            None => return,
        };

        // 使用 Function 的惰性支配树缓存（不再重复构建）
        let domtree = func.dominator_tree();
        if domtree.block_count() == 0 {
            return;
        }

        // For each instruction, check each operand
        for (inst, instruction) in func.dfg.insts() {
            let user_block = instruction.block;
            for &operand in &instruction.operands {
                // Find the defining block
                let def_block = match func.dfg.value_def(operand) {
                    // 聚合常量值：无定义块（常量），视为 entry（恒可达）
                    Some(ValueDef::AggConst(_)) => entry,
                    // 未定义引用占位：无定义块,视为 entry（恒可达——第二十九轮）
                    Some(ValueDef::UndefNamed(_)) => entry,
                    Some(ValueDef::Inst(def_inst, _)) => {
                        // Look up the instruction's block
                        func.dfg
                            .insts
                            .get(def_inst.0 as usize)
                            .map(|i| i.block)
                            .unwrap_or(user_block)
                    }
                    Some(ValueDef::Param(def_block, _)) => *def_block,
                    None => continue, // undefined value — already reported
                };

                // Block params are defined at block entry and dominate
                // all instructions in that block. Skip self-dominance check.
                if def_block != user_block && !domtree.dominates(def_block, user_block) {
                    self.errors.push(VerifyError::DominanceViolation {
                        value: operand,
                        user: inst,
                        user_block,
                        def_block,
                    });
                }
            }
        }

        // Check terminator values（用专用错误变体，不再伪造 Inst(u32::MAX)）。
        // 异常边豁免：Invoke 的 unwind_args（异常路径传值）不受正常支配树约束——
        // unwind 边是隐式异常路径，其值不要求定义块支配 unwind 块（LLVM 语义）。
        for (block, block_data) in func.dfg.blocks() {
            let vals: Vec<Value> = match &block_data.terminator {
                Terminator::Invoke {
                    args, normal_args, ..
                } => {
                    let mut v = args.to_vec();
                    v.extend_from_slice(normal_args);
                    v
                }
                other => other.used_values(),
            };
            for val in vals {
                let def_block = match func.dfg.value_def(val) {
                    Some(ValueDef::AggConst(_)) => entry,
                    Some(ValueDef::UndefNamed(_)) => entry,
                    Some(ValueDef::Inst(def_inst, _)) => func
                        .dfg
                        .insts
                        .get(def_inst.0 as usize)
                        .map(|i| i.block)
                        .unwrap_or(entry),
                    Some(ValueDef::Param(def_block, _)) => *def_block,
                    None => continue,
                };

                if def_block != block && !domtree.dominates(def_block, block) {
                    self.errors.push(VerifyError::TerminatorDominanceViolation {
                        value: val,
                        block,
                        def_block,
                    });
                }
            }
        }
    }

    /// 检查同块 SSA 位置序：指令使用某值前，其定义必须已出现
    /// （块参数视为块开头已定义）。块间支配由 [`Self::check_dominance`] 负责。
    fn check_inst_order(&mut self, func: &Function) {
        let dfg = &func.dfg;
        for (block, block_data) in dfg.blocks() {
            let mut defined: HashSet<Value> = block_data.param_values.iter().copied().collect();
            for &inst_id in &block_data.inst_order {
                let inst = match dfg.insts.get(inst_id.0 as usize) {
                    Some(i) => i,
                    None => continue,
                };
                // 跳过已删除（Nop 墓碑）指令
                if matches!(inst.opcode, Opcode::Nop) && inst.results.is_empty() {
                    continue;
                }
                // 结果数必须与 opcode 元数据一致（Store/Trap 无结果、overflow 双结果）
                let expected_results = inst.opcode.result_count() as usize;
                if inst.results.len() != expected_results {
                    self.errors.push(VerifyError::ResultCountMismatch {
                        inst: inst_id,
                        opcode: inst.opcode.mnemonic().to_string(),
                        expected: expected_results,
                        found: inst.results.len(),
                    });
                }
                for &op in &inst.operands {
                    if defined.contains(&op) {
                        continue;
                    }
                    // 未在已定义集合中：若定义在同块（靠后位置）则为顺序违规；
                    // 跨块 use 由支配检查报告
                    if let Some(ValueDef::Inst(def_inst, _)) = dfg.value_def(op)
                        && dfg
                            .insts
                            .get(def_inst.0 as usize)
                            .is_some_and(|d| d.block == block)
                    {
                        self.errors.push(VerifyError::InstOrderViolation {
                            value: op,
                            user: inst_id,
                            block,
                        });
                    }
                }
                for &r in &inst.results {
                    defined.insert(r);
                }
            }
        }
    }

    /// Check that non-void functions have Return on every path.
    fn check_path_termination(&mut self, func: &Function) {
        // Skip void functions
        if func.return_types().is_empty() {
            return;
        }

        let entry = match func.entry_block {
            Some(e) => e,
            None => return,
        };

        // BFS from entry，收集所有可达块
        let mut visited = HashSet::new();
        let mut worklist = vec![entry];
        visited.insert(entry);

        while let Some(block) = worklist.pop() {
            if let Some(block_data) = func.dfg.blocks.get(block.0 as usize) {
                for succ in block_data.terminator.successors() {
                    if visited.insert(succ) {
                        worklist.push(succ);
                    }
                }
            }
        }

        // 可达且无后继的块必须终结于 Return（Unreachable 合法——显式死代码）
        for (block, _) in func.dfg.blocks() {
            if !visited.contains(&block) {
                continue; // unreachable blocks already reported
            }
            if let Some(block_data) = func.dfg.blocks.get(block.0 as usize) {
                let succs = block_data.terminator.successors();
                if succs.is_empty()
                    && !matches!(
                        block_data.terminator,
                        Terminator::Return { .. }
                    )
                    // 显式 unreachable（has_terminator）是合法死代码；
                    // 从未设置终结符的块（默认 Unreachable）是构建遗漏 → PathWithoutReturn
                    && !(matches!(
                        block_data.terminator,
                        Terminator::Unreachable
                    ) && block_data.has_terminator)
                {
                    self.errors
                        .push(VerifyError::PathWithoutReturn { last_block: block });
                }
            }
        }
    }
}

impl Default for Verifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::FunctionBuilder;
    use crate::inst_flags::InstFlags;
    use crate::opcode::IntCC;
    use crate::types::{FunctionSignature, TypeContext};
    use smallvec::SmallVec;

    #[test]
    fn test_verify_valid_function() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        assert!(verifier.verify(&func).is_ok());
    }

    #[test]
    fn test_verify_undefined_value() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();

        // Manually create an instruction with an undefined value
        let bad_val = Value(999);
        fb.func.dfg.make_inst(
            Opcode::Iadd,
            entry,
            smallvec::smallvec![bad_val, bad_val],
            SmallVec::new(),
            &[TypeId::I32],
            InstFlags::NONE,
        );

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&fb.func);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_branch_condition() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "x")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, params) = fb.create_entry_block();
        let then_blk = fb.create_block();
        let else_blk = fb.create_block();
        let (merge_blk, _mp) = fb.create_block_with_params(&[(ctx.i32_ty(), "r")]);
        {
            fb.switch_to_block(entry);
            let cond = fb.icmp(IntCC::SignedGreaterThan, params[0], params[0]);
            fb.branch(cond, then_blk, &[], else_blk, &[]);
        }
        {
            fb.switch_to_block(then_blk);
            fb.jump(merge_blk, &[params[0]]);
        }
        {
            fb.switch_to_block(else_blk);
            fb.jump(merge_blk, &[params[0]]);
        }
        {
            let merge_params = fb.func.dfg.block_param_values(merge_blk).to_vec();
            fb.switch_to_block(merge_blk);
            fb.ret(&[merge_params[0]]);
        }

        let mut verifier = Verifier::with_ctx(ctx.clone());
        assert!(verifier.verify(&fb.func).is_ok());
    }

    #[test]
    fn test_verify_type_mismatch_binary_ops() {
        let ctx = TypeContext::new();
        let sig =
            FunctionSignature::new(&[(ctx.i32_ty(), "x"), (ctx.f64_ty(), "y")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, params) =
            fb.create_block_with_params(&[(TypeId::I32, "x"), (TypeId::F64, "y")]);
        fb.switch_to_block(entry);
        // builder 的 iadd 在 upcast 阶段即拒绝 i32/f64 混合（upcast 返回 None → panic），
        // 无法用于构造非法 IR；直接 make_inst 绕过 builder 检查，
        // 由 verify 的 check_operand_types 检测操作数类型不匹配。
        fb.func.dfg.make_inst(
            Opcode::Iadd,
            entry,
            smallvec::smallvec![params[0], params[1]],
            SmallVec::new(),
            &[TypeId::I32],
            InstFlags::NONE,
        );
        fb.ret(&[params[0]]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(result.is_err(), "type mismatch should be detected");
    }

    /// select cond 非 bool、sext 同位宽、icmp 操作数非整型、load 地址非指针
    /// —— 新增语义检查应各自报对应错误。
    #[test]
    fn test_verify_opcode_semantics() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let a = fb.iconst_i32(1);
        let c = fb.iconst_i32(2);
        let f = fb.fconst(1.5f64.to_bits(), TypeId::F64);
        let f2 = fb.fconst(2.5f64.to_bits(), TypeId::F64);
        fb.ret(&[a]);

        // 1) select cond 用 i32 → SelectCondNotBool
        fb.func.dfg.make_inst(
            Opcode::Select,
            entry,
            smallvec::smallvec![a, a, c],
            SmallVec::new(),
            &[TypeId::I32],
            InstFlags::NONE,
        );
        // 2) sext i64 → i64（同位宽）→ ConversionBitWidthMismatch
        let i64v = fb.func.dfg.make_inst(
            Opcode::Iconst,
            entry,
            smallvec::smallvec![],
            smallvec::smallvec![Immediate::Const(fb.func.constants.insert_int(1, 64))],
            &[TypeId::I64],
            InstFlags::NONE,
        );
        let i64r = fb.func.dfg.inst_results(i64v)[0];
        fb.func.dfg.make_inst(
            Opcode::Sextend,
            entry,
            smallvec::smallvec![i64r],
            SmallVec::new(),
            &[TypeId::I64],
            InstFlags::NONE,
        );
        // 3) icmp 操作数 float → IcmpOperandNotInt
        let _ = f;
        let _ = f2;
        fb.func.dfg.make_inst(
            Opcode::Icmp {
                cond: crate::opcode::IntCC::SignedGreaterThan,
            },
            entry,
            smallvec::smallvec![f, f2],
            SmallVec::new(),
            &[TypeId::BOOL],
            InstFlags::NONE,
        );
        // 4) load 地址非指针（i32）→ LoadAddrNotPointer
        fb.func.dfg.make_inst(
            Opcode::Load,
            entry,
            smallvec::smallvec![a],
            SmallVec::new(),
            &[TypeId::I32],
            InstFlags::NONE,
        );

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let errs = verifier.verify(&fb.func).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, VerifyError::SelectCondNotBool { .. })),
            "select cond: {:?}",
            errs
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, VerifyError::ConversionBitWidthMismatch { .. })),
            "sext width: {:?}",
            errs
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, VerifyError::IcmpOperandNotInt { .. })),
            "icmp category: {:?}",
            errs
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, VerifyError::LoadAddrNotPointer { .. })),
            "load addr: {:?}",
            errs
        );
    }

    #[test]
    fn test_verify_operand_count_mismatch() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();

        // Manually create iadd with too few operands
        let v = fb.iconst_i32(1);
        let _inst = fb.func.dfg.make_inst(
            Opcode::Iadd,
            entry,
            smallvec::smallvec![v], // only 1 operand, but iadd expects 2
            SmallVec::new(),
            &[TypeId::I32],
            InstFlags::NONE,
        );
        // Add ret to avoid PathWithoutReturn error
        let r = fb.iconst_i32(0);
        fb.ret(&[r]);

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&fb.func);
        assert!(result.is_err(), "operand count mismatch should be detected");
    }

    #[test]
    fn test_verify_return_type_mismatch() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty(), ctx.f64_ty()]); // 2 returns
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]); // only 1 return value, but signature expects 2
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(result.is_err(), "return type mismatch should be detected");
    }

    #[test]
    fn test_verify_block_param_count_mismatch() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        let (target, _) = fb.create_block_with_params(&[(TypeId::I32, "x"), (TypeId::I32, "y")]); // 2 params

        fb.switch_to_block(entry);
        fb.jump(target, &[]); // 0 args → should be 2
        fb.switch_to_block(target);
        let r = fb.iconst_i32(0);
        fb.ret(&[r]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(
            result.is_err(),
            "block param count mismatch should be detected"
        );
    }

    #[test]
    fn test_verify_unreachable_block() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        let orphan = fb.create_block(); // created but never targeted
        // 孤儿块带指令（非空死块——P0-14 后只有近空死 merge 豁免，
        // 带指令的不可达块仍是错误）
        fb.switch_to_block(orphan);
        let dead1 = fb.iconst_i32(1);
        let dead2 = fb.iconst_i32(2);
        let _ = (dead1, dead2);
        fb.unreachable();

        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(result.is_err(), "unreachable block should be detected");
    }

    #[test]
    fn test_verify_vconst_size_mismatch() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        // <4 x f32> 应为 16 字节，恶意只给 8 字节（2 lane）——
        // 直接 emit 绕过 vconst_bytes 的 debug_assert 长度校验
        let ty = ctx.vector_ty(ctx.f32_ty(), 4);
        let cid = fb.func.constants.insert_vector(&[0u8; 8]);
        let v = fb.emit1(
            crate::Opcode::Vconst,
            vec![],
            vec![crate::Immediate::Const(cid)],
            ty,
            crate::InstFlags::NONE,
        );
        fb.ret(&[v]); // 类型不匹配？ret 检查会拦——先不 ret 类型，用 i32 返回
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(
            matches!(result, Err(ref e) if e.iter().any(|x| matches!(x, VerifyError::InvalidImmediate { .. }))),
            "vconst size mismatch should be reported, got: {result:?}"
        );
    }

    #[test]
    fn test_verify_fpext_wrong_direction() {
        // fpext 应扩展（src 位宽 < dst）；反向（double→float）报
        // ConversionBitWidthMismatch
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.f32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let d = fb.fconst(1.0f64.to_bits(), ctx.f64_ty());
        let v = fb.fpext(d, ctx.f32_ty()); // 错误方向：f64 → f32
        fb.ret(&[v]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(
            matches!(result, Err(ref e) if e.iter().any(|x| matches!(x, VerifyError::ConversionBitWidthMismatch { .. }))),
            "fpext wrong direction should be reported, got: {result:?}"
        );
    }

    #[test]
    fn test_verify_ptrtoint_src_not_ptr() {
        // ptrtoint 的源必须是指针；整数源报 ConversionBitWidthMismatch
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i64_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let a = fb.iconst(42, ctx.i64_ty());
        let v = fb.ptrtoint(a, ctx.i64_ty()); // 源是 i64 非指针
        fb.ret(&[v]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(
            matches!(result, Err(ref e) if e.iter().any(|x| matches!(x, VerifyError::ConversionBitWidthMismatch { .. }))),
            "ptrtoint with non-ptr src should be reported, got: {result:?}"
        );
    }

    #[test]
    fn test_verify_arith_flag_misuse() {
        // nsw 用在非 add/sub/mul/shl（如 and）→ InvalidImmediate；
        // exact 用在非 div/shift（如 add）→ InvalidImmediate
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let a = fb.iconst(1, ctx.i32_ty());
        let b = fb.iconst(2, ctx.i32_ty());
        let v = fb.emit1(
            crate::Opcode::Iadd,
            vec![a, b],
            vec![],
            ctx.i32_ty(),
            crate::InstFlags::EXACT, // exact 对 add 非法
        );
        fb.ret(&[v]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(
            matches!(result, Err(ref e) if e.iter().any(|x| matches!(x, VerifyError::InvalidImmediate { .. }))),
            "exact on add should be reported, got: {result:?}"
        );
    }

    #[test]
    fn test_verify_cmpxchg_bad_failure_ordering() {
        // cmpxchg 失败序不能是 release/acq_rel（LLVM LangRef）
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let p = fb.alloca(ctx.i32_ty(), 1);
        let c = fb.iconst(1, ctx.i32_ty());
        let n = fb.iconst(2, ctx.i32_ty());
        let v = fb.cmpxchg(
            p,
            c,
            n,
            crate::opcode::Ordering::AcquireRelease,
            crate::opcode::Ordering::AcquireRelease,
            false,
        );
        fb.ret(&[v]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(
            matches!(result, Err(ref e) if e.iter().any(|x| matches!(x, VerifyError::InvalidImmediate { .. }))),
            "cmpxchg release failure ordering should be reported, got: {result:?}"
        );
    }

    #[test]
    fn test_verify_result_count_mismatch() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        let addr = fb.stack_addr(0);
        fb.store(v, addr); // Store 无结果
        fb.ret(&[]);
        // 恶意：给 store 指令错误地附加一个结果值
        let store_id = {
            let insts = &fb.func.dfg.insts;
            insts
                .iter()
                .position(|i| matches!(i.opcode, Opcode::Store))
                .map(|p| Inst(p as u32))
                .unwrap()
        };
        fb.func.dfg.insts[store_id.0 as usize].results.push(v);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(result.is_err(), "result count mismatch should be detected");
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, VerifyError::ResultCountMismatch { .. })),
            "expected ResultCountMismatch, got: {errors:?}"
        );
    }

    #[test]
    fn test_verify_dominance_violation() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        let blk_a = fb.create_block();
        let blk_b = fb.create_block();

        // Set up proper CFG: entry → blk_a or blk_b
        fb.switch_to_block(entry);
        let c = fb.iconst_i32(1);
        fb.branch(c, blk_a, &[], blk_b, &[]);

        // Create a value in blk_a
        fb.switch_to_block(blk_a);
        let val_a = fb.iconst_i32(1);
        fb.jump(blk_b, &[]);

        fb.switch_to_block(blk_b);
        let r = fb.iconst_i32(0);
        fb.ret(&[r]);
        let mut func = fb.finish().expect("build");

        // Corrupt: use val_a in entry (defined in blk_a) — dominance violation
        func.dfg.make_inst(
            Opcode::Iadd,
            entry,
            smallvec::smallvec![val_a, val_a],
            SmallVec::new(),
            &[TypeId::I32],
            InstFlags::NONE,
        );

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(result.is_err(), "dominance violation should be detected");
    }

    #[test]
    fn test_verify_block_arg_type_mismatch_error() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.f64_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        // Target block expects f64 param
        let (target, _) = fb.create_block_with_params(&[(TypeId::F64, "x")]);

        fb.switch_to_block(entry);
        // Jump with i32 value — type mismatch: expect f64, found i32
        let val = fb.iconst_i32(1);
        fb.jump(target, &[val]);

        fb.switch_to_block(target);
        let r = fb.fconst_f64(0.0);
        fb.ret(&[r]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(
            result.is_err(),
            "block arg type mismatch should be detected"
        );
        // Verify the specific error
        if let Err(ref errors) = result {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, VerifyError::BlockArgTypeMismatch { .. })),
                "expected BlockArgTypeMismatch, got: {:?}",
                errors
            );
        }
    }

    #[test]
    fn test_verify_block_arg_type_match_pass() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        // Target block expects i32 param
        let (target, _) = fb.create_block_with_params(&[(TypeId::I32, "x")]);

        fb.switch_to_block(entry);
        let val = fb.iconst_i32(1);
        fb.jump(target, &[val]); // correct type

        fb.switch_to_block(target);
        fb.ret(&[val]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        // Correct param count and type should pass
        assert!(
            verifier.verify(&func).is_ok(),
            "correct jump args should pass"
        );
    }

    #[test]
    fn test_verify_return_value_type_mismatch() {
        // 签名返回 i32，但 ret 传 i64 → ReturnValueTypeMismatch
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let v = fb.iconst_i64(42);
        fb.ret(&[v]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let errs = verifier.verify(&func).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, VerifyError::ReturnValueTypeMismatch { .. })),
            "expected ReturnValueTypeMismatch, got: {:?}",
            errs
        );
    }

    #[test]
    fn test_verify_return_value_type_match_pass() {
        // 签名返回 i32，ret 传 i32 → 通过（类型匹配）
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        assert!(verifier.verify(&func).is_ok());
    }

    #[test]
    fn test_verify_inst_order_violation() {
        // 同块内 use 出现在定义之后（删除定义指令制造悬空 use）
        // → InstOrderViolation
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let a = fb.iconst_i32(1);
        let v1 = fb.iadd(a, a);
        let v2 = fb.iadd(v1, a);
        fb.ret(&[v2]);

        // 删除 v1 的定义指令 → v2 的 use 悬空（同块、定义在后）
        let def_inst = match fb.func.dfg.value_def(v1) {
            Some(ValueDef::Inst(i, _)) => *i,
            _ => panic!("v1 should be inst-defined"),
        };
        fb.func.dfg.remove_inst(def_inst);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let errs = verifier.verify(&func).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, VerifyError::InstOrderViolation { .. })),
            "expected InstOrderViolation, got: {:?}",
            errs
        );
    }

    #[test]
    fn test_verify_multiple_entry_blocks() {
        // Create a function with two blocks that have no predecessors
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        let floating = fb.create_block(); // never targeted by any block

        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);

        fb.switch_to_block(floating);
        let r1 = fb.iconst_i32(0);
        let r2 = fb.iconst_i32(1);
        let r = fb.iadd(r1, r2);
        fb.ret(&[r]);
        let func = fb.finish().expect("build");

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        // floating has no predecessors → should be detected as unreachable or multiple entry
        assert!(
            result.is_err(),
            "multiple entry-like blocks should be detected"
        );
    }

    #[test]
    fn test_verify_value_def_mismatch() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let mut func = fb.finish().expect("build");

        // Corrupt the DFG: add a value that points to a non-existent inst
        func.dfg.values.push(crate::dfg::ValueData {
            def: crate::dfg::ValueDef::Inst(Inst(99999), 0),
            ty: TypeId::I32,
        });

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(result.is_err(), "value def mismatch should be detected");
    }

    #[test]
    fn test_verify_jump_to_nonexistent_block() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        let real_blk = fb.create_block();
        fb.switch_to_block(entry);
        fb.jump(real_blk, &[]);
        fb.switch_to_block(real_blk);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let mut func = fb.finish().expect("build");

        // Corrupt: set Jump target to a non-existent block
        func.dfg.blocks[entry.0 as usize].terminator = Terminator::Jump {
            target: Block(999),
            args: SmallVec::new(),
            metadata: SmallVec::new(),
        };

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(
            result.is_err(),
            "jump to nonexistent block should be detected"
        );
    }

    #[test]
    fn test_verify_use_list_inconsistency() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (_entry, _) = fb.create_entry_block();
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let mut func = fb.finish().expect("build");

        // Corrupt: add a bogus use entry referencing a non-existent inst
        func.use_lists.record_inst(Inst(99999), &[v]);

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(result.is_err(), "use-list inconsistency should be detected");
        if let Err(ref errors) = result {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, VerifyError::UseListInconsistency { .. })),
                "expected UseListInconsistency, got: {:?}",
                errors
            );
        }
    }
}
