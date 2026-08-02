//! IR 验证器 — 检查 SSA 属性、类型一致性、CFG 完整性。

use super::analysis::DominatorTree;
use super::dfg::{DataFlowGraph, ValueDef};
use super::entity::*;
use super::function::Function;
use super::opcode::Opcode;
use super::terminator::Terminator;
use super::types::{TypeContext, TypeStore};
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
        details: Vec<String>,
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
                    details.first().map(|s| s.as_str()).unwrap_or("unknown")
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

    /// Backward compat: wrap a TypeStore in TypeContext.
    #[deprecated(note = "use Verifier::with_ctx instead")]
    pub fn with_store(store: TypeStore) -> Self {
        Self::with_ctx(TypeContext::from_store(store))
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
        self.check_use_lists(func);
        self.check_block_params(&func.dfg, &func.return_tys);
        self.check_terminators(&func.dfg);
        self.check_reachability(func);
        self.check_value_defs(&func.dfg);
        self.check_dominance(func);
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
        // that aren't the designated entry block.
        let mut no_pred_blocks: Vec<Block> = Vec::new();
        for (block, _) in func.dfg.blocks() {
            let has_preds = func
                .dfg
                .blocks()
                .any(|(_, bd)| bd.terminator.successors().contains(&block));
            if !has_preds {
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
                // Skip variable-count opcodes (Call, CallIndirect, GEP, ShuffleVector)
                match instruction.opcode {
                    Opcode::Call
                    | Opcode::CallIndirect
                    | Opcode::GetElementPtr
                    | Opcode::ShuffleVector => continue,
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

    fn check_block_params(&mut self, dfg: &DataFlowGraph, signature_rets: &[TypeId]) {
        for (block, block_data) in dfg.blocks() {
            let expected_params = block_data.params.len();

            // Check all Jump/Branch targets pointing to this block
            for (pred, pred_data) in dfg.blocks() {
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
                    Terminator::Jump { target, args } if *target == block => {
                        if args.len() != expected_params {
                            self.errors.push(VerifyError::BlockParamCountMismatch {
                                block: pred,
                                expected: expected_params,
                                found: args.len(),
                            });
                        } else {
                            Self::check_arg_types(&mut self.errors, dfg, *target, args, block_data);
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

            // Check Return value counts match signature (always, not just non-void)
            if let Terminator::Return { values } = &block_data.terminator
                && values.len() != signature_rets.len()
            {
                self.errors.push(VerifyError::ReturnTypeMismatch {
                    block,
                    expected: signature_rets.len(),
                    found: values.len(),
                });
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
            // All blocks must have a non-default terminator
            // (Unreachable is allowed as an explicit terminator)
            match &block_data.terminator {
                Terminator::Jump { target, .. } if dfg.blocks.get(target.0 as usize).is_none() => {
                    self.errors.push(VerifyError::MissingTerminator { block });
                }
                Terminator::Branch {
                    then_block,
                    else_block,
                    ..
                } if (dfg.blocks.get(then_block.0 as usize).is_none()
                    || dfg.blocks.get(else_block.0 as usize).is_none()) =>
                {
                    self.errors.push(VerifyError::MissingTerminator { block });
                }
                Terminator::Switch {
                    default_block,
                    cases,
                    ..
                } => {
                    if dfg.blocks.get(default_block.0 as usize).is_none() {
                        self.errors.push(VerifyError::MissingTerminator { block });
                    }
                    for (_, target, _) in cases.iter() {
                        if dfg.blocks.get(target.0 as usize).is_none() {
                            self.errors.push(VerifyError::MissingTerminator { block });
                            break;
                        }
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

        for (block, _) in func.dfg.blocks() {
            if !reachable.contains(&block) {
                self.errors.push(VerifyError::UnreachableBlock { block });
            }
        }
    }

    /// Check that every ValueDef is consistent with the DFG state.
    /// - ValueDef::Inst(i, idx): i exists and idx < opcode.result_count()
    /// - ValueDef::Param(b, idx): b exists and idx < block.params.len()
    fn check_value_defs(&mut self, dfg: &DataFlowGraph) {
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
                        self.errors.push(VerifyError::ValueDefMismatch {
                            value,
                            expected_def: format!("inst {} result {}", inst, result_idx),
                            found_def: "inst is deleted (Nop)".to_string(),
                        });
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
            }
        }
    }

    /// Check that every use of a value is dominated by its definition.
    fn check_dominance(&mut self, func: &Function) {
        let entry = match func.entry_block {
            Some(e) => e,
            None => return,
        };

        // Build dominator tree (uses Function's lazy cache)
        let domtree = DominatorTree::build(func);
        if domtree.block_count() == 0 {
            return;
        }

        // For each instruction, check each operand
        for (inst, instruction) in func.dfg.insts() {
            let user_block = instruction.block;
            for &operand in &instruction.operands {
                // Find the defining block
                let def_block = match func.dfg.value_def(operand) {
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

        // Check terminator values
        for (block, block_data) in func.dfg.blocks() {
            for val in block_data.terminator.used_values() {
                let def_block = match func.dfg.value_def(val) {
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
                    self.errors.push(VerifyError::DominanceViolation {
                        value: val,
                        user: Inst(u32::MAX), // terminator reference
                        user_block: block,
                        def_block,
                    });
                }
            }
        }
    }

    /// Check that non-void functions have Return on every path.
    fn check_path_termination(&mut self, func: &Function) {
        // Skip void functions
        if func.return_tys.is_empty() {
            return;
        }

        let entry = match func.entry_block {
            Some(e) => e,
            None => return,
        };

        // BFS from entry, track blocks without successors
        let mut visited = HashSet::new();
        let mut worklist = vec![entry];
        visited.insert(entry);

        while let Some(block) = worklist.pop() {
            if let Some(block_data) = func.dfg.blocks.get(block.0 as usize) {
                let succs = block_data.terminator.successors();
                if succs.is_empty() {
                    // No successors — must be Return (or Unreachable)
                    if !matches!(
                        block_data.terminator,
                        Terminator::Return { .. } | Terminator::Unreachable
                    ) {
                        // This shouldn't happen since Jump/Branch/Switch all have successors,
                        // but check defensively
                    } else if matches!(block_data.terminator, Terminator::Unreachable) {
                        continue; // Unreachable is always valid
                    }
                    // Return is valid
                }
                for succ in succs {
                    if visited.insert(succ) {
                        worklist.push(succ);
                    }
                }
            }
        }

        // Now find all reachable blocks with no successors that aren't Return
        for (block, _) in func.dfg.blocks() {
            if !visited.contains(&block) {
                continue; // unreachable blocks already reported
            }
            if let Some(block_data) = func.dfg.blocks.get(block.0 as usize) {
                let succs = block_data.terminator.successors();
                if succs.is_empty()
                    && !matches!(
                        block_data.terminator,
                        Terminator::Return { .. } | Terminator::Unreachable
                    )
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
        let mut b = fb.build(entry);
        let v = b.iconst_i32(42);
        b.ret(&[v]);
        let func = fb.finish();

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
            let mut b = fb.build(entry);
            let cond = b.icmp(IntCC::SignedGreaterThan, params[0], params[0]);
            b.branch(cond, then_blk, &[], else_blk, &[]);
        }
        {
            let mut b = fb.build(then_blk);
            b.jump(merge_blk, &[params[0]]);
        }
        {
            let mut b = fb.build(else_blk);
            b.jump(merge_blk, &[params[0]]);
        }
        {
            let merge_params = fb.func.dfg.block_param_values(merge_blk).to_vec();
            let mut b = fb.build(merge_blk);
            b.ret(&[merge_params[0]]);
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
        // iadd with i32 and f64 — type mismatch
        let sum = fb.iadd(params[0], params[1]);
        fb.ret(&[sum]);
        let func = fb.finish();

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(result.is_err(), "type mismatch should be detected");
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
        let func = fb.finish();

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
        let func = fb.finish();

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
        let _orphan = fb.create_block(); // created but never targeted

        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let func = fb.finish();

        let mut verifier = Verifier::with_ctx(ctx.clone());
        let result = verifier.verify(&func);
        assert!(result.is_err(), "unreachable block should be detected");
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
        let mut func = fb.finish();

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
        let func = fb.finish();

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
        let func = fb.finish();

        let mut verifier = Verifier::with_ctx(ctx.clone());
        // Correct param count and type should pass
        assert!(
            verifier.verify(&func).is_ok(),
            "correct jump args should pass"
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
        let r = fb.iconst_i32(0);
        fb.ret(&[r]);
        let func = fb.finish();

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
        let mut func = fb.finish();

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
        let mut func = fb.finish();

        // Corrupt: set Jump target to a non-existent block
        func.dfg.blocks[entry.0 as usize].terminator = Terminator::Jump {
            target: Block(999),
            args: SmallVec::new(),
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
        let mut func = fb.finish();

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
