//! IR code generation — walks the lowered AST and emits forge-ir instructions.
//!
//! # Strategy
//! - **stack-based locals**: uses `stack_addr(0)` + `iadd` to compute addresses,
//!   then `load`/`store` for access.  `alloca` is not supported by x86 backend.
//! - **expression codegen**: walks the transparent precedence chain.
//!   Each level (`additive`, `multiplicative`, …) has its children flattened
//!   into `[operand, op_token, operand, …]` — codegen iterates left-to-right.
//! - **control flow**: creates new `Block`s for if/else/while/for and uses
//!   `branch`/`jump` to wire them together.

use code_forge::forge_grammar::{AstId, AstRef, TypedAst};
use code_forge::ir::{
    Block, FuncRef, FunctionBuilder, FunctionSignature, IntCC, Module, TypeId, Value,
};
use std::collections::HashMap;

// ============================================================
// Symbol table
// ============================================================

/// Information about a struct type: field names + offsets.
#[derive(Clone)]
pub struct StructInfo {
    pub fields: Vec<(String, i32)>, // (field_name, byte_offset)
    pub total_size: i32,
}

/// Tracks variables (stack slot pointers), functions, and type info during codegen.
#[derive(Default)]
pub struct SymTable {
    /// Variable name → stack slot pointer (Value).
    pub locals: HashMap<String, Value>,
    /// Function name → FuncRef (for cross-function calls).
    pub funcs: HashMap<String, FuncRef>,
    /// Function name → AstId of func_def node (for inlining).
    pub func_defs: HashMap<String, AstId>,
    /// Enum value name → i32 constant value.
    pub enum_values: HashMap<String, i32>,
    /// Struct type name → field layout info.
    pub structs: HashMap<String, StructInfo>,
    /// Variable name → struct type name (for field access).
    pub var_structs: HashMap<String, String>,
}

impl SymTable {
    pub fn new() -> Self {
        Self::default()
    }
}

// ============================================================
// Codegen context
// ============================================================

/// Mutable state carried through codegen for a single function.
pub struct CodegenCtx<'a> {
    pub builder: &'a mut FunctionBuilder,
    pub syms: &'a mut SymTable,
    /// AST reference — for accessing func_def nodes during inlining.
    pub ast: &'a TypedAst,
    /// Source text (for extracting operator tokens from spans).
    pub source: &'a str,
    /// Stack offset for the next local variable (grows downward).
    pub next_offset: i32,
    /// Stack of (cond_block, exit_block) for nested while/for loops.
    pub loops: Vec<(Block, Block)>,
    /// Whether the current block already has a terminator (ret/jump/branch).
    pub terminated: bool,
    /// Inlining: return value temp slot. None = not inlining (emit real ret).
    pub return_slot: Option<Value>,
    /// Inlining: block to jump to on return. None = not inlining.
    pub return_block: Option<Block>,
    /// 当前内联链（函数名集合）——递归检测：callee 已在链中 → 改用真实 Call。
    /// 递归函数的 FuncRef 由编译器预注册（`syms.funcs`），否则无法生成自调用。
    pub inlining: Vec<String>,
}

impl<'a> CodegenCtx<'a> {
    pub fn new(
        builder: &'a mut FunctionBuilder,
        syms: &'a mut SymTable,
        ast: &'a TypedAst,
        source: &'a str,
    ) -> Self {
        Self {
            builder,
            syms,
            ast,
            source,
            next_offset: -4, // first local at offset -4
            loops: Vec::new(),
            terminated: false,
            return_slot: None,
            return_block: None,
            inlining: Vec::new(),
        }
    }

    /// Extract the operator text from an assign_stmt node using the source span.
    /// The span covers "[name] [op] [expr];" — we look for the operator between name and expr.
    fn extract_assign_op(&self, node: AstRef<'_>) -> String {
        let span = node.span();
        if span.start >= span.end {
            return "=".to_string();
        }
        let node_text = &self.source[span.start..span.end];
        // The node text is like "x = 2;" or "x += 2;"
        // Find the operator: after the first identifier, before the expression
        let after_name = node_text
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(0);
        let rest = node_text[after_name..].trim_start();
        // Check for compound operators (3-char first, then 2-char)
        for op in &[
            ">>=", "<<=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "=",
        ] {
            if rest.starts_with(op) {
                return op.to_string();
            }
        }
        "=".to_string()
    }

    /// Allocate a new stack slot for a local variable, return its pointer.
    fn alloc_slot(&mut self) -> Value {
        let base = self.builder.stack_addr(0);
        let offset = self.builder.iconst_i32(self.next_offset);
        self.next_offset -= 4;
        self.builder.iadd(base, offset)
    }
}

// ============================================================
// Entry point: compile a single function
// ============================================================

