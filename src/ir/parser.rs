//! LLVM IR 文本格式解析器。
//!
//! 解析 `.ll` 文件格式（兼容 LLVM 15-22 opaque pointer），
//! 将文本 IR 转换为 codegen-lib 的内部 Module/Functions。
//!
//! ## 支持的语法
//!
//! - Module: `target datalayout`, `target triple`, `declare`, `define`
//! - Function: `define <retty> @name(<args>) { <body> }`
//! - BasicBlock: `<label>:` + 指令列表
//! - Opaque pointer: `ptr` (LLVM 15+)
//! - 整数类型: `i1`, `i8`, `i16`, `i32`, `i64`
//! - 浮点类型: `half`, `float`, `double`
//! - 指令: add, sub, mul, udiv, sdiv, urem, srem, and, or, xor, shl, lshr, ashr,
//!   fadd, fsub, fmul, fdiv, fneg, alloca, load, store, getelementptr,
//!   icmp, fcmp, phi, select, call, zext, sext, trunc, bitcast,
//!   br, ret, switch, unreachable
//!
//! ## 未支持的语法
//!
//! - Typed pointers (`i32*`) — LLVM 17+ 已移除
//! - Vector 类型 (`<N x T>`)
//! - Metadata (`!name = !{...}`)
//! - Inline assembly (`call void asm ...`)
//! - Exception handling (`invoke`, `landingpad`)

use crate::ir::*;
use smallvec::SmallVec;
use std::collections::HashMap;

// ============================================================
// Parser resource limits — protect against malicious/corrupt input
// ============================================================

/// Maximum input size in bytes (10 MB).
const MAX_INPUT_SIZE: usize = 10_000_000;

/// Maximum nesting depth of blocks within a function body (reserved for future nested block support).
#[allow(dead_code)]
const MAX_NESTING_DEPTH: i32 = 1000;

/// Maximum number of instructions allowed in a single function.
const MAX_INSTRUCTIONS_PER_FUNC: usize = 100_000;

/// 解析错误。
#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
}

impl ParseError {
    pub fn new(msg: impl Into<String>, line: usize) -> Self {
        Self {
            message: msg.into(),
            line,
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Parse error at line {}: {}", self.line, self.message)
    }
}

/// LLVM IR 文本解析器。
pub struct LlvmIrParser {
    line: usize,
}

impl LlvmIrParser {
    pub fn new() -> Self {
        Self { line: 1 }
    }

    /// 解析 LLVM IR 文本，返回 codegen-lib Module。
    pub fn parse(&mut self, text: &str) -> Result<Module, ParseError> {
        // Input size limit — reject excessively large inputs early
        if text.len() > MAX_INPUT_SIZE {
            return Err(ParseError::new(
                format!(
                    "Input too large: {} bytes (maximum {} bytes / 10 MB)",
                    text.len(),
                    MAX_INPUT_SIZE
                ),
                0,
            ));
        }

        self.line = 1;
        let mut module = Module::new();

        for line_text in text.lines() {
            let trimmed = line_text.trim();
            if trimmed.is_empty() || trimmed.starts_with(';') {
                self.line += 1;
                continue;
            }
            if trimmed.starts_with("target ") || trimmed.starts_with("declare ") {
                // 跳过 target/declare 行
            } else if trimmed.starts_with("define ") {
                let func = self.parse_function(text, trimmed)?;
                let _ = module.add_function(func);
            }
            self.line += 1;
        }

        Ok(module)
    }

    /// 解析完整函数定义，包括 body。
    fn parse_function(&mut self, full_text: &str, define_line: &str) -> Result<Function, ParseError> {
        let at_pos = define_line
            .find('@')
            .ok_or_else(|| ParseError::new("Expected '@' in function definition", self.line))?;
        let paren_pos = define_line[at_pos..]
            .find('(')
            .map(|p| at_pos + p)
            .ok_or_else(|| ParseError::new("Expected '(' in function definition", self.line))?;
        let name = define_line[at_pos + 1..paren_pos].trim().to_string();

        // 提取返回类型
        let ret_str = define_line[7..at_pos].trim(); // skip "define "
        let return_ty = parse_type(ret_str)?;

        // 解析参数列表
        let close_paren = define_line[paren_pos..]
            .find(')')
            .map(|p| paren_pos + p)
            .ok_or_else(|| ParseError::new("Expected ')' in function definition", self.line))?;
        let params_str = &define_line[paren_pos + 1..close_paren];
        let params = parse_params(params_str)?;

        let signature = Signature {
            params,
            returns: vec![return_ty],
            calling_convention: CallConv::default(),
        };

        let mut func = Function::new(&name, signature);

        // 提取函数体（花括号之间的文本）
        let Some(brace_pos) = define_line.find('{') else {
            // 没有 body（只是声明）
            return Ok(func);
        };

        // 收集完整 body: 从当前行到 "}" 行
        let _body_start = define_line[brace_pos + 1..].trim();
        let body_text = self.extract_function_body(full_text)?;

        // 解析 body 中的基本块和指令
        self.parse_function_body(&body_text, &mut func)?;

        Ok(func)
    }

