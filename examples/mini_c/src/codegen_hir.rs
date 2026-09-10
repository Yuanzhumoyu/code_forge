//! HIR-based code generation — the forge-hir backend.
//!
//! Mirrors the logic in `codegen.rs` (the direct FunctionBuilder backend),
//! but builds an [`IrGraph`] with the unified [`HirCtx`] context and lowers
//! it in one step via `lower_into_module`.
//!
//! # Differences from codegen.rs
//! - Values are [`GraphValue`] (IrGraph handles) instead of `forge_ir::Value`.
//! - Every lowering function takes `&mut HirCtx` — one mutable borrow for the
//!   graph + symbol table + loop stack + inlining state (no field-splitting).
//! - Ops are emitted through the type-safe `build_xxx` constructors generated
//!   by `define_lowering!` (op catalog), not through raw node creation.
//! - Function parameters arrive as entry-block params and are stored to stack
//!   slots, exactly like the direct backend.

use crate::atoms::minic_lowering;
use code_forge::forge_grammar::{AstRef, TypedAst};
use code_forge::ir::{FuncRef, FunctionSignature, IntCC, TypeId};
use forge_hir::{
    BlockId, BrickRegistry, GraphValue, HirCtx, HirError, IrGraph, LoopFrame, lower_into_module,
};
use std::collections::HashMap;

// Re-use SymTable from codegen.rs (it's pub)
use crate::codegen::SymTable;

// ============================================================
// Entry point: compile a single function
// ============================================================

/// Lower one `func_def` AST node into a forge-ir `Function` via the HIR
/// pipeline, add it to `module`, and return its `FuncRef`.
pub fn codegen_function_hir(
    module: &mut code_forge::ir::Module,
    func_node: AstRef<'_>,
    syms: &mut SymTable,
    ast: &TypedAst,
    source: &str,
) -> Result<FuncRef, String> {
    let name = func_node
        .get_text("name")
        .ok_or("func_def missing name")?
        .to_string();

    // Collect parameters
    let params: Vec<(TypeId, String)> = func_node
        .get_children("params")
        .iter()
        .map(|p| {
            let pname = p.get_text("name").unwrap_or("_").to_string();
            (TypeId::I32, pname)
        })
        .collect();

    // All mini_c functions return int
    let sig = FunctionSignature::new(
        &params
            .iter()
            .map(|(t, n)| (*t, n.as_str()))
            .collect::<Vec<_>>(),
        &[TypeId::I32],
    );

    // Build the graph; the entry block carries the function parameters.
    let mut graph = IrGraph::new();
    let entry_params: Vec<(TypeId, &str)> = params.iter().map(|(t, n)| (*t, n.as_str())).collect();
    let entry = graph.create_block(&entry_params);
    graph.set_current_block(entry);

    let entry_values: Vec<GraphValue> = graph
        .block_params(entry)
        .map(|ps| ps.to_vec())
        .unwrap_or_default();

    // Phase 1: codegen the body into the IrGraph
    {
        let mut ctx = HirCtx::new(&mut graph, ast, source, syms);
        // 当前函数视为"已内联"——函数体内调用自身 = 递归（HIR 无真实 Call
        // 支持，明确拒绝而非无限内联栈溢出）。
        ctx.inlining.push(name.clone());
        ctx.next_offset = -4; // first param slot at -4

        // Store each parameter into a stack slot, then register the slot.
        for (i, (_, pname)) in params.iter().enumerate() {
            let slot = ctx.alloc_slot().map_err(|e| e.to_string())?;
            let pv = entry_values
                .get(i)
                .copied()
                .ok_or_else(|| format!("entry param {} missing", i))?;
            minic_lowering::build_store(ctx.graph, pv, slot).map_err(|e| e.to_string())?;
            ctx.locals.insert(pname.clone(), slot);
        }

        let body = func_node.get_child("body").ok_or("func_def missing body")?;
        lower_block(&mut ctx, body).map_err(|e| e.to_string())?;
    } // ctx dropped — mutable borrow on graph ends

    // Implicit return 0 if the last block isn't terminated
    if let Some(cur) = graph.current_block()
        && !graph.has_terminator(cur)
    {
        let v = graph.iconst_i32(0).map_err(|e| e.to_string())?;
        graph.emit_ret(v).map_err(|e| e.to_string())?;
    }

    // Phase 2: lower the graph into the module (registry via define_lowering!)
    let mut registry = BrickRegistry::new();
    minic_lowering::register_atoms(&mut registry);

    lower_into_module(&graph, &registry, module, &name, sig).map_err(|e| e.to_string())
}

// ============================================================
// Block / statement lowering
// ============================================================

fn lower_block(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    // 诊断升级：记录块节点位置；错误经 locate 附加源位置
    ctx.set_span(node);
    let result = (|| {
        for item in node.get_children("items") {
            let cur = ctx
                .graph
                .current_block()
                .ok_or_else(|| HirError::Internal("no current block".into()))?;
            if ctx.graph.has_terminator(cur) {
                break;
            }
            lower_stmt(ctx, item)?;
        }
        Ok(())
    })();
    result.map_err(|e| ctx.locate(e))
}

fn lower_stmt(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    ctx.set_span(node);
    let result = match node.kind() {
        "return_stmt" => lower_return(ctx, node),
        "if_stmt" => lower_if(ctx, node),
        "while_stmt" => lower_while(ctx, node),
        "for_stmt" => lower_for(ctx, node),
        "do_while_stmt" => lower_do_while(ctx, node),
        "var_decl" => lower_var_decl(ctx, node),
        "assign_stmt" => lower_assign(ctx, node),
        "expr_stmt" => {
            let expr_node = node
                .get_child("expr")
                .ok_or_else(|| HirError::Lowering("expr_stmt missing expr".into()))?;
            lower_expr(ctx, expr_node)?; // discard result
            Ok(())
        }
        "break_stmt" => lower_break(ctx),
        "continue_stmt" => lower_continue(ctx),
        "for_init" => lower_for_init(ctx, node),
        "for_update" => lower_for_update(ctx, node),
        "enum_def" => Ok(()), // pre-scanned in compiler.rs
        "struct_def" => lower_struct_def(ctx, node),
        "struct_decl" => lower_struct_decl(ctx, node),
        "struct_init" => lower_struct_init(ctx, node),
        other => Err(HirError::Lowering(format!("unknown stmt: {}", other))),
    };
    result.map_err(|e| ctx.locate(e))
}

// ── return_stmt ──

fn lower_return(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    let v = match node.get_optional("value") {
        Some(Some(expr_node)) => lower_expr(ctx, expr_node)?,
        _ => ctx.graph.iconst_i32(0)?,
    };

    // Inlining mode: store the return value and jump to the after-block.
    if let (Some(ret_slot), Some(after_blk)) = (ctx.return_slot, ctx.return_block) {
        minic_lowering::build_store(ctx.graph, v, ret_slot)?;
        ctx.graph.emit_jump(after_blk)?;
    } else {
        ctx.graph.emit_ret(v)?;
    }
    Ok(())
}