/// Lower one `func_def` AST node into a forge-ir `Function`, add it to `module`,
/// and return its `FuncRef`.
///
/// `pre_registered`：若为 Some(fr)，函数体已预注册占位（递归检测用——递归
/// 函数在编译体时需要自己的 FuncRef 生成自调用 Call），编译完成后用
/// [`Module::replace_function`] 覆盖占位体。
pub fn codegen_function(
    module: &mut Module,
    func_node: AstRef<'_>,
    syms: &mut SymTable,
    ast: &TypedAst,
    source: &str,
    pre_registered: Option<FuncRef>,
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

    let ctx = module.types.clone();
    let mut b = FunctionBuilder::new(name.as_str(), ctx, sig);

    // Entry block with params
    let (entry_block, entry_params) = b.create_entry_block();
    b.switch_to_block(entry_block);

    // Store each parameter into a stack slot
    for (i, param_node) in func_node.get_children("params").iter().enumerate() {
        let pname = param_node.get_text("name").unwrap_or("_").to_string();
        let base = b.stack_addr(0);
        let offset = b.iconst_i32(-4 * (i as i32 + 1));
        let slot = b.iadd(base, offset);
        b.store(entry_params[i], slot);
        syms.locals.insert(pname, slot);
    }

    // Codegen the body (in a block so we can release the borrow before finish())
    let body = func_node.get_child("body").ok_or("func_def missing body")?;

    {
        let param_count = func_node.get_children("params").len() as i32;
        let mut cg = CodegenCtx::new(&mut b, syms, ast, source);
        // 当前函数视为"已内联"——函数体内调用自身 = 递归（真实 Call），
        // 否则 fib 编译时 fib(n-1) 会再次内联 fib → 无限展开。
        cg.inlining.push(name.clone());
        cg.next_offset = -4 * (param_count + 1); // start after params
        codegen_block(&mut cg, body);

        // Implicit return 0 if no explicit return
        if !cg.terminated {
            let zero = cg.builder.iconst_i32(0);
            cg.builder.ret(&[zero]);
        }
    } // cg dropped — borrow on b released

    let func = b.finish().expect("build");
    match pre_registered {
        Some(fr) => {
            // 预注册占位：用真实体覆盖（保留 FuncRef——递归 Call 的 target 不变）
            module.replace_function(fr, func);
            Ok(fr)
        }
        None => Ok(module.add_function(func)),
    }
}

// ============================================================
// Block / statement list
// ============================================================

fn codegen_block(cg: &mut CodegenCtx, block_node: AstRef<'_>) {
    let stmts = block_node.get_children("items");
    for stmt in stmts {
        if cg.terminated {
            break;
        }
        codegen_stmt(cg, stmt);
    }
}

// ============================================================
// Statement dispatch
// ============================================================

/// 赋值运算符全表（长运算符在前，`=` 最后——精确匹配用）。
const ASSIGN_OPS: &[&str] = &[
    ">>=", "<<=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "=",
];

/// 该令牌是否为赋值运算符——按**令牌源码区间**取文本判断。
///
/// 多字符运算符的 `AstRef::text()` 是 `PUNCT_2b3d` 这类词法记号名而非字面量，
/// 故必须回源码取字面文本（`+=` 的源码区间即 "+="）。
fn is_assign_op(source: &str, tok: AstRef<'_>) -> bool {
    tok_assign_op(source, tok).is_some()
}

/// 令牌 → 赋值运算符字面文本（非赋值运算符 → None）。
fn tok_assign_op<'s>(source: &'s str, tok: AstRef<'_>) -> Option<&'s str> {
    let s = tok.span();
    if s.start >= s.end || s.end > source.len() {
        return None;
    }
    let txt = &source[s.start..s.end];
    ASSIGN_OPS.iter().copied().find(|op| *op == txt)
}

fn codegen_stmt(cg: &mut CodegenCtx, node: AstRef<'_>) {
    match node.kind() {
        "return_stmt" => codegen_return(cg, node),
        "var_decl" => codegen_var_decl(cg, node),
        "assign_stmt" => codegen_assign(cg, node),
        "expr_stmt" => codegen_expr_stmt(cg, node),
        "if_stmt" => codegen_if(cg, node),
        "while_stmt" => codegen_while(cg, node),
        "for_stmt" => codegen_for(cg, node),
        "do_while_stmt" => codegen_do_while(cg, node),
        "break_stmt" => codegen_break(cg),
        "continue_stmt" => codegen_continue(cg),
        "enum_def" => codegen_enum_def(cg, node),
        "struct_def" => codegen_struct_def(cg, node),
        "struct_decl" => codegen_struct_decl(cg, node),
        "struct_init" => codegen_struct_init(cg, node),
        other => panic!("unknown statement kind: {}", other),
    }
}

// ── return_stmt ──

fn codegen_return(cg: &mut CodegenCtx, node: AstRef<'_>) {
    let val = node.get_optional("value");
    let v = match val {
        Some(Some(expr_node)) => codegen_expr(cg, expr_node),
        _ => cg.builder.iconst_i32(0),
    };

    // Inlining mode: store return value to temp slot and jump to after-block
    if let (Some(ret_slot), Some(after_blk)) = (cg.return_slot, cg.return_block) {
        cg.builder.store(v, ret_slot);
        cg.builder.jump(after_blk, &[]);
    } else {
        cg.builder.ret(&[v]);
    }
    cg.terminated = true;
}

// ── var_decl ──

fn codegen_var_decl(cg: &mut CodegenCtx, node: AstRef<'_>) {
    let name = node.get_text("name").unwrap_or("_").to_string();
    let slot = cg.alloc_slot();

    if let Some(Some(init_node)) = node.get_optional("init") {
        let val = codegen_expr(cg, init_node);
        cg.builder.store(val, slot);
    }
    cg.syms.locals.insert(name, slot);
}

// ── assign_stmt ──

