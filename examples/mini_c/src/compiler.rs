//! Compiler pipeline — orchestrates the full compilation flow:
//! grammar → parse → schema lowering → AST → codegen → JIT → execute.
//!
//! The public entry point is `compile_and_run(source)` which takes a Mini C
//! source string, compiles all functions to x86_64 machine code, calls `main()`,
//! and returns its exit code.

use code_forge::backend::jit::JitCompiler;
use code_forge::backend::x86_v12::ensure_registered;
use code_forge::forge_grammar::{AstVisitor, Parser};
use code_forge::ir::Module;

use crate::codegen::{SymTable, codegen_function};
use crate::codegen_hir::codegen_function_hir;
use crate::grammar::build_grammar;
use crate::schema::build_schema;

/// Which IR-generation backend to use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    /// Direct FunctionBuilder codegen (`codegen.rs`).
    Direct,
    /// forge-hir pipeline (`codegen_hir.rs`): IrGraph + HirCtx + lowering.
    Hir,
    /// v12 DSL 后端（`x86_v12` TargetMachine）。Direct/Hir 均用同一 v12 机器
    /// 编译（v11 语法层已删除），区别仅在 IR 构建路径。
    V12,
}

/// Walk the AST expression tree to find a constant NUMBER/CHAR token and parse its value.
fn resolve_enum_value(node: code_forge::forge_grammar::AstRef<'_>) -> i32 {
    match node.kind() {
        "NUMBER" => node
            .text()
            .map(crate::codegen::parse_number_static)
            .unwrap_or(0),
        "CHAR" => node
            .text()
            .and_then(|t| t.chars().nth(1))
            .map(|c| c as i32)
            .unwrap_or(0),
        _ => {
            // Walk children backward to prefer VALUE children over wrapper nodes
            // Each expression level like 'unary' has: [opt (wrapper), NUMBER (value)]
            for child in node.children().iter().rev() {
                let kind = child.kind();
                match kind {
                    "opt" | "rep" | "seq" => continue,
                    _ => return resolve_enum_value(*child),
                }
            }
            0
        }
    }
}

/// Compile the Mini C source and run `main()`, returning its return value.
///
/// Uses the direct backend ([`Backend::Direct`]); see [`compile_and_run_with`]
/// to select the forge-hir backend.
///
/// # Errors
/// Returns an error string if parsing, lowering, codegen, or JIT compilation fails.
pub fn compile_and_run(source: &str) -> Result<i32, String> {
    compile_and_run_with(source, Backend::Direct)
}