// ── var_decl ──

fn lower_var_decl(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    let name = node.get_text("name").unwrap_or("_").to_string();
    let slot = ctx.alloc_slot()?;

    if let Some(Some(init_node)) = node.get_optional("init") {
        let val = lower_expr(ctx, init_node)?;
        minic_lowering::build_store(ctx.graph, val, slot)?;
    }
    ctx.locals.insert(name, slot);
    Ok(())
}

// ── assign_stmt (plain + member access + compound ops) ──

fn lower_assign(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    // Check if LHS is a member_access (struct field assignment)
    let member_access_node = node
        .children()
        .iter()
        .find(|c| c.kind() == "member_access")
        .copied();

    if let Some(ma_node) = member_access_node {
        let val_node = node
            .get_child("value")
            .ok_or_else(|| HirError::Lowering("assign missing value".into()))?;
        let rhs = lower_expr(ctx, val_node)?;

        let ma_idents = ma_node.get_children("idents");
        let obj_name = ma_idents.first().and_then(|c| c.text()).unwrap_or("_");
        let field_name = ma_idents.get(1).and_then(|c| c.text()).unwrap_or("_");

        // Each field has its own slot: obj.field
        let field_slot = ctx.lookup(&field_key(obj_name, field_name))?;

        let op = extract_assign_op(ctx.source, node);
        let result = apply_compound_op(ctx, &op, field_slot, rhs)?;
        minic_lowering::build_store(ctx.graph, result, field_slot)?;
        return Ok(());
    }

    // Plain variable assignment: x = expr
    let name = node.get_text("name").unwrap_or("_");
    let slot = ctx.lookup(name)?;
    let val_node = node
        .get_child("value")
        .ok_or_else(|| HirError::Lowering("assign missing value".into()))?;
    let rhs = lower_expr(ctx, val_node)?;

    let op = extract_assign_op(ctx.source, node);
    let result = apply_compound_op(ctx, &op, slot, rhs)?;
    minic_lowering::build_store(ctx.graph, result, slot)?;
    Ok(())
}

/// 结构体字段的槽键：`"对象.字段"`（每字段独立栈槽的命名约定）。
///
/// 计划收缩点 3（`docs/archive/hir-shrink-plan.md`，修正版）：该字符串格式旧实现
/// 在 5 处各自 `format!`——收敛为一个命名约定函数（`lower_struct_init` /
/// `lower_member_access` / 成员赋值共用）。
fn field_key(base: &str, field: &str) -> String {
    format!("{base}.{field}")
}

/// 赋值运算符全表（长运算符在前，`=` 最后——精确匹配用）。
const ASSIGN_OPS: &[&str] = &[
    ">>=", "<<=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "=",
];

/// 令牌 → 赋值运算符字面文本（非赋值运算符 → None）。
///
/// 多字符运算符的 `AstRef::text()` 是 `PUNCT_2b3d` 这类词法记号名而非字面量，
/// 故必须回**源码区间**取字面文本（`+=` 的区间即 "+="）。
fn tok_assign_op<'s>(source: &'s str, tok: AstRef<'_>) -> Option<&'s str> {
    let s = tok.span();
    if s.start >= s.end || s.end > source.len() {
        return None;
    }
    let txt = &source[s.start..s.end];
    ASSIGN_OPS.iter().copied().find(|op| *op == txt)
}

/// 赋值运算符文本：优先按运算符令牌区间取，取不到回退整段扫描
///（`assign_stmt` 形态：整段文本里找运算符）。
fn assign_op_of(source: &str, node: AstRef<'_>, tok: Option<AstRef<'_>>) -> String {
    tok.and_then(|t| tok_assign_op(source, t))
        .map(|s| s.to_string())
        .unwrap_or_else(|| extract_assign_op(source, node))
}

/// Extract the operator from an assign_stmt / assignment node via its span.
fn extract_assign_op(source: &str, node: AstRef<'_>) -> String {
    let span = node.span();
    if span.start >= span.end {
        return "=".to_string();
    }
    let node_text = &source[span.start..span.end];
    // The node text is like "x = 2;" or "x += 2;"
    let after_name = node_text
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(0);
    let rest = node_text[after_name..].trim_start();
    for op in &[
        ">>=", "<<=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "=",
    ] {
        if rest.starts_with(op) {
            return op.to_string();
        }
    }
    "=".to_string()
}

/// Apply a compound assignment: read old value, compute `old op rhs`.
fn apply_compound_op(
    ctx: &mut HirCtx<'_, SymTable>,
    op: &str,
    slot: GraphValue,
    rhs: GraphValue,
) -> Result<GraphValue, HirError> {
    if op == "=" {
        return Ok(rhs); // simple assign: no need to load old value
    }
    let old = minic_lowering::build_load(ctx.graph, TypeId::I32, slot)?;
    bin_op(ctx, op, old, rhs)
}

// ── Control flow ──

/// 真值化：`v != 0` → i1（`if` / `while` / `for` / `do-while` 共用的比较惯用法）。
fn truthy(ctx: &mut HirCtx<'_, SymTable>, v: GraphValue) -> Result<GraphValue, HirError> {
    let zero = ctx.graph.iconst_i32(0)?;
    ctx.graph.icmp(IntCC::NotEqual, v, zero)
}

/// 条件跳转——三种循环的**公共内容**就在这里（其余是各自固定的块结构与顺序）。
///
/// `Some(cond)`：求值 + `truthy` + `emit_branch(then, else)`；
/// `None`（`for` 省略条件）：无条件 `emit_jump(then)`。
///
/// 不做 `LoopKind` 式统一驱动：init/update/先判或后判/`continue` 落点的差异就是
/// 三种循环的全部语义，抽象成本高于收益（见 docs/archive/hir-shrink-plan.md 收缩点 2）。
fn emit_cond_branch(
    ctx: &mut HirCtx<'_, SymTable>,
    cond: Option<AstRef<'_>>,
    then_blk: BlockId,
    else_blk: BlockId,
) -> Result<(), HirError> {
    match cond {
        Some(node) => {
            let v = lower_expr(ctx, node)?;
            let is_true = truthy(ctx, v)?;
            ctx.graph.emit_branch(is_true, then_blk, else_blk)?;
        }
        None => {
            ctx.graph.emit_jump(then_blk)?;
        }
    }
    Ok(())
}

/// 当前块**没有终结符**时才跳到 `target`。
///
/// 循环体/分支体尾部常用：体内若以 break/continue/return 结束，当前块已有终结符，
/// 再发跳转会产出不可达块（IR 校验 `UnreachableBlock` 拒绝）；且体内嵌套 if 可能
/// 切换了当前块，故判据取**当前块**而非进入体时的块句柄。
fn jump_if_open(ctx: &mut HirCtx<'_, SymTable>, target: BlockId) -> Result<(), HirError> {
    let cur = ctx
        .graph
        .current_block()
        .ok_or_else(|| HirError::Internal("no current block".into()))?;
    if !ctx.graph.has_terminator(cur) {
        ctx.graph.emit_jump(target)?;
    }
    Ok(())
}