fn codegen_assign(cg: &mut CodegenCtx, node: AstRef<'_>) {
    // Check if LHS is a member_access (struct field assignment)
    let member_access_node = node
        .children()
        .iter()
        .find(|c| c.kind() == "member_access")
        .copied();

    if let Some(ma_node) = member_access_node {
        // p.x = expr → store to field's dedicated slot
        let val_node = node.get_child("value").expect("assign value");
        let rhs = codegen_expr(cg, val_node);

        let ma_idents = ma_node.get_children("idents");
        let obj_name = ma_idents.first().and_then(|c| c.text()).unwrap_or("_");
        let field_name = ma_idents.get(1).and_then(|c| c.text()).unwrap_or("_");

        // Each field has its own slot: obj.field
        let field_key = format!("{}.{}", obj_name, field_name);
        let field_slot = *cg
            .syms
            .locals
            .get(&field_key)
            .unwrap_or_else(|| panic!("undefined field: {}", field_key));

        let op = cg.extract_assign_op(node);
        let result = apply_compound_op(cg, &op, field_slot, rhs);
        cg.builder.store(result, field_slot);
        return;
    }

    // Plain variable assignment: x = expr
    let name = node.get_text("name").unwrap_or("_");
    let ptr = *cg
        .syms
        .locals
        .get(name)
        .unwrap_or_else(|| panic!("undefined variable: {}", name));
    let val_node = node.get_child("value").expect("assign value");
    let rhs = codegen_expr(cg, val_node);

    // Extract the operator from the source span
    let op = cg.extract_assign_op(node);

    let result = apply_compound_op(cg, &op, ptr, rhs);
    cg.builder.store(result, ptr);
}

// ── expr_stmt ──

fn codegen_expr_stmt(cg: &mut CodegenCtx, node: AstRef<'_>) {
    let expr_node = node.get_child("expr").expect("expr_stmt missing expr");
    codegen_expr(cg, expr_node); // discard result
}

// ── if_stmt ──

fn codegen_if(cg: &mut CodegenCtx, node: AstRef<'_>) {
    let cond_node = node.get_child("condition").expect("if missing condition");
    let then_node = node.get_child("then_body").expect("if missing then");
    let else_opt = node.get_optional("else_body");

    // Evaluate condition expression. For integer comparisons this produces an i1
    // (0 or 1). icmp(NE, val, 0) normalizes non-boolean values to i1.
    let cond_val = codegen_expr(cg, cond_node);
    let zero = cg.builder.iconst_i32(0);
    let is_true = cg.builder.icmp(IntCC::NotEqual, cond_val, zero);

    let then_blk = cg.builder.create_block();
    let merge_blk = cg.builder.create_block();

    let has_else = else_opt.is_some_and(|o| o.is_some());
    let else_blk = if has_else {
        cg.builder.create_block()
    } else {
        Block(u32::MAX)
    };

    if has_else {
        cg.builder.branch(is_true, then_blk, &[], else_blk, &[]);
    } else {
        cg.builder.branch(is_true, then_blk, &[], merge_blk, &[]);
    }

    // Then block
    cg.builder.switch_to_block(then_blk);
    cg.terminated = false;
    codegen_block(cg, then_node);
    if !cg.terminated {
        cg.builder.jump(merge_blk, &[]);
        cg.terminated = true;
    }

    // Else block
    if has_else {
        cg.builder.switch_to_block(else_blk);
        cg.terminated = false;
        if let Some(Some(else_node)) = else_opt {
            // else_node is an else_clause (wraps "else" keyword + inner block).
            // Find the inner block child by kind.
            let body = else_node
                .children()
                .iter()
                .find(|c| c.kind() == "block")
                .copied()
                .unwrap_or(else_node);
            codegen_block(cg, body);
        }
        if !cg.terminated {
            cg.builder.jump(merge_blk, &[]);
            cg.terminated = true;
        }
    }

    cg.builder.switch_to_block(merge_blk);
    cg.terminated = false;
}

// ── while_stmt ──

fn codegen_while(cg: &mut CodegenCtx, node: AstRef<'_>) {
    let cond_node = node
        .get_child("condition")
        .expect("while missing condition");
    let body_node = node.get_child("body").expect("while missing body");

    let cond_blk = cg.builder.create_block();
    let body_blk = cg.builder.create_block();
    let exit_blk = cg.builder.create_block();

    // Jump to condition block
    cg.builder.jump(cond_blk, &[]);

    // Condition block
    cg.builder.switch_to_block(cond_blk);
    let cond_val = codegen_expr(cg, cond_node);
    let zero = cg.builder.iconst_i32(0);
    let is_true = cg.builder.icmp(IntCC::NotEqual, cond_val, zero);
    // NOTE: branch directly on the i1 icmp result (like for/do-while).
    // The old `sextend(is_true, I32)` made the branch condition i32, which
    // the x86 backend's `lower_term.Branch` (test cond,cond; je ...) mis-
    // encodes, producing garbage results for while loops with accumulation.
    cg.builder.branch(is_true, body_blk, &[], exit_blk, &[]);

    // Body block
    cg.builder.switch_to_block(body_blk);
    cg.terminated = false;
    cg.loops.push((cond_blk, exit_blk));
    codegen_block(cg, body_node);
    cg.loops.pop();
    if !cg.terminated {
        cg.builder.jump(cond_blk, &[]);
        cg.terminated = true;
    }

    // Exit block
    cg.builder.switch_to_block(exit_blk);
    cg.terminated = false;
}

// ── for_stmt ──