/// Compile the Mini C source with the given backend and run `main()`.
///
/// # Errors
/// Returns an error string if parsing, lowering, codegen, or JIT compilation fails.
pub fn compile_and_run_with(source: &str, backend: Backend) -> Result<i32, String> {
    // 1. Load grammar
    let grammar = build_grammar();

    // 2. Build schema
    let schema = build_schema();

    // 3. Parse + lower to AST in one call
    let parser = Parser::build(grammar);
    let ast = parser
        .parse_to_ast(source, &schema)
        .map_err(|e| format!("parse/lower error: {}", e))?;

    // 4. Scan function definitions from the AST
    let funcs: Vec<_> = ast.root_ref().find_all("func_def");

    if funcs.is_empty() {
        return Err("no functions defined (need at least 'main')".into());
    }

    // 5. Codegen — pre-process enum/struct definitions, then codegen functions
    let mut module = Module::new();
    let mut syms = SymTable::new();

    // Pre-process: register enum values (walks expression tree for constants)
    for enum_node in ast.root_ref().find_all("enum_def") {
        let items = enum_node.get_children("items");
        let mut next_val: i32 = 0;
        for item in &items {
            let name = item.get_text("name").unwrap_or("_").to_string();
            if let Some(Some(val_node)) = item.get_optional("value") {
                // Walk the expression chain to find the constant NUMBER token
                next_val = resolve_enum_value(val_node);
            }
            syms.enum_values.insert(name.clone(), next_val);
            next_val += 1;
        }
    }

    // First pass: register all function AST nodes for inlining
    for func_node in &funcs {
        let name = func_node.get_text("name").unwrap_or("_").to_string();
        syms.func_defs.insert(name, func_node.id);
    }

    // 预注册：为每个函数创建占位 Function（仅 ret 0），拿 FuncRef 填入
    // syms.funcs——递归函数编译体时需要用自身 FuncRef 生成自调用 Call
    //（内联检测到递归 → 真实 Call）。占位体随后被 codegen_function 覆盖。
    use code_forge::ir::FunctionSignature;
    for func_node in &funcs {
        let name = func_node.get_text("name").unwrap_or("_").to_string();
        // 参数列表（i32 全部；占位体参数名占位 "_"——真实体覆盖时重命名）
        let params: Vec<(code_forge::ir::TypeId, String)> = func_node
            .get_children("params")
            .iter()
            .map(|p| {
                let pname = p.get_text("name").unwrap_or("_").to_string();
                (code_forge::ir::TypeId::I32, pname)
            })
            .collect();
        let params_refs: Vec<(code_forge::ir::TypeId, &str)> =
            params.iter().map(|(t, n)| (*t, n.as_str())).collect();
        let sig = FunctionSignature::new(&params_refs, &[code_forge::ir::TypeId::I32]);
        let mut b = code_forge::ir::FunctionBuilder::new(name.as_str(), module.types.clone(), sig);
        b.create_block_here();
        let zero = b.iconst_i32(0);
        b.ret(&[zero]);
        let fr = module.add_function(b.finish().expect("placeholder"));
        syms.funcs.insert(name, fr);
    }

    // Second pass: codegen each function
    for func_node in &funcs {
        let name = func_node.get_text("name").unwrap_or("_").to_string();
        let pre = syms.funcs.get(&name).copied();
        let func_ref = match backend {
            Backend::Direct | Backend::V12 => {
                codegen_function(&mut module, *func_node, &mut syms, &ast, source, pre)
            }
            Backend::Hir => codegen_function_hir(&mut module, *func_node, &mut syms, &ast, source),
        }
        .map_err(|e| format!("codegen error in '{}': {}", name, e))?;
        syms.funcs.insert(name, func_ref);
    }

    // 6. JIT compile the entire module (handles cross-function relocations)
    ensure_registered();
    let mut jit: Box<dyn JitRunner> = Box::new(JitRunnerImpl(JitCompiler::new(
        code_forge::backend::x86_v12::TargetMachine::new(),
    )));
    jit.compile_module(&module)
        .map_err(|e| format!("JIT compile error: {}", e))?;

    // 7. Call main()
    let main_fn: extern "C" fn() -> i32 = jit
        .get_main()
        .map_err(|e| format!("JIT get_fn error: {}", e))?;

    Ok(main_fn())
}

/// JIT 运行器 — v12 唯一后端（x86_v12 TargetMachine）。
struct JitRunnerImpl(JitCompiler<code_forge::backend::x86_v12::TargetMachine>);

trait JitRunner {
    fn compile_module(&mut self, module: &Module) -> Result<(), String>;
    fn get_main(&self) -> Result<extern "C" fn() -> i32, String>;
}

impl JitRunner for JitRunnerImpl {
    fn compile_module(&mut self, module: &Module) -> Result<(), String> {
        self.0.compile_module(module).map_err(|e| e.to_string())
    }
    fn get_main(&self) -> Result<extern "C" fn() -> i32, String> {
        self.0.get_fn("main").map_err(|e| e.to_string())
    }
}

/// Print the AST structure for debugging.
pub fn dump_ast(source: &str) -> Result<String, String> {
    let grammar = build_grammar();
    let schema = build_schema();
    let parser = Parser::build(grammar);
    let ast = parser
        .parse_to_ast(source, &schema)
        .map_err(|e| format!("parse error: {}", e))?;

    let mut printer = code_forge::forge_grammar::TreePrinter::new();
    printer.walk(&ast);
    Ok(printer.finish())
}