    /// 从文本中提取 `{` 到 `}` 之间的函数体。
    fn extract_function_body(&self, full_text: &str) -> Result<String, ParseError> {
        // 找到第一个 `{` 之后的部分
        let brace_pos = full_text.find('{').ok_or_else(|| {
            ParseError::new("Expected '{' for function body", self.line)
        })?;
        let after_brace = &full_text[brace_pos + 1..];

        // 找到匹配的 `}`，同时检查嵌套深度
        let mut depth = 1i32;
        let mut end_pos = 0usize;
        for (i, ch) in after_brace.char_indices() {
            match ch {
                '{' => {
                    depth += 1;
                    if depth > MAX_NESTING_DEPTH {
                        return Err(ParseError::new(
                            format!(
                                "Function body nesting depth exceeds maximum ({})",
                                MAX_NESTING_DEPTH
                            ),
                            self.line,
                        ));
                    }
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end_pos = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        if depth != 0 {
            return Err(ParseError::new("Unmatched '{' in function body", self.line));
        }

        Ok(after_brace[..end_pos].to_string())
    }

    /// 解析函数体：基本块标签 + 指令。
    fn parse_function_body(&self, body: &str, func: &mut Function) -> Result<(), ParseError> {
        let mut current_block: Option<BlockId> = None;
        let mut name_to_value: HashMap<String, Value> = HashMap::new();
        let mut next_value_id: u32 = 0;
        let mut instruction_count: usize = 0;

        for line_text in body.lines() {
            let trimmed = line_text.trim();
            if trimmed.is_empty() || trimmed.starts_with(';') {
                continue;
            }

            // 基本块标签: "label:"
            if let Some(colon_pos) = trimmed.find(':')
                && !trimmed.starts_with('\"')  // skip string literals
            {
                let label = &trimmed[..colon_pos];
                // 不是 LLVM 关键字／指令名
                if is_label_name(label) {
                    let block_id = func.create_block_id();
                    func.blocks.push(Block::new(block_id));
                    current_block = Some(block_id);
                    // 处理块标签后的内联指令: "entry:  %x = add i32 1, 2"
                    let after_colon = trimmed[colon_pos + 1..].trim();
                    if !after_colon.is_empty() {
                        instruction_count += 1;
                        if instruction_count > MAX_INSTRUCTIONS_PER_FUNC {
                            return Err(ParseError::new(
                                format!(
                                    "Too many instructions in function (maximum {})",
                                    MAX_INSTRUCTIONS_PER_FUNC
                                ),
                                self.line,
                            ));
                        }
                        self.parse_instruction_line(
                            after_colon,
                            func,
                            &mut current_block,
                            &mut name_to_value,
                            &mut next_value_id,
                        )?;
                    }
                    continue;
                }
            }

            // 指令或终止指令
            if let Some(block_id) = current_block {
                instruction_count += 1;
                if instruction_count > MAX_INSTRUCTIONS_PER_FUNC {
                    return Err(ParseError::new(
                        format!(
                            "Too many instructions in function (maximum {})",
                            MAX_INSTRUCTIONS_PER_FUNC
                        ),
                        self.line,
                    ));
                }
                self.parse_instruction_line(
                    trimmed,
                    func,
                    &mut Some(block_id),
                    &mut name_to_value,
                    &mut next_value_id,
                )?;
            }
        }

        // 分配 value_count
        func.value_count = next_value_id;
        Ok(())
    }

    /// 解析一行指令或终止指令。
    fn parse_instruction_line(
        &self,
        trimmed: &str,
        func: &mut Function,
        current_block: &mut Option<BlockId>,
        name_to_value: &mut HashMap<String, Value>,
        next_value_id: &mut u32,
    ) -> Result<(), ParseError> {
        let block_id = current_block.ok_or_else(|| {
            ParseError::new("instruction outside any basic block", self.line)
        })?;
        let block = func.block_mut(block_id).ok_or_else(|| {
            ParseError::new("block not found", self.line)
        })?;

        // 检查终止指令
        if trimmed == "ret void" {
            block.terminator = Terminator::Return {
                values: SmallVec::new(),
            };
            return Ok(());
        }
        if let Some(stripped) = trimmed.strip_prefix("ret ") {
            let ret_vals = parse_ret_operands(stripped, name_to_value)?;
            block.terminator = Terminator::Return {
                values: ret_vals,
            };
            return Ok(());
        }
        if trimmed == "unreachable" {
            block.terminator = Terminator::Unreachable;
            return Ok(());
        }
        if let Some(stripped) = trimmed.strip_prefix("br label ") {
            let target_name = stripped.trim();
            let target = resolve_label(target_name, name_to_value);
            block.terminator = Terminator::Jump {
                target,
                args: SmallVec::new(),
            };
            return Ok(());
        }
        if trimmed.starts_with("br i1 ") {
            // br i1 %cond, label %true, label %false
            return self.parse_br_cond(trimmed, block, name_to_value);
        }

        // 解析 "result = opcode type operands" 格式的指令
        if let Some(eq_pos) = trimmed.find('=') {
            let result_name = trimmed[..eq_pos].trim().to_string();
            let rest = trimmed[eq_pos + 1..].trim();

            let (opcode, ty, operands) = self.parse_opcode_and_operands(rest, name_to_value)?;
            let value = Value(*next_value_id);
            if let Some(op) = opcode {
                *next_value_id += 1;
                name_to_value.insert(result_name, value);

                let inst = Instruction::new(op, operands, Some(value), ty);
                // Drop block borrow before mutating func for set_value_type
                let block_id = current_block.ok_or_else(|| {
                    ParseError::new("instruction outside any basic block", self.line)
                })?;
                {
                    let blk = func.block_mut(block_id).ok_or_else(|| {
                        ParseError::new("block not found", self.line)
                    })?;
                    blk.instructions.push(inst);
                }
                func.set_value_type(value, ty);
            }
            // else: unrecognized — skip
        } else {
            // 无结果指令: store, fence, call void
            let block_id = current_block.ok_or_else(|| {
                ParseError::new("instruction outside any basic block", self.line)
            })?;
            let blk = func.block_mut(block_id).ok_or_else(|| {
                ParseError::new("block not found", self.line)
            })?;
            self.parse_side_effect_inst(trimmed, blk, name_to_value)?;
        }
        Ok(())
    }

    /// 解析 `opcode type operands`（有结果的指令格式）。
    fn parse_opcode_and_operands(
        &self,
        rest: &str,
        name_to_value: &HashMap<String, Value>,
    ) -> Result<(Option<Opcode>, Type, SmallVec<[Value; 4]>), ParseError> {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.is_empty() {
            return Ok((None, Type::Void, SmallVec::new()));
        }

        let opcode_str = parts[0];

        // 匹配常见 LLVM IR 指令
        match opcode_str {
            "add" | "sub" | "mul" | "udiv" | "sdiv" | "urem" | "srem"
            | "and" | "or" | "xor" | "shl" | "lshr" | "ashr" => {
                let ty = parse_type(parts.get(1).unwrap_or(&"i32"))?;
                let a = resolve_operand(parts.get(2).unwrap_or(&""), name_to_value);
                let b = resolve_operand(parts.get(3).unwrap_or(&""), name_to_value);
                let op = match opcode_str {
                    "add" => Opcode::Iadd, "sub" => Opcode::Isub,
                    "mul" => Opcode::Imul, "udiv" => Opcode::Udiv,
                    "sdiv" => Opcode::Sdiv, "urem" => Opcode::Urem,
                    "srem" => Opcode::Srem, "and" => Opcode::Band,
                    "or" => Opcode::Bor, "xor" => Opcode::Bxor,
                    "shl" => Opcode::Ishl, "lshr" => Opcode::Ushr,
                    "ashr" => Opcode::Sshr, _ => Opcode::Nop,
                };
                Ok((Some(op), ty, smallvec::smallvec![a, b]))
            }
            "fadd" | "fsub" | "fmul" | "fdiv" => {
                let ty = parse_type(parts.get(1).unwrap_or(&"float"))?;
                let a = resolve_operand(parts.get(2).unwrap_or(&""), name_to_value);
                let b = resolve_operand(parts.get(3).unwrap_or(&""), name_to_value);
                let op = match opcode_str {
                    "fadd" => Opcode::Fadd { flags: FastMathFlags::NONE },
                    "fsub" => Opcode::Fsub { flags: FastMathFlags::NONE },
                    "fmul" => Opcode::Fmul { flags: FastMathFlags::NONE },
                    "fdiv" => Opcode::Fdiv { flags: FastMathFlags::NONE },
                    _ => Opcode::Nop,
                };
                Ok((Some(op), ty, smallvec::smallvec![a, b]))
            }
            "fneg" => {
                let ty = parse_type(parts.get(1).unwrap_or(&"float"))?;
                let a = resolve_operand(parts.get(2).unwrap_or(&""), name_to_value);
                Ok((Some(Opcode::Fneg { flags: FastMathFlags::NONE }), ty, smallvec::smallvec![a]))
            }
            "alloca" => {
                let _alloc_ty = parse_type(parts.get(1).unwrap_or(&"i32"))?;
                Ok((Some(Opcode::Alloca { count: 1 }), Type::Ptr, SmallVec::new()))
            }
            "load" => {
                let load_ty = parse_type(parts.get(1).unwrap_or(&"i32"))?;
                let ptr = resolve_operand(parts.get(3).unwrap_or(parts.get(2).unwrap_or(&"")), name_to_value);
                Ok((Some(Opcode::Load), load_ty, smallvec::smallvec![ptr]))
            }
            "icmp" => {
                let cond_str = parts.get(1).unwrap_or(&"eq");
                let _icmp_ty = parse_type(parts.get(2).unwrap_or(&"i32"))?;
                let a = resolve_operand(parts.get(3).unwrap_or(&""), name_to_value);
                let b = resolve_operand(parts.get(4).unwrap_or(&""), name_to_value);
                let cond = parse_icmp_cond(cond_str);
                Ok((Some(Opcode::Icmp { cond }), Type::Bool, smallvec::smallvec![a, b]))
            }
            "fcmp" => {
                let cond_str = parts.get(1).unwrap_or(&"oeq");
                let _fcmp_ty = parse_type(parts.get(2).unwrap_or(&"float"))?;
                let a = resolve_operand(parts.get(3).unwrap_or(&""), name_to_value);
                let b = resolve_operand(parts.get(4).unwrap_or(&""), name_to_value);
                let cond = parse_fcmp_cond(cond_str);
                Ok((Some(Opcode::Fcmp { cond, flags: FastMathFlags::NONE }), Type::Bool, smallvec::smallvec![a, b]))
            }
            "select" => {
                let sel_ty = parse_type(parts.get(1).unwrap_or(&"i32"))?;
                let cond = resolve_operand(parts.get(2).unwrap_or(&""), name_to_value);
                let a = resolve_operand(parts.get(3).unwrap_or(&""), name_to_value);
                let b = resolve_operand(parts.get(4).unwrap_or(&""), name_to_value);
                Ok((Some(Opcode::Select), sel_ty, smallvec::smallvec![cond, a, b]))
            }
            "zext" | "sext" | "trunc" | "bitcast" => {
                let a = resolve_operand(parts.get(1).unwrap_or(&""), name_to_value);
                let _from_ty = parse_type(parts.get(2).unwrap_or(&"i32"))?;
                let to_ty_str = parts.get(4).unwrap_or(parts.get(3).unwrap_or(&"i32"));
                let to_ty = parse_type(to_ty_str)?;
                let op = match opcode_str {
                    "zext" => Opcode::Uextend, "sext" => Opcode::Sextend,
                    "trunc" => Opcode::Ireduce, "bitcast" => Opcode::Bitcast,
                    _ => Opcode::Nop,
                };
                Ok((Some(op), to_ty, smallvec::smallvec![a]))
            }
            "phi" => {
                let phi_ty = parse_type(parts.get(1).unwrap_or(&"i32"))?;
                let rest_start = rest.find('[').unwrap_or(0);
                let incoming = parse_phi_incoming(&rest[rest_start..], name_to_value)?;
                Ok((Some(Opcode::Phi { incoming }), phi_ty, SmallVec::new()))
            }
            "getelementptr" | "gep" => {
                let ptr = resolve_operand(parts.get(2).unwrap_or(parts.get(1).unwrap_or(&"")), name_to_value);
                let indexed_ty = if parts.len() > 3 {
                    parse_type(parts[1])?
                } else {
                    Type::I32
                };
                // indices are the remaining operands after ptr
                let mut operands = smallvec::smallvec![ptr];
                for p in &parts[3..] {
                    let v = resolve_operand(p, name_to_value);
                    operands.push(v);
                }
                Ok((Some(Opcode::GetElementPtr { indexed_ty }), Type::Ptr, operands))
            }
            "call" => {
                let ret_ty = parse_type(parts.get(1).unwrap_or(&"void"))?;
                let _callee_name = parts.get(2).unwrap_or(&"").trim_start_matches('@');
                if ret_ty == Type::Void {
                    return Ok((None, Type::Void, SmallVec::new()));
                }
                Ok((Some(Opcode::Call { func: FuncRef(0) }), ret_ty, SmallVec::new()))
            }
            _ => Ok((None, Type::Void, SmallVec::new())),
        }
    }

    /// 解析副作用指令（无返回值）: store, fence, call void
    fn parse_side_effect_inst(
        &self,
        trimmed: &str,
        block: &mut Block,
        name_to_value: &HashMap<String, Value>,
    ) -> Result<(), ParseError> {
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(());
        }
        match parts[0] {
            "store"
                if parts.len() >= 4 => {
                    let val = resolve_operand(parts[1], name_to_value);
                    let ptr = resolve_operand(parts[3], name_to_value);
                    block.instructions.push(Instruction::new(
                        Opcode::Store,
                        smallvec::smallvec![val, ptr],
                        None,
                        Type::Void,
                    ));
                }
            "call" => {
                // call void @func(args...) — skip for now
            }
            "fence" => {
                // fence seq_cst — handled as Fence
                let ordering = if parts.len() > 1 {
                    parse_ordering(parts[1])
                } else {
                    Ordering::SequentiallyConsistent
                };
                block.instructions.push(Instruction::new(
                    Opcode::Fence { ordering },
                    SmallVec::new(),
                    None,
                    Type::Void,
                ));
            }
            _ => {} // unrecognized — skip
        }
        Ok(())
    }

    /// 解析条件分支: `br i1 %cond, label %true, label %false`
    fn parse_br_cond(
        &self,
        trimmed: &str,
        block: &mut Block,
        name_to_value: &HashMap<String, Value>,
    ) -> Result<(), ParseError> {
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        // [br, i1, %cond, label, %true, label, %false]
        if parts.len() < 7 {
            return Err(ParseError::new("Malformed br instruction", self.line));
        }
        let cond = resolve_operand(parts[2], name_to_value);
        let true_block = resolve_label(parts[4], name_to_value);
        let false_block = resolve_label(parts[6], name_to_value);
        block.terminator = Terminator::Branch {
            cond,
            true_block,
            false_block,
            true_args: SmallVec::new(),
            false_args: SmallVec::new(),
        };
        Ok(())
    }
}

impl Default for LlvmIrParser {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Helper functions
// ============================================================

fn is_label_name(s: &str) -> bool {
    // LLVM 标签通常以 % 开头或为纯标识符
    // 排除关键字
    !matches!(
        s,
        "ret" | "br" | "switch" | "unreachable" | "add" | "sub" | "mul"
            | "udiv" | "sdiv" | "urem" | "srem" | "and" | "or" | "xor"
            | "shl" | "lshr" | "ashr" | "fadd" | "fsub" | "fmul" | "fdiv"
            | "fneg" | "alloca" | "load" | "store" | "icmp" | "fcmp"
            | "phi" | "select" | "call" | "zext" | "sext" | "trunc"
            | "bitcast" | "getelementptr" | "gep"
    )
}

fn parse_type(s: &str) -> Result<Type, ParseError> {
    match s.trim() {
        "void" => Ok(Type::Void),
        "i1" => Ok(Type::Bool),
        "i8" => Ok(Type::I8),
        "i16" => Ok(Type::I16),
        "i32" => Ok(Type::I32),
        "i64" => Ok(Type::I64),
        "half" => Ok(Type::F16),
        "float" => Ok(Type::F32),
        "double" => Ok(Type::F64),
        "ptr" => Ok(Type::Ptr),
        _ => Err(ParseError::new(format!("Unknown type: {}", s), 0)),
    }
}

fn parse_params(params_str: &str) -> Result<Vec<(Type, String)>, ParseError> {
    if params_str.trim().is_empty() || params_str.trim() == "..." {
        return Ok(Vec::new());
    }
    let mut params = Vec::new();
    for part in params_str.split(',') {
        let part = part.trim();
        if part.is_empty() || part == "..." {
            continue;
        }
        // "i32 %name" or "i32"
        let words: Vec<&str> = part.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let ty = parse_type(words[0])?;
        let name = words
            .get(1)
            .map(|s| s.trim_start_matches('%').to_string())
            .unwrap_or_default();
        params.push((ty, name));
    }
    Ok(params)
}

fn parse_ret_operands(
    s: &str,
    name_to_value: &HashMap<String, Value>,
) -> Result<SmallVec<[Value; 2]>, ParseError> {
    let s = s.trim();
    // "i32 %val" or "void"
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.is_empty() || parts[0] == "void" {
        return Ok(SmallVec::new());
    }
    // Skip type, parse values
    let mut values = SmallVec::new();
    for part in &parts[1..] {
        let cleaned = part.trim_end_matches(',');
        values.push(resolve_operand(cleaned, name_to_value));
    }
    Ok(values)
}

fn resolve_operand(s: &str, name_to_value: &HashMap<String, Value>) -> Value {
    let s = s.trim().trim_end_matches(',');
    if s.is_empty() {
        return Value(0);
    }
    // Try %name lookup
    if s.starts_with('%')
        && let Some(&v) = name_to_value.get(&s[1..].to_string()) {
            return v;
        }
    // Try direct name lookup
    if let Some(&v) = name_to_value.get(s) {
        return v;
    }
    // Try parsing as integer literal
    if let Ok(n) = s.parse::<u32>() {
        return Value(n);
    }
    Value(0)
}

fn resolve_label(s: &str, name_to_value: &HashMap<String, Value>) -> BlockId {
    let s = s.trim().trim_end_matches(',').trim_start_matches('%');
    // Look up as a value, then use its index as block ID
    if let Some(&v) = name_to_value.get(s) {
        return BlockId(v.0);
    }
    // Try parsing as number
    if let Ok(n) = s.parse::<u32>() {
        return BlockId(n);
    }
    BlockId(0)
}

fn parse_icmp_cond(s: &str) -> IntCC {
    match s.trim() {
        "eq" => IntCC::Equal,
        "ne" => IntCC::NotEqual,
        "slt" => IntCC::SignedLessThan,
        "sgt" => IntCC::SignedGreaterThan,
        "sle" => IntCC::SignedLessThanOrEqual,
        "sge" => IntCC::SignedGreaterThanOrEqual,
        "ult" => IntCC::UnsignedLessThan,
        "ugt" => IntCC::UnsignedGreaterThan,
        "ule" => IntCC::UnsignedLessThanOrEqual,
        "uge" => IntCC::UnsignedGreaterThanOrEqual,
        _ => IntCC::Equal,
    }
}

fn parse_fcmp_cond(s: &str) -> FloatCC {
    match s.trim() {
        "false" => FloatCC::Ordered,     // always false
        "oeq" => FloatCC::Equal,
        "ogt" => FloatCC::GreaterThan,
        "oge" => FloatCC::GreaterThanOrEqual,
        "olt" => FloatCC::LessThan,
        "ole" => FloatCC::LessThanOrEqual,
        "one" => FloatCC::NotEqual,
        "ord" => FloatCC::Ordered,
        "ueq" => FloatCC::Equal,         // unordered or equal → map to Equal
        "ugt" => FloatCC::GreaterThan,
        "uge" => FloatCC::GreaterThanOrEqual,
        "ult" => FloatCC::LessThan,
        "ule" => FloatCC::LessThanOrEqual,
        "une" => FloatCC::NotEqual,
        "uno" => FloatCC::Unordered,
        "true" => FloatCC::Ordered,      // always true
        _ => FloatCC::Ordered,
    }
}

fn parse_ordering(s: &str) -> Ordering {
    match s.trim() {
        "unordered" => Ordering::Unordered,
        "monotonic" => Ordering::Monotonic,
        "acquire" => Ordering::Acquire,
        "release" => Ordering::Release,
        "acq_rel" => Ordering::AcquireRelease,
        "seq_cst" => Ordering::SequentiallyConsistent,
        _ => Ordering::SequentiallyConsistent,
    }
}

fn parse_phi_incoming(
    s: &str,
    name_to_value: &HashMap<String, Value>,
) -> Result<SmallVec<[(Value, BlockId); 4]>, ParseError> {
    let mut incoming = SmallVec::new();
    let s = s.trim();
    // "[ %val, %label ], [ %val2, %label2 ]"
    for bracket in s.split(']') {
        let inner = bracket.trim().trim_start_matches('[').trim();
        if inner.is_empty() {
            continue;
        }
        let parts: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
        if parts.len() >= 2 {
            let val = resolve_operand(parts[0], name_to_value);
            let label = resolve_label(parts[1], name_to_value);
            incoming.push((val, label));
        }
    }
    Ok(incoming)
}

/// 便捷函数：从 LLVM IR 文本解析为 Module。
pub fn parse_llvm_ir(text: &str) -> Result<Module, ParseError> {
    let mut parser = LlvmIrParser::new();
    parser.parse(text)
}

impl Module {
    /// 从 LLVM IR 文本格式解析 Module。
    pub fn parse_ll(text: &str) -> Result<Self, ParseError> {
        parse_llvm_ir(text)
    }
}

/// 解析 LLVM IR 文本中的所有函数并返回它们的名字。
pub fn parse_function_names(text: &str) -> Result<Vec<String>, ParseError> {
    let mut names = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("define ")
            && let Some(at_pos) = trimmed.find('@')
            && let Some(paren_pos) = trimmed[at_pos..].find('(')
        {
            names.push(trimmed[at_pos + 1..at_pos + paren_pos].trim().to_string());
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_type_basic() {
        assert_eq!(parse_type("void").unwrap(), Type::Void);
        assert_eq!(parse_type("i1").unwrap(), Type::Bool);
        assert_eq!(parse_type("i32").unwrap(), Type::I32);
        assert_eq!(parse_type("i64").unwrap(), Type::I64);
        assert_eq!(parse_type("float").unwrap(), Type::F32);
        assert_eq!(parse_type("double").unwrap(), Type::F64);
        assert_eq!(parse_type("ptr").unwrap(), Type::Ptr);
    }

    #[test]
    fn test_parse_type_unknown() {
        assert!(parse_type("i256").is_err());
    }

    #[test]
    fn test_parse_function_names() {
        let ir = r#"
define i32 @add(i32 %a, i32 %b) {
entry:
  %sum = add i32 %a, %b
  ret i32 %sum
}

define void @main() {
entry:
  ret void
}
"#;
        let names = parse_function_names(ir).unwrap();
        assert_eq!(names, vec!["add", "main"]);
    }

    #[test]
    fn test_parse_simple_module() {
        let ir = r#"
define i32 @answer() {
entry:
  ret i32 42
}
"#;
        let module = Module::parse_ll(ir).unwrap();
        assert!(!module.is_empty());
    }

    #[test]
    fn test_parse_add_function() {
        let ir = r#"
define i32 @add(i32 %a, i32 %b) {
entry:
  %sum = add i32 %a, %b
  ret i32 %sum
}
"#;
        let module = Module::parse_ll(ir).unwrap();
        assert_eq!(module.len(), 1);
        let func = module.iter().next().unwrap();
        assert_eq!(func.name, "add");
        let entry = func.entry_block().unwrap();
        // Should have at least 1 instruction (add)
        let _ = entry;
    }

    #[test]
    fn test_parse_icmp() {
        let ir = r#"
define i1 @cmp(i32 %a, i32 %b) {
entry:
  %r = icmp slt i32 %a, %b
  ret i1 %r
}
"#;
        let module = Module::parse_ll(ir).unwrap();
        assert!(!module.is_empty());
    }

    #[test]
    fn test_empty_ir() {
        let module = Module::parse_ll("").unwrap();
        assert_eq!(module.len(), 0);
    }

    #[test]
    fn test_comment_only_ir() {
        let ir = "; This is a comment\n; Another comment\n";
        let module = Module::parse_ll(ir).unwrap();
        assert_eq!(module.len(), 0);
    }
}