fn codegen_for(cg: &mut CodegenCtx, node: AstRef<'_>) {
    // for (init?; cond?; update?) { body }
    // Desugars to:
    //   init;
    //   cond_blk: if (cond) goto body_blk else goto exit_blk
    //   body_blk: { body }; jump update_blk
    //   update_blk: update; jump cond_blk
    //   exit_blk:
    // 注：`continue` 的目标是 **update_blk**（不是 cond_blk）——C 语义要求
    // for 的 continue 先执行 update 再判条件；旧实现把 cond_blk 入循环栈会跳过
    // update → 自增丢失、循环不终止（dual_backend 补 continue 用例时实证挂死）。

    // Init (optional)
    if let Some(Some(init_node)) = node.get_optional("init") {
        codegen_for_init(cg, init_node);
    }

    let cond_blk = cg.builder.create_block();
    let body_blk = cg.builder.create_block();
    let update_blk = cg.builder.create_block();
    let exit_blk = cg.builder.create_block();

    cg.builder.jump(cond_blk, &[]);

    // Condition block
    cg.builder.switch_to_block(cond_blk);
    if let Some(Some(cond_node)) = node.get_optional("condition") {
        let cond_val = codegen_expr(cg, cond_node);
        let zero = cg.builder.iconst_i32(0);
        let is_true = cg.builder.icmp(IntCC::NotEqual, cond_val, zero);
        cg.builder.branch(is_true, body_blk, &[], exit_blk, &[]);
    } else {
        // No condition → always enter body
        cg.builder.jump(body_blk, &[]);
    }

    // Body block
    cg.builder.switch_to_block(body_blk);
    cg.terminated = false;
    cg.loops.push((update_blk, exit_blk));

    let body_node = node.get_child("body").expect("for missing body");
    codegen_block(cg, body_node);

    if !cg.terminated {
        cg.builder.jump(update_blk, &[]);
        cg.terminated = true;
    }
    cg.loops.pop();

    // Update block（continue 落点；无 update 时直通条件块）
    cg.builder.switch_to_block(update_blk);
    cg.terminated = false;
    if let Some(Some(update_node)) = node.get_optional("update") {
        codegen_for_update(cg, update_node);
    }
    if !cg.terminated {
        cg.builder.jump(cond_blk, &[]);
        cg.terminated = true;
    }

    cg.builder.switch_to_block(exit_blk);
    cg.terminated = false;
}

fn codegen_for_init(cg: &mut CodegenCtx, node: AstRef<'_>) {
    let name = node.get_text("name").unwrap_or("_").to_string();
    let slot = cg.alloc_slot();
    let val = codegen_expr(cg, node.get_child("value").expect("for_init value"));
    cg.builder.store(val, slot);
    cg.syms.locals.insert(name, slot);
}

fn codegen_for_update(cg: &mut CodegenCtx, node: AstRef<'_>) {
    let name = node.get_text("name").unwrap_or("_");
    let ptr = *cg.syms.locals.get(name).expect("for_update var not found");
    let val = codegen_expr(cg, node.get_child("value").expect("for_update value"));
    cg.builder.store(val, ptr);
}

// ── do_while_stmt ──

fn codegen_do_while(cg: &mut CodegenCtx, node: AstRef<'_>) {
    // do { body } while (cond);
    // Desugars to:
    //   body_blk: { body }
    //   cond_blk: if (cond) goto body_blk else goto exit_blk
    //   exit_blk:

    let body_node = node.get_child("body").expect("do_while missing body");
    let cond_node = node
        .get_child("condition")
        .expect("do_while missing condition");

    let body_blk = cg.builder.create_block();
    let cond_blk = cg.builder.create_block();
    let exit_blk = cg.builder.create_block();

    cg.builder.jump(body_blk, &[]);

    // Body block
    cg.builder.switch_to_block(body_blk);
    cg.terminated = false;
    cg.loops.push((cond_blk, exit_blk));
    codegen_block(cg, body_node);
    cg.loops.pop();
    if !cg.terminated {
        cg.builder.jump(cond_blk, &[]);
        cg.terminated = true;
    }

    // Condition block
    cg.builder.switch_to_block(cond_blk);
    let cond_val = codegen_expr(cg, cond_node);
    let zero = cg.builder.iconst_i32(0);
    let is_true = cg.builder.icmp(IntCC::NotEqual, cond_val, zero);
    cg.builder.branch(is_true, body_blk, &[], exit_blk, &[]);

    // Exit block
    cg.builder.switch_to_block(exit_blk);
    cg.terminated = false;
}

// ── break_stmt ──

fn codegen_break(cg: &mut CodegenCtx) {
    let (_, exit_blk) = *cg.loops.last().expect("break outside loop");
    cg.builder.jump(exit_blk, &[]);
    cg.terminated = true;
}

// ── continue_stmt ──

fn codegen_continue(cg: &mut CodegenCtx) {
    let (cond_blk, _) = *cg.loops.last().expect("continue outside loop");
    cg.builder.jump(cond_blk, &[]);
    cg.terminated = true;
}

// ============================================================
// Expression codegen
// ============================================================

/// Flatten transparent wrappers (rep, opt, seq) from AST children.
/// The lowering engine doesn't flatten these, so the AST preserves them.
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

