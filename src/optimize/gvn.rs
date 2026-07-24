//! 全局值编号 (GVN) pass。
//!
//! 使用支配树进行跨基本块的公共子表达式消除。
//! 如果一条指令与支配树中某个祖先块的指令完全等价（相同操作码、相同操作数、相同类型），
//! 则可以用祖先的结果替换当前结果。
//!
//! # 特性
//!
//! - **全局 CSE**: 跨基本块消除冗余计算（支配树作用域栈）
//! - **常量折叠**: GVN 过程中自动简化纯常量操作
//! - **Memory Load GVN**: 相同地址的 load 在无中间 store/kill 时被消除
//! - **交换律**: 自动规范化交换律操作的参数顺序
//!
//! # 算法
//!
//! 1. 构建支配树（使用 `Function::dominator_tree()`）
//! 2. 对支配树做前序遍历（DFS）
//! 3. 维护作用域栈（每个基本块一个作用域）
//! 4. 进入基本块 → push 新作用域
//! 5. 对每条指令：
//!    a. 尝试常量折叠 (所有操作数为 Iconst/Fconst)
//!    b. 计算表达式键（操作码 + 操作数 + 类型）
//!    c. Memory load: 检查地址是否被后续 store 杀死
//!    d. 从栈顶向下查找键
//!    e. 找到 → 替换为已有 Value，标记为 Nop
//!    f. 未找到 → 插入当前作用域
//! 6. 离开基本块 → pop 作用域
//!
//! # 与局部 CSE 的区别
//!
//! - CSE:  仅消除同一基本块内的重复计算
//! - GVN:  消除整个支配子树内的重复计算（CSE 的超集）
//!
//! # 示例
//!
//! ```ignore
//! // Before GVN:
//! entry:
//!     v2 = iadd v0, v1        // 计算 a+b
//!     branch cond, then, else
//! then:
//!     v3 = iadd v0, v1        // 重复! (v0,v1 未变)
//!     v4 = imul v3, v2
//! else:
//!     v5 = iadd v0, v1        // 重复!
//!     v6 = imul v5, v2
//!
//! // After GVN:
//! entry:
//!     v2 = iadd v0, v1
//!     branch cond, then, else
//! then:
//!     v4 = imul v2, v2        // v3 → v2
//! else:
//!     v6 = imul v2, v2        // v5 → v2
//! ```

use super::cse::ExprKey;
use crate::CompileError;
use crate::ir::*;
use crate::optimize::{OptimizationPass, PassResult};
use std::collections::HashMap;

/// 全局值编号 pass。子集局部 CSE，增加常量折叠和内存 load GVN。
#[derive(Default)]
pub struct GvnPass;

impl GvnPass {
    pub fn new() -> Self {
        Self
    }
}

impl OptimizationPass for GvnPass {
    fn name(&self) -> &'static str {
        "gvn"
    }

    fn description(&self) -> &'static str {
        "Global value numbering: eliminates duplicate computations across blocks using dominator tree, with constant folding and memory load GVN"
    }

    fn run_on_function(&self, func: &mut Function) -> Result<PassResult, CompileError> {
        global_value_numbering(func)
    }
}

/// 对单个函数执行全局值编号（含常量折叠和内存 load GVN）。
#[allow(deprecated)]
pub fn global_value_numbering(func: &mut Function) -> Result<PassResult, CompileError> {
    let n = func.blocks.len();
    if n == 0 {
        return Ok(PassResult::default());
    }

    // 获取支配树子节点 (dominator tree children)
    let dom_children = func.dominator_tree();
    // dom_children[b] = 块 b 直接支配的块列表

    let mut result = PassResult::default();
    let mut replacements: HashMap<Value, Value> = HashMap::new();
    // 作用域栈：栈顶是当前基本块的作用域
    let mut scopes: Vec<HashMap<ExprKey, Value>> = vec![];
    // 常量映射表：Value → 常量值（用于 GVN 过程中的常量折叠）
    let mut const_map: HashMap<Value, (Big, Type)> = HashMap::new();

    // 从 entry block 开始 DFS
    let entry_id = func.blocks[0].id;
    gvn_dfs(
        func,
        entry_id,
        &dom_children,
        &mut scopes,
        &mut replacements,
        &mut const_map,
        &mut result,
    );

    // 在所有指令中应用替换
    if !replacements.is_empty() {
        result.values_replaced += replacements.len();
        super::cse::apply_replacements(func, &replacements);
    }

    Ok(result)
}