fn lower_if(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    let cond_node = node
        .get_child("condition")
        .ok_or_else(|| HirError::Lowering("if missing condition".into()))?;
    let then_node = node
        .get_child("then_body")
        .ok_or_else(|| HirError::Lowering("if missing then_body".into()))?;
    let else_opt: Option<AstRef<'_>> = node.get_optional("else_body").flatten();

    let cond_val = lower_expr(ctx, cond_node)?;
    let is_true = truthy(ctx, cond_val)?;

    // Like the direct backend: no else → branch straight to merge (do NOT
    // create an unused else block — an orphan block breaks the x86 backend).
    let has_else = else_opt.is_some();
    let then_blk = ctx.graph.create_block(&[]);
    let else_blk = if has_else {
        ctx.graph.create_block(&[])
    } else {
        BlockId::INVALID
    };
    let merge_blk = ctx.graph.create_block(&[]);

    ctx.graph.emit_branch(
        is_true,
        then_blk,
        if has_else { else_blk } else { merge_blk },
    )?;

    // Then block
    ctx.graph.set_current_block(then_blk);
    lower_block(ctx, then_node)?;
    jump_if_open(ctx, merge_blk)?;

    // Else block
    if has_else {
        let else_n = else_opt.unwrap();
        ctx.graph.set_current_block(else_blk);
        // else_n is an else_clause wrapping "else" + inner block
        let body = else_n
            .children()
            .iter()
            .find(|c| c.kind() == "block")
            .copied()
            .unwrap_or(else_n);
        lower_block(ctx, body)?;
        jump_if_open(ctx, merge_blk)?;
    }

    ctx.graph.set_current_block(merge_blk);
    Ok(())
}

fn lower_while(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    let cond_node = node
        .get_child("condition")
        .ok_or_else(|| HirError::Lowering("while missing condition".into()))?;
    let body_node = node
        .get_child("body")
        .ok_or_else(|| HirError::Lowering("while missing body".into()))?;

    let cond_blk = ctx.graph.create_block(&[]);
    let body_blk = ctx.graph.create_block(&[]);
    let exit_blk = ctx.graph.create_block(&[]);

    ctx.graph.emit_jump(cond_blk)?;

    // Condition block
    ctx.graph.set_current_block(cond_blk);
    emit_cond_branch(ctx, Some(cond_node), body_blk, exit_blk)?;

    // Body block
    ctx.graph.set_current_block(body_blk);
    // while：`continue` 直接跳条件块（无需 update 块）
    ctx.loops.push(LoopFrame::new_cond(cond_blk, exit_blk));
    lower_block(ctx, body_node)?;
    ctx.loops.pop();
    // 体内可能切换了当前块（如内嵌 if），由 jump_if_open 取当前块判定
    jump_if_open(ctx, cond_blk)?;

    ctx.graph.set_current_block(exit_blk);
    Ok(())
}

fn lower_for(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    // for (init?; cond?; update?) { body }
    // 结构：cond → body → update → cond；`continue` 落点 = update 块
    //（C 语义：for 的 continue 先执行 update 再判条件）。
    // update 块**按需创建**：无 `continue` 时 update 内联在循环体尾部，
    // 否则会留下不可达块（IR 校验拒绝 UnreachableBlock）。
    if let Some(Some(init_node)) = node.get_optional("init") {
        lower_stmt(ctx, init_node)?;
    }
    let cond_node = node.get_optional("condition").flatten();
    let update_node = node.get_optional("update").flatten();

    let cond_blk = ctx.graph.create_block(&[]);
    let body_blk = ctx.graph.create_block(&[]);
    let exit_blk = ctx.graph.create_block(&[]);

    ctx.graph.emit_jump(cond_blk)?;

    // Condition block（省略条件 → 无条件进体）
    ctx.graph.set_current_block(cond_blk);
    emit_cond_branch(ctx, cond_node, body_blk, exit_blk)?;

    // Body block（continue 目标在首次 continue 时懒创建）
    ctx.graph.set_current_block(body_blk);
    ctx.loops.push(LoopFrame::new_for(cond_blk, exit_blk));
    let body_node = node
        .get_child("body")
        .ok_or_else(|| HirError::Lowering("for missing body".into()))?;
    lower_block(ctx, body_node)?;

    let frame = ctx
        .loops
        .last()
        .copied()
        .ok_or_else(|| HirError::Internal("for: loop frame missing".into()))?;
    match frame.update_blk {
        // 有 continue：循环体尾部汇入 update 块（两条路径各执行一次 update）
        Some(update_blk) => {
            jump_if_open(ctx, update_blk)?;
            ctx.graph.set_current_block(update_blk);
            if let Some(update_node) = update_node {
                lower_stmt(ctx, update_node)?;
            }
            jump_if_open(ctx, cond_blk)?;
        }
        // 无 continue：update 内联在循环体尾部（老结构，零额外块）
        None => {
            let cur = ctx
                .graph
                .current_block()
                .ok_or_else(|| HirError::Internal("no current block".into()))?;
            if !ctx.graph.has_terminator(cur) {
                if let Some(update_node) = update_node {
                    lower_stmt(ctx, update_node)?;
                }
                ctx.graph.emit_jump(cond_blk)?;
            }
        }
    }
    ctx.loops.pop();

    ctx.graph.set_current_block(exit_blk);
    Ok(())
}

fn lower_do_while(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    let body_node = node
        .get_child("body")
        .ok_or_else(|| HirError::Lowering("do_while missing body".into()))?;
    let cond_node = node
        .get_child("condition")
        .ok_or_else(|| HirError::Lowering("do_while missing condition".into()))?;

    let body_blk = ctx.graph.create_block(&[]);
    let cond_blk = ctx.graph.create_block(&[]);
    let exit_blk = ctx.graph.create_block(&[]);

    ctx.graph.emit_jump(body_blk)?;

    // Body block
    ctx.graph.set_current_block(body_blk);
    ctx.loops.push(LoopFrame::new_cond(cond_blk, exit_blk));
    lower_block(ctx, body_node)?;
    ctx.loops.pop();
    jump_if_open(ctx, cond_blk)?;

    // Condition block（先体后判：条件真 → 回到循环体）
    ctx.graph.set_current_block(cond_blk);
    emit_cond_branch(ctx, Some(cond_node), body_blk, exit_blk)?;

    ctx.graph.set_current_block(exit_blk);
    Ok(())
}

fn lower_break(ctx: &mut HirCtx<'_, SymTable>) -> Result<(), HirError> {
    let exit_blk = ctx
        .loops
        .last()
        .ok_or_else(|| HirError::Lowering("break outside loop".into()))?
        .exit_blk;
    ctx.graph.emit_jump(exit_blk)?;
    Ok(())
}