fn codegen_expr(cg: &mut CodegenCtx, node: AstRef<'_>) -> Value {
    match node.kind() {
        // ── Precedence levels (each may have rep/opt wrappers as children) ──
        "assignment" => codegen_assign_expr(cg, node),
        "logical_or" => codegen_logical_or(cg, node),
        "logical_and" => codegen_logical_and(cg, node),
        "bitwise_or" => codegen_bitwise_binary(cg, node),
        "bitwise_xor" => codegen_bitwise_binary(cg, node),
        "bitwise_and" => codegen_bitwise_binary(cg, node),
        "equality" => codegen_binary_comparison(cg, node),
        "relational" => codegen_binary_comparison(cg, node),
        "shift" => codegen_bitwise_binary(cg, node),
        "additive" => codegen_binary_arith(cg, node),
        "multiplicative" => codegen_binary_arith(cg, node),
        "unary" => codegen_unary_expr(cg, node),

        // ── Terminals (leaf tokens) ──
        "NUMBER" => {
            let text = node.text().unwrap_or("0");
            let val = parse_number(text);
            cg.builder.iconst_i32(val)
        }
        "CHAR" => {
            let text = node.text().unwrap_or("'\\0'");
            // Extract the character between the quotes
            let ch = text.chars().nth(1).unwrap_or('\0');
            cg.builder.iconst_i32(ch as i32)
        }
        "IDENT" => {
            let name = node.text().unwrap_or("_");
            // Check enum values first
            if let Some(&val) = cg.syms.enum_values.get(name) {
                return cg.builder.iconst_i32(val);
            }
            // Then check variables
            let ptr = cg
                .syms
                .locals
                .get(name)
                .unwrap_or_else(|| panic!("undefined variable: {}", name));
            cg.builder.load(*ptr, TypeId::I32)
        }

        // ── Member access (struct field read) ──
        "member_access" => codegen_member_access(cg, node),

        // ── Function call ──
        "call" => codegen_call(cg, node),

        // ── Parenthesized expr ──
        // After lowering, has children [LPAREN, expr, RPAREN] (with possible rep/opt wrappers).
        // Also handles primary node with single child.
        "primary" => {
            let fc = flat_children(node);
            if fc.len() == 1 {
                codegen_expr(cg, fc[0])
            } else if fc.len() == 3 {
                // ( expr ) — take the middle
                codegen_expr(cg, fc[1])
            } else {
                panic!("unexpected primary children: {} items", fc.len());
            }
        }

        _other => {
            let fc = flat_children(node);
            // 表达式级赋值（含成员）：`x += 4` / `p.x += 4` 经 expr_stmt 以
            // **seq** 形态到达（children = [IDENT|member_access, OP 令牌, rhs]）——
            // 语句级 assign_stmt 只接受成员 `=`，复合形式全走这里。
            // OP 令牌文本必须取自**源码区间**：多字符运算符的 token text 是
            // `PUNCT_2b3d` 这类名字而非 "+="。旧实现直接落入下方二元链 → 赋值
            // 被静默丢弃（成员复合赋值 dual_backend 用例实证：返回未修改值）。
            if fc.len() >= 3
                && matches!(fc[0].kind(), "IDENT" | "member_access")
                && is_assign_op(cg.source, fc[1])
            {
                return codegen_assign_expr(cg, node);
            }
            if fc.len() == 1 {
                codegen_expr(cg, fc[0])
            } else {
                // Try to handle as a generic binary operation chain
                // This handles seq nodes that survive lowering
                let mut lhs = codegen_expr(cg, fc[0]);
                let mut i = 1;
                while i + 1 < fc.len() {
                    let op = fc[i].text().unwrap_or("?");
                    let rhs = codegen_expr(cg, fc[i + 1]);
                    lhs = match op {
                        "+" => cg.builder.iadd(lhs, rhs),
                        "-" => cg.builder.isub(lhs, rhs),
                        "*" => cg.builder.imul(lhs, rhs),
                        "/" => cg.builder.sdiv(lhs, rhs),
                        "%" => cg.builder.srem(lhs, rhs),
                        "&" => cg.builder.band(lhs, rhs),
                        "|" => cg.builder.bor(lhs, rhs),
                        "^" => cg.builder.bxor(lhs, rhs),
                        "<<" => cg.builder.ishl(lhs, rhs),
                        ">>" => cg.builder.sshr(lhs, rhs),
                        "==" | "!=" | "<" | ">" | "<=" | ">=" => {
                            let cc = match op {
                                "==" => IntCC::Equal,
                                "!=" => IntCC::NotEqual,
                                "<" => IntCC::SignedLessThan,
                                ">" => IntCC::SignedGreaterThan,
                                "<=" => IntCC::SignedLessThanOrEqual,
                                ">=" => IntCC::SignedGreaterThanOrEqual,
                                _ => IntCC::Equal,
                            };
                            let cmp = cg.builder.icmp(cc, lhs, rhs);
                            cg.builder.sextend(cmp, TypeId::I32)
                        }
                        _ => rhs, // unrecognized
                    };
                    i += 2;
                }
                lhs
            }
        }
    }
}

/// Parse a NUMBER token that may be hex (0x), octal (0), or decimal.
/// Parse a NUMBER token that may be hex (0x), octal (0), or decimal.
/// Public for use by compiler.rs pre-processing.
pub fn parse_number_static(text: &str) -> i32 {
    parse_number(text)
}

fn parse_number(text: &str) -> i32 {
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).unwrap_or(0) as i32
    } else if text.starts_with('0') && text.len() > 1 {
        i64::from_str_radix(&text[1..], 8).unwrap_or(0) as i32
    } else {
        text.parse::<i64>().unwrap_or(0) as i32
    }
}

// ── Assignment (x = expr, x += expr, …) ──