/// 检查两个值是否指向同一内存位置（保守别名分析）。
///
/// 当前实现：仅当两个值是相同的 SSA Value 时才认为它们不别名。
/// 这是最保守但正确的方法。
#[allow(dead_code)]
fn may_alias(v1: Value, v2: Value) -> bool {
    v1 == v2
}

/// 尝试对指令做常量折叠。如果所有操作数都是已知常量，则求值指令。
/// 返回 `Some((result_value, const_big, const_ty))` 如果折叠成功。
fn try_const_fold(
    opcode: &Opcode,
    mapped_operands: &[Value],
    _ty: Type,
    const_map: &HashMap<Value, (Big, Type)>,
) -> Option<(Big, Type)> {
    // 收集所有操作数的常量值
    let const_ops: Vec<&(Big, Type)> = mapped_operands
        .iter()
        .map(|v| const_map.get(v))
        .collect::<Option<Vec<_>>>()?;

    match opcode {
        Opcode::Iadd => {
            let val = const_ops[0].0.clone() + const_ops[1].0.clone();
            Some((val, const_ops[0].1))
        }
        Opcode::Isub => {
            let val = const_ops[0].0.clone() - const_ops[1].0.clone();
            Some((val, const_ops[0].1))
        }
        Opcode::Imul => {
            let val = const_ops[0].0.clone() * const_ops[1].0.clone();
            Some((val, const_ops[0].1))
        }
        Opcode::Band => {
            let val = const_ops[0].0.clone() & const_ops[1].0.clone();
            Some((val, const_ops[0].1))
        }
        Opcode::Bor => {
            let val = const_ops[0].0.clone() | const_ops[1].0.clone();
            Some((val, const_ops[0].1))
        }
        Opcode::Bxor => {
            let val = const_ops[0].0.clone() ^ const_ops[1].0.clone();
            Some((val, const_ops[0].1))
        }
        Opcode::Udiv => {
            if const_ops[1].0.is_zero() {
                return None; // 除零 — 保守跳过
            }
            let val = const_ops[0].0.clone() / const_ops[1].0.clone();
            Some((val, const_ops[0].1))
        }
        Opcode::Sdiv => {
            if const_ops[1].0.is_zero() {
                return None;
            }
            // Sdiv: signed division (use truncated division for Big constant folding)
            let val = const_ops[0].0.clone() / const_ops[1].0.clone();
            Some((val, const_ops[0].1))
        }
        Opcode::Bnot => {
            // Bitwise NOT: Big doesn't implement Not; fall through
            None
        }
        _ => None, // 不支持的操作码
    }
}