fn lower_continue(ctx: &mut HirCtx<'_, SymTable>) -> Result<(), HirError> {
    let last = ctx
        .loops
        .last_mut()
        .ok_or_else(|| HirError::Lowering("continue outside loop".into()))?;
    // for 的 continue 必须先执行 update：update 块**按需创建**（无 continue 的
    // for 循环不留孤立块——IR 校验拒绝不可达块）。
    if last.update_needed && last.update_blk.is_none() {
        last.update_blk = Some(ctx.graph.create_block(&[]));
    }
    let target = last.continue_target();
    ctx.graph.emit_jump(target)?;
    Ok(())
}

// ── for_init / for_update (used by lower_for) ──

fn lower_for_init(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    let name = node.get_text("name").unwrap_or("_").to_string();
    let slot = ctx.alloc_slot()?;
    let val_node = node
        .get_child("value")
        .ok_or_else(|| HirError::Lowering("for_init missing value".into()))?;
    let val = lower_expr(ctx, val_node)?;
    minic_lowering::build_store(ctx.graph, val, slot)?;
    ctx.locals.insert(name, slot);
    Ok(())
}

fn lower_for_update(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    let name = node.get_text("name").unwrap_or("_");
    let slot = ctx.lookup(name)?;
    let val_node = node
        .get_child("value")
        .ok_or_else(|| HirError::Lowering("for_update missing value".into()))?;
    let val = lower_expr(ctx, val_node)?;
    minic_lowering::build_store(ctx.graph, val, slot)?;
    Ok(())
}

// ============================================================
// Expression lowering
// ============================================================