fn codegen_assign_expr(cg: &mut CodegenCtx, node: AstRef<'_>) -> Value {
    let children = flat_children(node);
    // assignment has: [IDENT, OP, rhs] for x = expr or x += expr
    // or: [sub_expr] for simple logical_or (no assignment)
    if children.len() >= 2 {
        // 运算符优先按令牌源码区间取（多字符运算符 token text 是 PUNCT_xxxx），
        // 取不到再回退整段扫描（assign_stmt 形态）。
        let op = children
            .get(1)
            .and_then(|t| tok_assign_op(cg.source, *t))
            .map(|s| s.to_string())
            .unwrap_or_else(|| cg.extract_assign_op(node));
        // Check if this is an assignment by looking at the children
        let has_ident = children[0].kind() == "IDENT";
        if has_ident {
            let name = children[0].text().unwrap_or("_");
            // Check if the name is in the symbol table (it's an assignment target)
            if let Some(&ptr) = cg.syms.locals.get(name) {
                let rhs = codegen_expr(cg, children[children.len() - 1]);
                let result = apply_compound_op(cg, &op, ptr, rhs);
                cg.builder.store(result, ptr);
                return result;
            }
        }
        // [member_access, OP, rhs] → 成员赋值（`p.x = 4` / `p.x += 4`）：
        // 表达式级 assignment 规则允许成员作 LHS（grammar: assignment ::=
        // member_access OP assignment），而语句级 assign_stmt 只接受 `=`——
        // `p.x += 4` 因此走 expr_stmt 到这里。旧实现只认 IDENT → **静默丢弃**
        // 整个赋值（dual_backend 成员复合赋值用例实证：两后端都返回未修改值）。
        if children[0].kind() == "member_access" {
            let ma_idents = children[0].get_children("idents");
            let obj_name = ma_idents.first().and_then(|c| c.text()).unwrap_or("_");
            let field_name = ma_idents.get(1).and_then(|c| c.text()).unwrap_or("_");
            let field_key = format!("{}.{}", obj_name, field_name);
            if let Some(&field_slot) = cg.syms.locals.get(&field_key) {
                let rhs = codegen_expr(cg, children[children.len() - 1]);
                let result = apply_compound_op(cg, &op, field_slot, rhs);
                cg.builder.store(result, field_slot);
                return result;
            }
        }
    }
    // Not an assignment (just a logical_or) — process first child
    if children.is_empty() {
        return cg.builder.iconst_i32(0);
    }
    codegen_expr(cg, children[0])
}

// ── Enum definition ──

fn codegen_enum_def(cg: &mut CodegenCtx, node: AstRef<'_>) {
    // Enum values are pre-processed in compiler.rs before codegen.
    // No-op here — just ensure we don't overwrite already-registered values.
    let _ = node;
    let _ = cg;
}

// ── Struct definition (register field names for per-field slot allocation) ──
// Each struct field gets its own stack slot (accessed as var.field_name).
// This avoids iadd(ptr, offset) which the x86 backend mis-encodes for non-zero offsets.

fn codegen_struct_def(cg: &mut CodegenCtx, node: AstRef<'_>) {
    let name = node.get_text("name").unwrap_or("_").to_string();
    let fields = node.get_children("fields");
    let field_names: Vec<String> = fields
        .iter()
        .map(|f| f.get_text("name").unwrap_or("_").to_string())
        .collect();
    cg.syms.structs.insert(
        name,
        StructInfo {
            fields: field_names.iter().map(|n| (n.clone(), 0)).collect(),
            total_size: (field_names.len() as i32) * 4,
        },
    );
}

/// Helper: allocate a separate stack slot for each field of a struct variable.
/// Inserts `var_name.field_name` entries into locals for direct access.
fn alloc_struct_fields(cg: &mut CodegenCtx, struct_name: &str, var_name: &str) {
    let field_names: Vec<String> = cg
        .syms
        .structs
        .get(struct_name)
        .map(|s| s.fields.iter().map(|(n, _)| n.clone()).collect())
        .unwrap_or_default();
    cg.syms
        .var_structs
        .insert(var_name.to_string(), struct_name.to_string());
    // Store field name list for struct assignment
    for fname in &field_names {
        let slot = cg.alloc_slot();
        cg.syms
            .locals
            .insert(format!("{}.{}", var_name, fname), slot);
    }
    // Store a sentinel value for the root var name (used for type lookup)
    let sentinel = cg.alloc_slot();
    cg.syms.locals.insert(var_name.to_string(), sentinel);
}

// ── Struct variable declaration ──

fn codegen_struct_decl(cg: &mut CodegenCtx, node: AstRef<'_>) {
    let idents = node.get_children("idents");
    let struct_name = idents.first().and_then(|c| c.text()).unwrap_or("_");
    let var_name = idents.get(1).and_then(|c| c.text()).unwrap_or("_");
    alloc_struct_fields(cg, struct_name, var_name);
}

// ── Struct init (declaration with initialization) ──

fn codegen_struct_init(cg: &mut CodegenCtx, node: AstRef<'_>) {
    let idents = node.get_children("idents");
    let struct_name = idents.first().and_then(|c| c.text()).unwrap_or("_");
    let var_name = idents.get(1).and_then(|c| c.text()).unwrap_or("_");

    alloc_struct_fields(cg, struct_name, var_name);

    // Initialize each field from the init list
    let field_names: Vec<String> = cg
        .syms
        .structs
        .get(struct_name)
        .map(|s| s.fields.iter().map(|(n, _)| n.clone()).collect())
        .unwrap_or_default();
    let values = node.get_children("values");
    for (i, val_node) in values.iter().enumerate() {
        if i < field_names.len() {
            let val = codegen_expr(cg, *val_node);
            let field_key = format!("{}.{}", var_name, field_names[i]);
            if let Some(&slot) = cg.syms.locals.get(&field_key) {
                cg.builder.store(val, slot);
            }
        }
    }
}

// ── Member access (p.x → load the field value) ──

fn codegen_member_access(cg: &mut CodegenCtx, node: AstRef<'_>) -> Value {
    let idents = node.get_children("idents");
    let obj_name = idents.first().and_then(|c| c.text()).unwrap_or("_");
    let field_name = idents.get(1).and_then(|c| c.text()).unwrap_or("_");

    // Each field has its own slot: obj.field
    let field_key = format!("{}.{}", obj_name, field_name);
    let ptr = *cg
        .syms
        .locals
        .get(&field_key)
        .unwrap_or_else(|| panic!("undefined field: {}", field_key));
    cg.builder.load(ptr, TypeId::I32)
}