/// 对支配树做 DFS 遍历，在基本块内执行 GVN（含常量折叠和内存 load GVN）。
fn gvn_dfs(
    func: &mut Function,
    block_id: BlockId,
    dom_children: &[Vec<BlockId>],
    scopes: &mut Vec<HashMap<ExprKey, Value>>,
    replacements: &mut HashMap<Value, Value>,
    const_map: &mut HashMap<Value, (Big, Type)>,
    result: &mut PassResult,
) {
    // 1. 进入基本块：push 新作用域
    scopes.push(HashMap::new());

    // 2. 处理当前块的所有指令
    let block = &mut func.blocks[block_id.0 as usize];
    for inst in block.instructions.iter_mut() {
        let inst_result = match inst.result {
            Some(v) => v,
            None => {
                // 无结果的指令：追踪 store（kill load 表达式）
                if matches!(inst.opcode, Opcode::Store) {
                    // Store 指令：清除作用域栈中所有 load 表达式
                    // 保守处理：store 到任意地址都 kill 所有 load
                    for scope in scopes.iter_mut() {
                        scope.retain(|key, _| {
                            key.opcode
                                != super::cse::opcode_discriminant(&Opcode::Load)
                        });
                    }
                }
                continue;
            }
        };

        // 处理 Iconst / Fconst：记录到常量映射表
        match &inst.opcode {
            Opcode::Iconst { index } => {
                if let Some(big) = func.constant_pool.get(*index) {
                    const_map.insert(inst_result, (big.clone(), inst.ty));
                }
                // Iconst/Fconst 本身不参与 GVN 查找（它们是叶子值）
                continue;
            }
            Opcode::Fconst { index } => {
                if let Some(big) = func.constant_pool.get(*index) {
                    const_map.insert(inst_result, (big.clone(), inst.ty));
                }
                continue;
            }
            _ => {}
        }

        // 跳过不可 GVN 的指令
        if !super::cse::is_cse_candidate(&inst.opcode) {
            continue;
        }

        // 计算表达式键（操作数已映射到替换后的值）
        let mapped_operands: Vec<Value> = inst
            .operands
            .iter()
            .map(|v| replacements.get(v).copied().unwrap_or(*v))
            .collect();

        // === 常量折叠 ===
        if !matches!(inst.opcode, Opcode::Load) {
            // 不用 const_map 中的值替换 load ops
            if let Some((folded_val, folded_ty)) =
                try_const_fold(&inst.opcode, &mapped_operands, inst.ty, const_map)
            {
                // 将当前指令替换为常量
                let index = func.constant_pool.insert(folded_val.clone());
                inst.opcode = Opcode::Iconst { index };
                inst.operands.clear();
                inst.ty = folded_ty;
                const_map.insert(inst_result, (folded_val, folded_ty));
                result.instructions_removed += 1;
                result.changed = true;
                continue;
            }
        }

        let key = super::cse::expr_key(&inst.opcode, &mapped_operands, inst.ty);

        // === Memory Load GVN ===
        if matches!(inst.opcode, Opcode::Load) {
            // Load 指令特殊处理 —— 只在前一个 load 未被 store kill 时才能 GVN
            // 如果作用域中有相同地址的 load，复用其值
            if let Some(&existing) = gvn_lookup(scopes, &key) {
                replacements.insert(inst_result, existing);
                inst.opcode = Opcode::Nop;
                inst.operands.clear();
                inst.ty = Type::Void;
                inst.result = None;
                result.instructions_removed += 1;
                result.changed = true;
                continue;
            }
            // 首次出现，记录（但可能会被后续 store kill）
            scopes.last_mut().unwrap().insert(key, inst_result);
            continue;
        }

        // 从栈顶向下查找（扫描所有祖先作用域）
        if let Some(&existing) = gvn_lookup(scopes, &key) {
            // 在祖先作用域中找到了等价表达式
            replacements.insert(inst_result, existing);
            // 传递常量信息
            if let Some(const_val) = const_map.get(&existing).cloned() {
                const_map.insert(inst_result, const_val);
            }
            inst.opcode = Opcode::Nop;
            inst.operands.clear();
            inst.ty = Type::Void;
            inst.result = None;
            result.instructions_removed += 1;
            result.changed = true;
        } else {
            // 首次出现，插入当前作用域
            scopes.last_mut().unwrap().insert(key, inst_result);
        }
    }

    // 3. 递归遍历支配树子节点
    let children: Vec<BlockId> = dom_children
        .get(block_id.0 as usize)
        .cloned()
        .unwrap_or_default();
    for child_id in children {
        gvn_dfs(
            func,
            child_id,
            dom_children,
            scopes,
            replacements,
            const_map,
            result,
        );
    }

    // 4. 离开基本块：pop 作用域
    scopes.pop();
}