fn lower_expr(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<GraphValue, HirError> {
    // 诊断升级：记录表达式节点位置（错误定位到最内层节点）
    ctx.set_span(node);
    let result = match node.kind() {
        // ── Terminals ──
        "NUMBER" => {
            let text = node.text().unwrap_or("0");
            minic_lowering::build_iconst(ctx.graph, crate::codegen::parse_number_static(text))
        }
        "CHAR" => {
            let text = node.text().unwrap_or("'\\0'");
            let ch = text.chars().nth(1).unwrap_or('\0');
            minic_lowering::build_iconst(ctx.graph, ch as i32)
        }
        "IDENT" => {
            let name = node.text().unwrap_or("_");
            // Enum constants first
            if let Some(&val) = ctx.syms.enum_values.get(name) {
                minic_lowering::build_iconst(ctx.graph, val)
            } else {
                // Then stack-slot locals
                let slot = ctx.lookup(name)?;
                minic_lowering::build_load(ctx.graph, TypeId::I32, slot)
            }
        }

        // ── Precedence levels ──
        "assignment" => lower_assign_expr(ctx, node),
        "logical_or" => lower_logical_or(ctx, node),
        "logical_and" => lower_logical_and(ctx, node),
        "bitwise_or" | "bitwise_xor" | "bitwise_and" | "shift" => lower_binary(ctx, node),
        "equality" | "relational" => lower_compare(ctx, node),
        "additive" | "multiplicative" => lower_binary(ctx, node),
        "unary" => lower_unary(ctx, node),

        // ── Composite ──
        "member_access" => lower_member_access(ctx, node),
        "call" => lower_call(ctx, node),

        // ── Parenthesized expr / primary wrapper ──
        "primary" => {
            let fc = flat_children(node);
            match fc.len() {
                1 => lower_expr(ctx, fc[0]),
                3 => lower_expr(ctx, fc[1]), // ( expr )
                n => Err(HirError::Lowering(format!(
                    "unexpected primary children: {} items",
                    n
                ))),
            }
        }

        other => {
            // Fallback: flatten transparent wrappers and try the single child
            let fc = flat_children(node);
            // 表达式级赋值（含成员）：`x += 4` / `p.x += 4` 经 expr_stmt 以 **seq**
            // 形态到达（children = [IDENT|member_access, OP 令牌, rhs]）。语句级
            // assign_stmt 只接受成员 `=`，复合形式全走这里；旧实现落入下方二元链
            // → 赋值被静默丢弃（成员复合赋值 dual_backend 用例实证）。
            if fc.len() >= 3
                && matches!(fc[0].kind(), "IDENT" | "member_access")
                && tok_assign_op(ctx.source, fc[1]).is_some()
            {
                return lower_assign_expr(ctx, node);
            }
            if fc.len() == 1 {
                lower_expr(ctx, fc[0])
            } else if !fc.is_empty() {
                // 多子 `seq`：按「左结合二元链」求值——与 Direct 后端
                // `codegen.rs` 的同名兜底等价（成员复合赋值 `p.x += 4` 等经语法
                // 包装后以此形态到达）。旧实现直接报 "unknown expr kind: seq"，
                // 导致 Hir 后端编译失败而 Direct 后端正常（dual_backend 补
                // 成员复合赋值用例时实证）。
                let mut acc = lower_expr(ctx, fc[0])?;
                let mut i = 1usize;
                while i + 1 < fc.len() {
                    let op = fc[i].text().unwrap_or("?");
                    let rhs = lower_expr(ctx, fc[i + 1])?;
                    acc = match intcc_for(op) {
                        Some(cc) => {
                            let cmp = minic_lowering::build_icmp(ctx.graph, cc, acc, rhs)?;
                            minic_lowering::build_sextend(ctx.graph, TypeId::I32, cmp)?
                        }
                        None => bin_op(ctx, op, acc, rhs)?,
                    };
                    i += 2;
                }
                Ok(acc)
            } else {
                Err(HirError::Lowering(format!("unknown expr kind: {}", other)))
            }
        }
    };
    result.map_err(|e| ctx.locate(e))
}

/// Flatten transparent wrappers (rep, opt, seq) from AST children.
fn flat_children(node: AstRef<'_>) -> Vec<AstRef<'_>> {
    let mut result = Vec::new();
    for child in node.children() {
        match child.kind() {
            "rep" | "opt" | "seq" => {
                result.extend(flat_children(child));
            }
            _ => result.push(child),
        }
    }
    result
}

// ── Binary arithmetic / bitwise (+ - * / % & | ^ << >>) ──

/// 二元运算分派（**唯一**的「运算符字符串 → build_xxx」表）。
///
/// 计划的收缩点 1（`docs/archive/hir-shrink-plan.md`）：旧实现里
/// `apply_compound_op`（`+=` 等复合形式）与 `lower_binary`（普通形式）各自
/// 维护一张 10 臂 match 表——本函数按「去掉尾随 `=`」归一化后共用一条分派。
fn bin_op(
    ctx: &mut HirCtx<'_, SymTable>,
    op: &str,
    l: GraphValue,
    r: GraphValue,
) -> Result<GraphValue, HirError> {
    match op.strip_suffix('=').unwrap_or(op) {
        "+" => minic_lowering::build_iadd(ctx.graph, l, r),
        "-" => minic_lowering::build_isub(ctx.graph, l, r),
        "*" => minic_lowering::build_imul(ctx.graph, l, r),
        "/" => minic_lowering::build_sdiv(ctx.graph, l, r),
        "%" => minic_lowering::build_srem(ctx.graph, l, r),
        "&" => minic_lowering::build_band(ctx.graph, l, r),
        "|" => minic_lowering::build_bor(ctx.graph, l, r),
        "^" => minic_lowering::build_bxor(ctx.graph, l, r),
        "<<" => minic_lowering::build_ishl(ctx.graph, l, r),
        ">>" => minic_lowering::build_sshr(ctx.graph, l, r),
        other => Err(HirError::Lowering(format!("unknown binary op: {other}"))),
    }
}

/// 比较运算符 → `IntCC`（`lower_compare` 与兜底链共用；未命中返回 None）。
fn intcc_for(op: &str) -> Option<IntCC> {
    Some(match op {
        "==" => IntCC::Equal,
        "!=" => IntCC::NotEqual,
        "<" => IntCC::SignedLessThan,
        ">" => IntCC::SignedGreaterThan,
        "<=" => IntCC::SignedLessThanOrEqual,
        ">=" => IntCC::SignedGreaterThanOrEqual,
        _ => return None,
    })
}

fn lower_binary(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<GraphValue, HirError> {
    let children = flat_children(node);
    if children.is_empty() {
        return minic_lowering::build_iconst(ctx.graph, 0);
    }
    let mut acc = lower_expr(ctx, children[0])?;
    let mut i = 1;
    while i + 1 < children.len() {
        let op = children[i].text().unwrap_or("?");
        let rhs = lower_expr(ctx, children[i + 1])?;
        acc = bin_op(ctx, op, acc, rhs)?;
        i += 2;
    }
    Ok(acc)
}

// ── Binary comparison (== != < > <= >=) ──

fn lower_compare(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<GraphValue, HirError> {
    let children = flat_children(node);
    if children.is_empty() {
        return minic_lowering::build_iconst(ctx.graph, 0);
    }
    if children.len() == 1 {
        return lower_expr(ctx, children[0]);
    }
    let mut acc = lower_expr(ctx, children[0])?;
    let mut i = 1;
    while i + 1 < children.len() {
        let op = children[i].text().unwrap_or("?");
        let rhs = lower_expr(ctx, children[i + 1])?;
        let cc = intcc_for(op)
            .ok_or_else(|| HirError::Lowering(format!("unknown compare op: {}", op)))?;
        let cmp = minic_lowering::build_icmp(ctx.graph, cc, acc, rhs)?;
        acc = minic_lowering::build_sextend(ctx.graph, TypeId::I32, cmp)?;
        i += 2;
    }
    Ok(acc)
}

// ── Logical and (&&) — non-short-circuit ──

fn lower_logical_and(
    ctx: &mut HirCtx<'_, SymTable>,
    node: AstRef<'_>,
) -> Result<GraphValue, HirError> {
    let children = flat_children(node);
    if children.is_empty() {
        return minic_lowering::build_iconst(ctx.graph, 0);
    }
    if children.len() == 1 {
        return lower_expr(ctx, children[0]);
    }
    let zero = ctx.graph.iconst_i32(0)?;
    let lhs = lower_expr(ctx, children[0])?;
    let mut result = ctx.graph.icmp(IntCC::NotEqual, lhs, zero)?;
    let mut i = 1;
    while i < children.len() {
        let rhs = lower_expr(ctx, children[i + 1])?;
        let rhs_ne = ctx.graph.icmp(IntCC::NotEqual, rhs, zero)?;
        result = ctx.graph.band(result, rhs_ne)?;
        i += 2;
    }
    ctx.graph.sextend(result, TypeId::I32)
}

// ── Logical or (||) — non-short-circuit ──

fn lower_logical_or(
    ctx: &mut HirCtx<'_, SymTable>,
    node: AstRef<'_>,
) -> Result<GraphValue, HirError> {
    let children = flat_children(node);
    if children.is_empty() {
        return minic_lowering::build_iconst(ctx.graph, 0);
    }
    if children.len() == 1 {
        return lower_expr(ctx, children[0]);
    }
    let zero = ctx.graph.iconst_i32(0)?;
    let lhs = lower_expr(ctx, children[0])?;
    let mut result = ctx.graph.icmp(IntCC::NotEqual, lhs, zero)?;
    let mut i = 1;
    while i < children.len() {
        let rhs = lower_expr(ctx, children[i + 1])?;
        let rhs_ne = ctx.graph.icmp(IntCC::NotEqual, rhs, zero)?;
        result = ctx.graph.bor(result, rhs_ne)?;
        i += 2;
    }
    ctx.graph.sextend(result, TypeId::I32)
}

// ── Unary (-, +, !, ~) ──

fn lower_unary(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<GraphValue, HirError> {
    let children = flat_children(node);
    if children.is_empty() {
        return minic_lowering::build_iconst(ctx.graph, 0);
    }
    if children.len() == 1 {
        return lower_expr(ctx, children[0]);
    }
    // children: [operator_token, operand]
    let op = children[0].text().unwrap_or("");
    let operand = lower_expr(ctx, children[1])?;
    match op {
        "-" => {
            let zero = ctx.graph.iconst_i32(0)?;
            minic_lowering::build_isub(ctx.graph, zero, operand)
        }
        "+" => Ok(operand), // unary plus is no-op
        "!" => {
            let zero = ctx.graph.iconst_i32(0)?;
            let cmp = ctx.graph.icmp(IntCC::Equal, operand, zero)?;
            ctx.graph.sextend(cmp, TypeId::I32)
        }
        "~" => minic_lowering::build_bnot(ctx.graph, operand),
        _ => Ok(operand),
    }
}

// ── Assignment expression (x = expr / x += expr as a value) ──

fn lower_assign_expr(
    ctx: &mut HirCtx<'_, SymTable>,
    node: AstRef<'_>,
) -> Result<GraphValue, HirError> {
    let children = flat_children(node);
    let op = assign_op_of(ctx.source, node, children.get(1).copied());
    if children.len() >= 2 {
        // [IDENT, OP, rhs] for x = expr / x += expr
        if children[0].kind() == "IDENT" {
            let name = children[0].text().unwrap_or("_");
            if ctx.locals.contains_key(name) {
                let slot = ctx.lookup(name)?;
                let rhs = lower_expr(ctx, children[children.len() - 1])?;
                let result = apply_compound_op(ctx, &op, slot, rhs)?;
                minic_lowering::build_store(ctx.graph, result, slot)?;
                return Ok(result);
            }
        }
        // [member_access, OP, rhs] → 成员赋值（`p.x = 4` / `p.x += 4`）——
        // 语法：语句级 assign_stmt 只接受 `=`，复合形式经 expr_stmt →
        // assignment(member_access OP assignment) 到达这里。旧实现只认 IDENT →
        // **静默丢弃**整个赋值（dual_backend 成员复合赋值用例实证）。
        if children[0].kind() == "member_access" {
            let ma_idents = children[0].get_children("idents");
            let obj_name = ma_idents.first().and_then(|c| c.text()).unwrap_or("_");
            let field_name = ma_idents.get(1).and_then(|c| c.text()).unwrap_or("_");
            let slot = ctx.lookup(&format!("{}.{}", obj_name, field_name))?;
            let rhs = lower_expr(ctx, children[children.len() - 1])?;
            let result = apply_compound_op(ctx, &op, slot, rhs)?;
            minic_lowering::build_store(ctx.graph, result, slot)?;
            return Ok(result);
        }
    }
    // Not an assignment — process first child
    if children.is_empty() {
        return minic_lowering::build_iconst(ctx.graph, 0);
    }
    lower_expr(ctx, children[0])
}

// ── Struct support (each field gets its own stack slot: "p.x") ──

fn lower_struct_def(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    // Register field names for per-field slot allocation (same as codegen.rs).
    let name = node.get_text("name").unwrap_or("_").to_string();
    let field_names: Vec<String> = node
        .get_children("fields")
        .iter()
        .map(|f| f.get_text("name").unwrap_or("_").to_string())
        .collect();
    ctx.syms.structs.insert(
        name,
        crate::codegen::StructInfo {
            fields: field_names.iter().map(|n| (n.clone(), 0)).collect(),
            total_size: (field_names.len() as i32) * 4,
        },
    );
    Ok(())
}

fn lower_struct_decl(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    let idents = node.get_children("idents");
    let struct_name = idents.first().and_then(|c| c.text()).unwrap_or("_");
    let var_name = idents.get(1).and_then(|c| c.text()).unwrap_or("_");
    alloc_struct_fields(ctx, struct_name, var_name)
}

fn lower_struct_init(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<(), HirError> {
    let idents = node.get_children("idents");
    let struct_name = idents.first().and_then(|c| c.text()).unwrap_or("_");
    let var_name = idents.get(1).and_then(|c| c.text()).unwrap_or("_");

    alloc_struct_fields(ctx, struct_name, var_name)?;

    // Initialize each field from the init list
    let field_names: Vec<String> = ctx
        .syms
        .structs
        .get(struct_name)
        .map(|s| s.fields.iter().map(|(n, _)| n.clone()).collect())
        .unwrap_or_default();
    let values = node.get_children("values");
    for (i, val_node) in values.iter().enumerate() {
        if i < field_names.len() {
            let val = lower_expr(ctx, *val_node)?;
            let key = field_key(var_name, &field_names[i]);
            if let Some(&slot) = ctx.locals.get(&key) {
                minic_lowering::build_store(ctx.graph, val, slot)?;
            }
        }
    }
    Ok(())
}

/// Allocate a separate stack slot for each field of a struct variable.
fn alloc_struct_fields(
    ctx: &mut HirCtx<'_, SymTable>,
    struct_name: &str,
    var_name: &str,
) -> Result<(), HirError> {
    let field_names: Vec<String> = ctx
        .syms
        .structs
        .get(struct_name)
        .map(|s| s.fields.iter().map(|(n, _)| n.clone()).collect())
        .unwrap_or_default();
    ctx.syms
        .var_structs
        .insert(var_name.to_string(), struct_name.to_string());
    // Insert var.field entries for direct access
    for fname in &field_names {
        let slot = ctx.alloc_slot()?;
        ctx.locals.insert(field_key(var_name, fname), slot);
    }
    // Sentinel for the root var name (used for type lookup)
    let sentinel = ctx.alloc_slot()?;
    ctx.locals.insert(var_name.to_string(), sentinel);
    Ok(())
}

fn lower_member_access(
    ctx: &mut HirCtx<'_, SymTable>,
    node: AstRef<'_>,
) -> Result<GraphValue, HirError> {
    let idents = node.get_children("idents");
    let obj_name = idents.first().and_then(|c| c.text()).unwrap_or("_");
    let field_name = idents.get(1).and_then(|c| c.text()).unwrap_or("_");

    let slot = ctx.lookup(&field_key(obj_name, field_name))?;
    minic_lowering::build_load(ctx.graph, TypeId::I32, slot)
}

// ── Function call (implemented via AST-level inlining) ──

fn lower_call(ctx: &mut HirCtx<'_, SymTable>, node: AstRef<'_>) -> Result<GraphValue, HirError> {
    let name = node.get_text("name").unwrap_or("_");

    // 递归检测：callee 已在内联链中（含当前函数自身）→ 无法内联（无限展开）。
    // HIR 后端暂无真实 Call 指令支持（IrGraph 无 call 原子）——明确报错，
    // 后续迭代补 Call 原子后改为运行时递归。
    if ctx.inlining.iter().any(|f| f == name) {
        return Err(HirError::Lowering(format!(
            "recursive call to '{}' unsupported in HIR backend (no real Call yet); use Backend::V12/Direct",
            name
        )));
    }

    // Look up the callee's func_def AST node for inlining
    let callee_id = *ctx
        .syms
        .func_defs
        .get(name)
        .ok_or_else(|| HirError::Lowering(format!("undefined function: {}", name)))?;
    let callee_node = ctx.ast.ref_to(callee_id);

    // Evaluate call arguments
    let arg_vals: Vec<GraphValue> = match node.get_optional("args") {
        Some(Some(arg_list)) => {
            let items = arg_list.get_children("items");
            let mut vals = Vec::new();
            for a in &items {
                vals.push(lower_expr(ctx, *a)?);
            }
            vals
        }
        _ => vec![],
    };

    let params = callee_node.get_children("params");

    // Save caller locals that would be shadowed by params
    let mut saved_locals: HashMap<String, GraphValue> = HashMap::new();
    for param in &params {
        let pname = param.get_text("name").unwrap_or("_").to_string();
        if let Some(old) = ctx.locals.remove(&pname) {
            saved_locals.insert(pname, old);
        }
    }

    // Allocate return value slot
    let ret_slot = ctx.alloc_slot()?;

    // Allocate parameter slots and store argument values
    for (i, param) in params.iter().enumerate() {
        let pname = param.get_text("name").unwrap_or("_").to_string();
        let slot = ctx.alloc_slot()?;
        let val = match arg_vals.get(i).copied() {
            Some(v) => v,
            None => ctx.graph.iconst_i32(0)?, // missing arg → 0
        };
        minic_lowering::build_store(ctx.graph, val, slot)?;
        ctx.locals.insert(pname, slot);
    }

    // Push inlining context and codegen the callee body directly inline
    let old_return_slot = ctx.return_slot;
    let old_return_block = ctx.return_block;
    let after_blk = ctx.graph.create_block(&[]);
    ctx.return_slot = Some(ret_slot);
    ctx.return_block = Some(after_blk);
    ctx.inlining.push(name.to_string());

    let body = callee_node
        .get_child("body")
        .ok_or_else(|| HirError::Lowering("callee missing body".into()))?;
    lower_block(ctx, body)?;

    // If the body didn't return, store 0 and jump to after
    let cur = ctx
        .graph
        .current_block()
        .ok_or_else(|| HirError::Internal("no current block".into()))?;
    if !ctx.graph.has_terminator(cur) {
        let zero = ctx.graph.iconst_i32(0)?;
        minic_lowering::build_store(ctx.graph, zero, ret_slot)?;
        ctx.graph.emit_jump(after_blk)?;
    }

    // Restore inlining context and continue in after block
    ctx.return_slot = old_return_slot;
    ctx.return_block = old_return_block;
    ctx.inlining.pop();
    ctx.graph.set_current_block(after_blk);

    // Restore shadowed caller locals
    for (local_name, slot) in saved_locals {
        ctx.locals.insert(local_name, slot);
    }

    // Load and return the inlined function's return value
    minic_lowering::build_load(ctx.graph, TypeId::I32, ret_slot)
}

// ============================================================
// HIR pipeline tests
// ============================================================

// 本测试模块 JIT 编译并在本机执行 x86_64 机器码（Windows x64 ABI）——
// 仅 Windows x86_64 宿主正确（macOS arm64 SIGILL；Linux SysV 参数错位）。
#[cfg(all(test, target_arch = "x86_64", windows))]
mod tests {
    use crate::atoms::minic_lowering;
    use code_forge::ir::{FunctionSignature, Module, TypeId};

    /// Compile `source` through the full HIR pipeline and JIT-run `main()`.
    fn run_hir(source: &str) -> i32 {
        use crate::codegen::SymTable;
        use crate::grammar::build_grammar;
        use crate::schema::build_schema;
        use code_forge::backend::arch::x86_v12::{self, ensure_registered};
        use code_forge::backend::jit::JitCompiler;
        use code_forge::forge_grammar::Parser;

        let grammar = build_grammar();
        let schema = build_schema();
        let parser = Parser::build(grammar);
        let ast = parser.parse_to_ast(source, &schema).unwrap();
        let funcs: Vec<_> = ast.root_ref().find_all("func_def");

        let mut module = Module::new();
        let mut syms = SymTable::new();

        // First pass: register all function AST nodes for inlining
        for func_node in &funcs {
            let name = func_node.get_text("name").unwrap_or("_").to_string();
            syms.func_defs.insert(name, func_node.id);
        }
        // Second pass: codegen each function
        for func_node in &funcs {
            let name = func_node.get_text("name").unwrap_or("_").to_string();
            super::codegen_function_hir(&mut module, *func_node, &mut syms, &ast, source)
                .unwrap_or_else(|e| panic!("HIR codegen error in '{}': {}", name, e));
        }

        ensure_registered();
        let tm = x86_v12::TargetMachine::new();
        let mut jit = JitCompiler::new(tm);
        jit.compile_module(&module).unwrap();
        let main_fn: extern "C" fn() -> i32 = jit.get_fn("main").unwrap();
        main_fn()
    }

    /// Verify HIR lowering produces correct IR structure.
    #[test]
    fn test_hir_lowering_iconst_ret() {
        use forge_hir::{BrickRegistry, IrGraph, lower_into_module};

        let mut graph = IrGraph::new();
        let entry = graph.create_block(&[]);
        graph.set_current_block(entry);
        let v = graph.iconst_i32(42).unwrap();
        graph.emit_ret(v).unwrap();

        let mut registry = BrickRegistry::new();
        minic_lowering::register_atoms(&mut registry);

        let mut module = Module::new();
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let func_ref = lower_into_module(&graph, &registry, &mut module, "test", sig).unwrap();
        let func = module.get_function(func_ref);

        assert!(func.dfg.block_count() >= 1);
        assert!(func.entry_block.is_some());
    }

    /// Verify HIR lowering → JIT execution works end-to-end.
    #[test]
    fn test_hir_jit_iconst_ret() {
        use code_forge::backend::arch::x86_v12::{self, ensure_registered};
        use code_forge::backend::jit::JitCompiler;
        use forge_hir::{BrickRegistry, IrGraph, lower_into_module};

        let mut graph = IrGraph::new();
        let entry = graph.create_block(&[]);
        graph.set_current_block(entry);
        let v = graph.iconst_i32(42).unwrap();
        graph.emit_ret(v).unwrap();

        let mut registry = BrickRegistry::new();
        minic_lowering::register_atoms(&mut registry);

        let mut module = Module::new();
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let _ = lower_into_module(&graph, &registry, &mut module, "test_hir_jit", sig).unwrap();

        ensure_registered();
        let tm = x86_v12::TargetMachine::new();
        let mut jit = JitCompiler::new(tm);
        jit.compile_module(&module).unwrap();

        let f: extern "C" fn() -> i32 = jit.get_fn("test_hir_jit").unwrap();
        assert_eq!(f(), 42, "HIR JIT should return 42");
    }

    /// Control: JIT execution of direct FunctionBuilder (no HIR).
    #[test]
    fn test_direct_jit_iconst_ret() {
        use code_forge::backend::arch::x86_v12::{self, ensure_registered};
        use code_forge::backend::jit::JitCompiler;
        use code_forge::ir::FunctionBuilder;

        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let ctx = code_forge::ir::TypeContext::new();
        let mut builder = FunctionBuilder::new("test_direct", ctx, sig);
        let (entry, _) = builder.create_entry_block();
        builder.switch_to_block(entry);
        let v = builder.iconst_i32(42);
        builder.ret(&[v]);
        let func = builder.finish().expect("build");

        let mut module = Module::new();
        module.add_function(func);

        ensure_registered();
        let tm = x86_v12::TargetMachine::new();
        let mut jit = JitCompiler::new(tm);
        jit.compile_module(&module).unwrap();

        let f: extern "C" fn() -> i32 = jit.get_fn("test_direct").unwrap();
        assert_eq!(f(), 42, "Direct JIT should return 42");
    }

    // ── End-to-end JIT assertions over the full mini_c pipeline ──

    #[test]
    fn test_hir_e2e_return_42() {
        assert_eq!(run_hir("int main() { return 42; }"), 42);
    }

    #[test]
    fn test_hir_e2e_arithmetic() {
        assert_eq!(run_hir("int main() { return 2 + 3 * 4; }"), 14);
        assert_eq!(run_hir("int main() { return 17 / 5; }"), 3);
        assert_eq!(run_hir("int main() { return 17 % 5; }"), 2);
    }

    #[test]
    fn test_hir_e2e_locals_and_assign() {
        assert_eq!(
            run_hir("int main() { int x = 5; int y = x + 1; return y; }"),
            6
        );
        assert_eq!(
            run_hir("int main() { int x = 5; x = x * 2; return x; }"),
            10
        );
        assert_eq!(run_hir("int main() { int x = 5; x += 3; return x; }"), 8);
        assert_eq!(run_hir("int main() { int x = 5; x *= 4; return x; }"), 20);
    }

    #[test]
    fn test_hir_e2e_params() {
        assert_eq!(
            run_hir("int add(int a, int b) { return a + b; } int main() { return add(2, 3); }"),
            5
        );
        assert_eq!(
            run_hir(
                "int add(int a, int b) { return a + b; } int main() { return add(add(10, 20), 12); }"
            ),
            42
        );
    }

    #[test]
    fn test_hir_e2e_if_else() {
        assert_eq!(
            run_hir("int main() { if (1) { return 10; } else { return 20; } }"),
            10
        );
        assert_eq!(
            run_hir("int main() { if (0) { return 10; } else { return 20; } }"),
            20
        );
        assert_eq!(
            run_hir("int main() { int x = 3; if (x > 2) { return 1; } return 0; }"),
            1
        );
    }

    #[test]
    fn test_hir_e2e_loops() {
        assert_eq!(
            run_hir(
                "int main() { int s = 0; for (int i = 1; i <= 10; i = i + 1) { s = s + i; } return s; }"
            ),
            55
        );
        assert_eq!(
            run_hir(
                "int main() { int s = 0; int i = 1; while (i <= 10) { s = s + i; i = i + 1; } return s; }"
            ),
            55
        );
        assert_eq!(
            run_hir(
                "int main() { int s = 0; int i = 1; do { s = s + i; i = i + 1; } while (i <= 10); return s; }"
            ),
            55
        );
    }

    #[test]
    fn test_hir_e2e_break() {
        // NOTE（2026-08 P2 更新）：x86 后端条件分支编码错误已随 break SEGV 根治
        // （b87f0b7）与后续修复消除——循环内条件分支现可正常测试，见
        // `tests/dual_backend_tests.rs` `both_loop_cond_branch`（双后端一致）。
        assert_eq!(
            run_hir(
                "int main() { int x = 0; for (int i = 0; i < 100; i = i + 1) { x = 42; break; } return x; }"
            ),
            42
        );
        assert_eq!(
            run_hir("int main() { int x = 0; while (1) { x = 42; break; } return x; }"),
            42
        );
    }

    #[test]
    fn test_hir_e2e_loop_bound_from_param() {
        // 残余回归（commit cc28d46 记录 "loops got 1"）：循环边界来自函数参数，
        // 而非字面量——覆盖参数入栈 + 循环条件读取栈槽的组合。
        assert_eq!(
            run_hir(
                "int sum(int n) { int s = 0; for (int i = 0; i < n; i = i + 1) { s = s + i; } return s; } int main() { return sum(10); }"
            ),
            45
        );
        assert_eq!(
            run_hir(
                "int count(int n) { int c = 0; int i = 0; while (i < n) { c = c + 1; i = i + 1; } return c; } int main() { return count(7); }"
            ),
            7
        );
    }

    #[test]
    fn test_hir_e2e_multi_param_call_after_loop() {
        // 残余回归（commit cc28d46 记录 "params SEGV"）：循环体之后的多参数
        // 函数调用——覆盖循环退出后栈布局与参数槽的重叠场景。
        assert_eq!(
            run_hir(
                "int f(int a, int b, int c) { return a + b + c; } int main() { int s = 0; for (int i = 0; i < 3; i = i + 1) { s = s + 1; } return f(s, 2, 3); }"
            ),
            8
        );
        assert_eq!(
            run_hir(
                "int f(int a, int b, int c, int d) { return a * 1000 + b * 100 + c * 10 + d; } int main() { int s = 0; while (s < 2) { s = s + 1; } return f(1, 2, s, 4); }"
            ),
            1224
        );
    }

    #[test]
    fn test_hir_e2e_bitwise_and_shift() {
        assert_eq!(run_hir("int main() { return 6 & 3; }"), 2);
        assert_eq!(run_hir("int main() { return 5 | 2; }"), 7);
        assert_eq!(run_hir("int main() { return 1 << 4; }"), 16);
        assert_eq!(run_hir("int main() { return 16 >> 2; }"), 4);
        assert_eq!(run_hir("int main() { return ~0; }"), -1);
    }

    #[test]
    fn test_hir_e2e_logical() {
        assert_eq!(run_hir("int main() { return 1 && 1; }"), 1);
        assert_eq!(run_hir("int main() { return 1 && 0; }"), 0);
        assert_eq!(run_hir("int main() { return 0 || 1; }"), 1);
    }

    #[test]
    fn test_hir_e2e_unary_and_char() {
        assert_eq!(run_hir("int main() { return -5; }"), -5);
        assert_eq!(run_hir("int main() { return !0; }"), 1);
        assert_eq!(run_hir("int main() { return 'A'; }"), 65);
        assert_eq!(run_hir("int main() { return 0x10; }"), 16);
        assert_eq!(run_hir("int main() { return 010; }"), 8);
    }

    #[test]
    fn test_hir_e2e_struct() {
        assert_eq!(
            run_hir(
                "int main() { struct P { int x; int y; }; struct P p; p.x = 3; p.y = 4; return p.x + p.y; }"
            ),
            7
        );
    }

    /// 递归函数：HIR 后端用 AST 内联实现调用——递归无法内联，应报清晰错误
    /// （而非无限展开栈溢出）。Direct/V12 后端支持递归（真实 Call）。
    #[test]
    fn test_hir_recursive_rejected() {
        use crate::codegen::SymTable;
        use crate::grammar::build_grammar;
        use crate::schema::build_schema;
        use code_forge::forge_grammar::Parser;

        let source = "int fib(int n) { if(n <= 1){ return n; }  return fib(n-1) + fib(n-2); } int main() { return fib(3); }";
        let grammar = build_grammar();
        let schema = build_schema();
        let parser = Parser::build(grammar);
        let ast = parser.parse_to_ast(source, &schema).unwrap();
        let funcs: Vec<_> = ast.root_ref().find_all("func_def");

        let mut module = code_forge::ir::Module::new();
        let mut syms = SymTable::new();
        for func_node in &funcs {
            let name = func_node.get_text("name").unwrap_or("_").to_string();
            syms.func_defs.insert(name, func_node.id);
        }
        // main 调 fib（内联 fib），fib 体内 fib(n-1) 递归 → 应报"recursive call"
        let mut err: Option<String> = None;
        for func_node in &funcs {
            if let Err(e) =
                super::codegen_function_hir(&mut module, *func_node, &mut syms, &ast, source)
            {
                err = Some(e.to_string());
                break;
            }
        }
        let msg = err.unwrap_or_default();
        assert!(
            msg.contains("recursive"),
            "HIR 递归应报清晰错误，实际: {msg:?}"
        );
    }
}