/// Apply a compound assignment: read old value, compute `old op rhs`, return new value.
fn apply_compound_op(cg: &mut CodegenCtx, op: &str, ptr: Value, rhs: Value) -> Value {
    match op {
        "=" => rhs, // simple assign: no need to load old value
        _ => {
            let old = cg.builder.load(ptr, TypeId::I32);
            match op {
                "+=" => cg.builder.iadd(old, rhs),
                "-=" => cg.builder.isub(old, rhs),
                "*=" => cg.builder.imul(old, rhs),
                "/=" => cg.builder.sdiv(old, rhs),
                "%=" => cg.builder.srem(old, rhs),
                "&=" => cg.builder.band(old, rhs),
                "|=" => cg.builder.bor(old, rhs),
                "^=" => cg.builder.bxor(old, rhs),
                "<<=" => cg.builder.ishl(old, rhs),
                ">>=" => cg.builder.sshr(old, rhs),
                _ => rhs,
            }
        }
    }
}

// ── Bitwise binary (&, |, ^, <<, >>) ──

fn codegen_bitwise_binary(cg: &mut CodegenCtx, node: AstRef<'_>) -> Value {
    let children = flat_children(node);
    if children.is_empty() {
        return cg.builder.iconst_i32(0);
    }
    if children.len() == 1 {
        return codegen_expr(cg, children[0]);
    }
    let mut lhs = codegen_expr(cg, children[0]);
    let mut i = 1;
    while i < children.len() {
        let op = children[i].text().unwrap_or("?");
        let rhs = codegen_expr(cg, children[i + 1]);
        lhs = match op {
            "&" => cg.builder.band(lhs, rhs),
            "|" => cg.builder.bor(lhs, rhs),
            "^" => cg.builder.bxor(lhs, rhs),
            "<<" => cg.builder.ishl(lhs, rhs),
            ">>" => cg.builder.sshr(lhs, rhs),
            _ => lhs, // unrecognized op → return left as-is
        };
        i += 2;
    }
    lhs
}

// ── Binary arithmetic (+, -, *, /, %) ──

fn codegen_binary_arith(cg: &mut CodegenCtx, node: AstRef<'_>) -> Value {
    let children = flat_children(node);
    if children.is_empty() {
        return cg.builder.iconst_i32(0);
    }
    let mut lhs = codegen_expr(cg, children[0]);
    let mut i = 1;
    while i < children.len() {
        let op = children[i].text().unwrap_or("?");
        let rhs = codegen_expr(cg, children[i + 1]);
        lhs = match op {
            "+" => cg.builder.iadd(lhs, rhs),
            "-" => cg.builder.isub(lhs, rhs),
            "*" => cg.builder.imul(lhs, rhs),
            "/" => cg.builder.sdiv(lhs, rhs),
            "%" => cg.builder.srem(lhs, rhs),
            _ => lhs, // unrecognized op → return left as-is
        };
        i += 2;
    }
    lhs
}

// ── Binary comparison (==, !=, <, >, <=, >=) ──

fn codegen_binary_comparison(cg: &mut CodegenCtx, node: AstRef<'_>) -> Value {
    let children = flat_children(node);
    if children.is_empty() {
        return cg.builder.iconst_i32(0);
    }
    if children.len() == 1 {
        return codegen_expr(cg, children[0]);
    }
    let mut lhs = codegen_expr(cg, children[0]);
    let mut i = 1;
    while i < children.len() {
        let op = children[i].text().unwrap_or("?");
        let rhs = codegen_expr(cg, children[i + 1]);
        let cc = match op {
            "==" => IntCC::Equal,
            "!=" => IntCC::NotEqual,
            "<" => IntCC::SignedLessThan,
            ">" => IntCC::SignedGreaterThan,
            "<=" => IntCC::SignedLessThanOrEqual,
            ">=" => IntCC::SignedGreaterThanOrEqual,
            _ => return lhs, // unrecognized → return left as-is
        };
        let cmp = cg.builder.icmp(cc, lhs, rhs);
        lhs = cg.builder.sextend(cmp, TypeId::I32);
        i += 2;
    }
    lhs
}

// ── Logical and (&&) — non-short-circuit ──

fn codegen_logical_and(cg: &mut CodegenCtx, node: AstRef<'_>) -> Value {
    let children = flat_children(node);
    if children.is_empty() {
        return cg.builder.iconst_i32(0);
    }
    if children.len() == 1 {
        return codegen_expr(cg, children[0]);
    }
    let zero = cg.builder.iconst_i32(0);

    let lhs = codegen_expr(cg, children[0]);
    let mut result = cg.builder.icmp(IntCC::NotEqual, lhs, zero);

    let mut i = 1;
    while i < children.len() {
        let rhs = codegen_expr(cg, children[i + 1]);
        let rhs_ne = cg.builder.icmp(IntCC::NotEqual, rhs, zero);
        result = cg.builder.band(result, rhs_ne);
        i += 2;
    }
    cg.builder.sextend(result, TypeId::I32)
}

// ── Logical or (||) — non-short-circuit ──

fn codegen_logical_or(cg: &mut CodegenCtx, node: AstRef<'_>) -> Value {
    let children = flat_children(node);
    if children.is_empty() {
        return cg.builder.iconst_i32(0);
    }
    if children.len() == 1 {
        return codegen_expr(cg, children[0]);
    }
    let zero = cg.builder.iconst_i32(0);

    let lhs = codegen_expr(cg, children[0]);
    let mut result = cg.builder.icmp(IntCC::NotEqual, lhs, zero);

    let mut i = 1;
    while i < children.len() {
        let rhs = codegen_expr(cg, children[i + 1]);
        let rhs_ne = cg.builder.icmp(IntCC::NotEqual, rhs, zero);
        result = cg.builder.bor(result, rhs_ne);
        i += 2;
    }
    cg.builder.sextend(result, TypeId::I32)
}