/// 在作用域栈中从上到下查找表达式键。
///
/// 返回第一个匹配的 Value（最近的支配定义）。
fn gvn_lookup<'a>(scopes: &'a [HashMap<ExprKey, Value>], key: &ExprKey) -> Option<&'a Value> {
    for scope in scopes.iter().rev() {
        if let Some(v) = scope.get(key) {
            return Some(v);
        }
    }
    None
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::OptimizationPass;

    #[test]
    fn gvn_cross_block_duplicate() {
        // entry:
        //   v2 = iadd v0, v1
        //   branch → then
        // then:
        //   v3 = iadd v0, v1  ← duplicate! should be replaced by v2
        //   return v3
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        let then_block = b.create_block();

        b.switch_to_block(entry);
        let a = params[0];
        let b_val = params[1];
        let _sum_entry = b.iadd(a, b_val);
        b.jump(then_block, &[]);

        b.switch_to_block(then_block);
        let sum_then = b.iadd(a, b_val); // same as sum_entry
        b.return_(&[sum_then]);

        let mut func = b.finish();
        eprintln!("Before GVN:\n{}", func);

        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        eprintln!("After GVN:\n{}", func);
        assert!(r.changed);
        assert!(r.instructions_removed >= 1);

        // sum_then should be replaced by sum_entry
        // check: return should use the same value as sum_entry
        let validation = func.validate();
        assert!(
            validation.is_valid(),
            "Validation errors: {:?}",
            validation.errors
        );
    }

    #[test]
    fn gvn_diamond_reuse() {
        // entry:
        //   v0 = iadd a, b      // compute once
        //   branch cond, left, right
        // left:
        //   v1 = iadd a, b      // duplicate
        //   jump merge
        // right:
        //   v2 = iadd a, b      // duplicate
        //   jump merge
        // merge:
        //   v3 = iadd a, b      // duplicate
        //   return v3
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        let left = b.create_block();
        let right = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let a = params[0];
        let b_val = params[1];
        let _v0 = b.iadd(a, b_val);
        let c_one = b.iconst_i32(1);
        let cmp = b.icmp(IntCC::SignedGreaterThan, a, c_one);
        b.branch(cmp, left, right, &[], &[]);

        b.switch_to_block(left);
        let _v1 = b.iadd(a, b_val);
        b.jump(merge, &[]);

        b.switch_to_block(right);
        let _v2 = b.iadd(a, b_val);
        b.jump(merge, &[]);

        b.switch_to_block(merge);
        let _v3 = b.iadd(a, b_val);
        b.return_(&[_v3]);

        let mut func = b.finish();
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        // All three duplicates should be eliminated
        assert!(r.instructions_removed >= 1);
        let validation = func.validate();
        assert!(
            validation.is_valid(),
            "Validation errors: {:?}",
            validation.errors
        );
    }

    #[test]
    fn gvn_preserves_side_effects() {
        // Stores should NOT be GVN'd
        let sig = Signature::new(&[(Type::Ptr, "p"), (Type::I32, "a"), (Type::I32, "b")], &[]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) =
            b.create_block_with_params(&[(Type::Ptr, "p"), (Type::I32, "a"), (Type::I32, "b")]);
        let then_block = b.create_block();

        b.switch_to_block(entry);
        let p = params[0];
        let a = params[1];
        let b_val = params[2];
        let sum = b.iadd(a, b_val);
        b.store(sum, p);
        b.jump(then_block, &[]);

        b.switch_to_block(then_block);
        let sum2 = b.iadd(a, b_val); // This is GVN-able (pure computation)
        b.store(sum2, p); // store is side-effecting but sum2 computation can be GVN'd
        b.return_(&[]);

        let mut func = b.finish();
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        eprintln!("After GVN:\n{}", func);
        assert!(r.changed); // iadd in then_block should be GVN'd
        let validation = func.validate();
        assert!(
            validation.is_valid(),
            "Validation errors: {:?}",
            validation.errors
        );
    }

    #[test]
    fn gvn_no_crash_on_loop() {
        // GVN should handle loops correctly (back edges are not in dominator tree)
        let sig = Signature::new(&[(Type::I32, "n")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "n")]);
        let (body, body_params) = b.create_block_with_params(&[(Type::I32, "i")]);
        let exit = b.create_block();

        b.switch_to_block(entry);
        let zero = b.iconst_i32(0);
        b.jump(body, &[zero]);

        b.switch_to_block(body);
        let i = body_params[0];
        let inc = b.iconst_i32(1);
        let next = b.iadd(i, inc);
        let cmp = b.icmp(IntCC::SignedLessThan, next, params[0]);
        b.branch(cmp, body, exit, &[next], &[]);

        b.switch_to_block(exit);
        b.return_(&[next]);

        let mut func = b.finish();
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        // Should not crash and should remain valid
        let validation = func.validate();
        assert!(
            validation.is_valid(),
            "Validation errors: {:?}",
            validation.errors
        );
        // It's OK if GVN finds or doesn't find duplicates in a loop
        let _ = r;
    }

    #[test]
    fn gvn_chain_cross_block() {
        // entry: v1 = mul a, c
        // block1: v2 = mul a, c  (dup of v1)
        // block2: v3 = mul a, c  (also dup of v1, block2 dominated by block1 dominated by entry)
        let sig = Signature::new(&[(Type::I32, "a")], &[Type::I32, Type::I32, Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "a")]);
        let block1 = b.create_block();
        let block2 = b.create_block();

        b.switch_to_block(entry);
        let a = params[0];
        let c = b.iconst_i32(10);
        let m1 = b.imul(a, c);
        b.jump(block1, &[]);

        b.switch_to_block(block1);
        let m2 = b.imul(a, c);
        b.jump(block2, &[]);

        b.switch_to_block(block2);
        let m3 = b.imul(a, c);
        b.return_(&[m1, m2, m3]);

        let mut func = b.finish();
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();

        assert!(r.changed);
        // All 3 mul should collapse to m1, eliminating m2 and m3
        assert!(r.instructions_removed >= 2);
        let validation = func.validate();
        assert!(
            validation.is_valid(),
            "Validation errors: {:?}",
            validation.errors
        );
    }

    #[test]
    fn gvn_commutative_elimination() {
        // a + b  and  b + a  should be recognized as equivalent
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        b.switch_to_block(entry);
        let a = params[0];
        let b_val = params[1];
        let sum1 = b.iadd(a, b_val); // a + b
        let sum2 = b.iadd(b_val, a); // b + a — commutative duplicate!
        let result = b.imul(sum1, sum2);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Commutative add should be GVN'd");
        assert!(func.validate().is_valid());
    }

    #[test]
    fn gvn_commutative_mul() {
        // a * b  and  b * a  should be recognized as equivalent
        let sig = Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "a"), (Type::I32, "b")]);
        b.switch_to_block(entry);
        let a = params[0];
        let b_val = params[1];
        let p1 = b.imul(a, b_val); // a * b
        let p2 = b.imul(b_val, a); // b * a — commutative duplicate!
        let sum = b.iadd(p1, p2); // use both to ensure replacements work
        b.return_(&[sum]);

        let mut func = b.finish();
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Commutative mul should be GVN'd");
        assert!(func.validate().is_valid());
    }

    // ============================================================
    // GVN + 常量折叠测试
    // ============================================================

    #[test]
    fn gvn_const_fold_add() {
        // v1 = iconst(3), v2 = iconst(5), v3 = iadd(v1, v2) → should become iconst(8)
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c3 = b.iconst_i32(3);
        let c5 = b.iconst_i32(5);
        let sum = b.iadd(c3, c5);
        b.return_(&[sum]);

        let mut func = b.finish();
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Constant add should be folded");
        assert!(func.validate().is_valid());

        // sum should now be Iconst(8)
        let mut found = false;
        for inst in &func.blocks[0].instructions {
            if let Opcode::Iconst { index } = &inst.opcode
                && let Some(big) = func.constant_pool.get(*index)
                    && big.try_to_i64() == Some(8) {
                        found = true;
                    }
        }
        assert!(found, "Expected Iconst(8) in output");
    }

    #[test]
    fn gvn_const_fold_mul() {
        // v1 = iconst(4), v2 = iconst(7), v3 = imul(v1, v2) → iconst(28)
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c4 = b.iconst_i32(4);
        let c7 = b.iconst_i32(7);
        let prod = b.imul(c4, c7);
        b.return_(&[prod]);

        let mut func = b.finish();
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Constant mul should be folded");
        assert!(func.validate().is_valid());

        let mut found = false;
        for inst in &func.blocks[0].instructions {
            if let Opcode::Iconst { index } = &inst.opcode
                && let Some(big) = func.constant_pool.get(*index)
                    && big.try_to_i64() == Some(28) {
                        found = true;
                    }
        }
        assert!(found, "Expected Iconst(28) in output");
    }

    #[test]
    fn gvn_const_fold_bitwise() {
        // v1 = iconst(0xFF), v2 = iconst(0x0F), v3 = band(v1, v2) → iconst(0x0F)
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c1 = b.iconst_i32(0xFF);
        let c2 = b.iconst_i32(0x0F);
        let result = b.band(c1, c2);
        b.return_(&[result]);

        let mut func = b.finish();
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Constant bitwise and should be folded");
        assert!(func.validate().is_valid());
    }

    #[test]
    fn gvn_const_fold_chain() {
        // c1 = iconst(2), c2 = iconst(3), c3 = iconst(4)
        // t1 = iadd(c1, c2) → 5
        // t2 = imul(t1, c3) → 20   (uses folded constant)
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let c2 = b.iconst_i32(2);
        let c3 = b.iconst_i32(3);
        let c4 = b.iconst_i32(4);
        let t1 = b.iadd(c2, c3);
        let t2 = b.imul(t1, c4);
        b.return_(&[t2]);

        let mut func = b.finish();
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Constant chain should be folded");
        assert!(func.validate().is_valid());

        let mut found_20 = false;
        for inst in &func.blocks[0].instructions {
            if let Opcode::Iconst { index } = &inst.opcode
                && let Some(big) = func.constant_pool.get(*index)
                    && big.try_to_i64() == Some(20) {
                        found_20 = true;
                    }
        }
        assert!(found_20, "Expected Iconst(20) from chain folding");
    }

    // ============================================================
    // Memory Load GVN 测试
    // ============================================================

    #[test]
    fn gvn_load_elimination_no_store() {
        // Load from same address twice without intervening store → second load eliminated
        let sig = Signature::new(&[(Type::Ptr, "p")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::Ptr, "p")]);
        b.switch_to_block(entry);
        let p = params[0];
        let v1 = b.load(p, Type::I32);
        let v2 = b.load(p, Type::I32); // same address — should be GVN'd
        let sum = b.iadd(v1, v2);
        b.return_(&[sum]);

        let mut func = b.finish();
        let pass = GvnPass::new();
        let r = pass.run_on_function(&mut func).unwrap();
        assert!(r.changed, "Duplicate load should be GVN'd");
        assert!(func.validate().is_valid());
    }

    #[test]
    fn gvn_load_not_eliminated_after_store() {
        // Load, Store (to same address), Load — second load should NOT be eliminated
        // because the store may have changed the value
        let sig = Signature::new(&[(Type::Ptr, "p"), (Type::I32, "v")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) =
            b.create_block_with_params(&[(Type::Ptr, "p"), (Type::I32, "v")]);
        b.switch_to_block(entry);
        let p = params[0];
        let v = params[1];
        let _v1 = b.load(p, Type::I32);
        b.store(v, p); // store to same address — kills load GVN
        let v2 = b.load(p, Type::I32); // this load should NOT be eliminated
        b.return_(&[v2]);

        let mut func = b.finish();
        // Count loads before GVN
        let load_count_before = func.blocks[0]
            .instructions
            .iter()
            .filter(|i| matches!(i.opcode, Opcode::Load))
            .count();
        let pass = GvnPass::new();
        let _r = pass.run_on_function(&mut func).unwrap();
        // Both loads should remain since store interleaves
        let load_count_after = func.blocks[0]
            .instructions
            .iter()
            .filter(|i| matches!(i.opcode, Opcode::Load))
            .count();
        assert_eq!(
            load_count_before, load_count_after,
            "Loads should not be eliminated after intervening store"
        );
        assert!(func.validate().is_valid());
    }

    #[test]
    fn gvn_load_elimination_different_address() {
        // Load from p1, load from p2 — different addresses, both should remain
        let sig = Signature::new(&[(Type::Ptr, "p1"), (Type::Ptr, "p2")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) =
            b.create_block_with_params(&[(Type::Ptr, "p1"), (Type::Ptr, "p2")]);
        b.switch_to_block(entry);
        let p1 = params[0];
        let p2 = params[1];
        let v1 = b.load(p1, Type::I32);
        let v2 = b.load(p2, Type::I32); // different address
        b.return_(&[v1, v2]);

        let mut func = b.finish();
        let load_count_before = func.blocks[0]
            .instructions
            .iter()
            .filter(|i| matches!(i.opcode, Opcode::Load))
            .count();
        assert_eq!(load_count_before, 2);
        let pass = GvnPass::new();
        let _r = pass.run_on_function(&mut func).unwrap();
        let load_count_after = func.blocks[0]
            .instructions
            .iter()
            .filter(|i| matches!(i.opcode, Opcode::Load))
            .count();
        // Different addresses should keep both loads
        assert_eq!(load_count_after, 2);
        assert!(func.validate().is_valid());
    }
}