// ── Unary (-, +, !, ~) ──

fn codegen_unary_expr(cg: &mut CodegenCtx, node: AstRef<'_>) -> Value {
    let children = flat_children(node);
    if children.is_empty() {
        return cg.builder.iconst_i32(0);
    }
    if children.len() == 1 {
        return codegen_expr(cg, children[0]);
    }
    // children: [operator_token, operand]
    let op = children[0].text().unwrap_or("");
    let operand = codegen_expr(cg, children[1]);
    match op {
        "-" => {
            let zero = cg.builder.iconst_i32(0);
            cg.builder.isub(zero, operand)
        }
        "+" => operand, // unary plus is no-op
        "!" => {
            let zero = cg.builder.iconst_i32(0);
            let cmp = cg.builder.icmp(IntCC::Equal, operand, zero);
            cg.builder.sextend(cmp, TypeId::I32)
        }
        "~" => cg.builder.bnot(operand),
        _ => operand, // no recognized operator: return operand directly
    }
}

// ── Function call (implemented via AST-level inlining) ──

fn codegen_call(cg: &mut CodegenCtx, node: AstRef<'_>) -> Value {
    let name = node.get_text("name").unwrap_or("_");

    // 真实 Call 判定：
    // 1. callee 是递归函数（函数体内含对自身的调用）——内联会无限展开，
    //    任何调用点都改真实 Call（运行时递归）。
    // 2. callee 已在本函数的内联链中（互递归/深层内联）→ 真实 Call。
    let callee_id = *cg
        .syms
        .func_defs
        .get(name)
        .unwrap_or_else(|| panic!("undefined function: {}", name));
    let callee_node = cg.ast.ref_to(callee_id);
    let callee_is_recursive = callee_is_self_calling(callee_node, name);

    if callee_is_recursive || cg.inlining.iter().any(|f| f == name) {
        // Evaluate call arguments
        let arg_vals: Vec<Value> = match node.get_optional("args") {
            Some(Some(arg_list)) => {
                let items = arg_list.get_children("items");
                let mut vals = Vec::new();
                for a in &items {
                    vals.push(codegen_expr(cg, *a));
                }
                vals
            }
            _ => vec![],
        };
        let func_ref = *cg.syms.funcs.get(name).unwrap_or_else(|| {
            panic!(
                "recursive call to '{}' but FuncRef not pre-registered",
                name
            )
        });
        let r = cg.builder.call(func_ref, &arg_vals, &[TypeId::I32]);
        return r
            .first()
            .copied()
            .unwrap_or_else(|| cg.builder.iconst_i32(0));
    }

    // Evaluate call arguments
    let arg_vals: Vec<Value> = match node.get_optional("args") {
        Some(Some(arg_list)) => {
            let items = arg_list.get_children("items");
            let mut vals = Vec::new();
            for a in &items {
                vals.push(codegen_expr(cg, *a));
            }
            vals
        }
        _ => vec![],
    };

    let params = callee_node.get_children("params");

    // Save caller locals that would be shadowed by params
    let mut saved_locals: HashMap<String, Value> = HashMap::new();
    for param in &params {
        let pname = param.get_text("name").unwrap_or("_").to_string();
        if let Some(old) = cg.syms.locals.remove(&pname) {
            saved_locals.insert(pname, old);
        }
    }

    // Allocate return value slot
    let ret_slot = cg.alloc_slot();

    // Allocate parameter slots and store argument values
    for (i, param) in params.iter().enumerate() {
        let pname = param.get_text("name").unwrap_or("_").to_string();
        let slot = cg.alloc_slot();
        let val = arg_vals
            .get(i)
            .copied()
            .unwrap_or_else(|| cg.builder.iconst_i32(0));
        cg.builder.store(val, slot);
        cg.syms.locals.insert(pname, slot);
    }

    // Push inlining context and codegen the callee body directly inline
    let old_return_slot = cg.return_slot;
    let old_return_block = cg.return_block;
    let after_blk = cg.builder.create_block();
    cg.return_slot = Some(ret_slot);
    cg.return_block = Some(after_blk);
    cg.inlining.push(name.to_string());

    let body = callee_node.get_child("body").expect("callee missing body");
    codegen_block(cg, body);

    // If the body didn't return, store 0 and jump to after
    if !cg.terminated {
        let zero = cg.builder.iconst_i32(0);
        cg.builder.store(zero, ret_slot);
        cg.builder.jump(after_blk, &[]);
    }

    // Restore inlining context and continue in after block
    cg.return_slot = old_return_slot;
    cg.return_block = old_return_block;
    cg.terminated = false;
    cg.inlining.pop();
    cg.builder.switch_to_block(after_blk);

    // Restore shadowed caller locals
    for (local_name, slot) in saved_locals {
        cg.syms.locals.insert(local_name, slot);
    }

    // Load and return the inlined function's return value
    cg.builder.load(ret_slot, TypeId::I32)
}

/// 判断函数是否递归：函数体内存在对自身的调用（`call` 节点的 name == self）。
/// 递归函数无法安全内联（无限展开），任何调用点都应改真实 Call。
fn callee_is_self_calling(func_node: AstRef<'_>, self_name: &str) -> bool {
    // 遍历函数体找 call 节点，检查其 name 是否等于自身
    let body = match func_node.get_child("body") {
        Some(b) => b,
        None => return false,
    };
    body.find_all("call")
        .iter()
        .any(|c| c.get_text("name").map(|n| n == self_name).unwrap_or(false))
}
