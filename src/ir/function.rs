//! 函数和基本块表示。

use super::constant_pool::ConstantPool;
use super::debug_info::DebugInfo;
use super::instructions::*;
use super::types::*;
use std::collections::HashSet;
use std::fmt;

/// 函数级属性位掩码。
///
/// 用于控制优化 pass 的行为（如内联策略）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FunctionAttributes(u8);

impl FunctionAttributes {
    pub const NONE: Self = Self(0);
    /// `#[inline(always)]` — 无论成本如何，始终内联。
    pub const INLINE_ALWAYS: Self = Self(1 << 0);
    /// `#[inline(never)]` — 永不内联。
    pub const INLINE_NEVER: Self = Self(1 << 1);
    /// 函数标记为 `const`（编译时可求值）。
    pub const CONST: Self = Self(1 << 2);
    /// 函数无副作用（纯函数）。
    pub const PURE: Self = Self(1 << 3);

    pub fn contains(self, attr: Self) -> bool {
        (self.0 & attr.0) != 0
    }

    pub fn set(&mut self, attr: Self) {
        self.0 |= attr.0;
    }

    pub fn remove(&mut self, attr: Self) {
        self.0 &= !attr.0;
    }
}

/// 基本块。
#[derive(Clone, Debug)]
pub struct Block {
    pub id: BlockId,
    /// 块参数（用于 SSA phi 节点和入口参数）。
    pub params: Vec<(Value, Type)>,
    /// 块内非终止指令。
    pub instructions: Vec<Instruction>,
    /// 终止指令。
    pub terminator: Terminator,
}

impl Block {
    pub fn new(id: BlockId) -> Self {
        Self {
            id,
            params: Vec::new(),
            instructions: Vec::new(),
            terminator: Terminator::Unreachable,
        }
    }

    pub fn with_params(id: BlockId, params: Vec<(Value, Type)>) -> Self {
        Self {
            id,
            params,
            instructions: Vec::new(),
            terminator: Terminator::Unreachable,
        }
    }
}

/// IR 函数。
#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub signature: Signature,
    pub blocks: Vec<Block>,
    pub value_count: u32,
    pub block_count: u32,
    /// 常量池 — 存储此函数使用的所有编译时常量。
    pub constant_pool: ConstantPool,
    /// 标记此函数是否可以在编译时求值（const fn）。
    ///
    /// const 函数不能有副作用（Store、调用非 const 函数等），
    /// 当所有实参是编译时常量时，可在 lowering 阶段被 `IrInterpreter` 求值。
    pub is_const: bool,
    /// 函数级属性位掩码（inline 策略等）。
    pub attributes: FunctionAttributes,
    /// 调试信息 — 源码位置元数据（可选）。
    pub debug_info: Option<DebugInfo>,
    /// 值到类型的映射（按 Value ID 索引）。
    /// 用于自动类型推断和消除 builder 中冗余的 ty 参数。
    /// 索引与 `value_count` 同步增长。
    pub(crate) value_types: Vec<Type>,
    /// Def-Use 链：每个 Value 的所有使用者列表（按 Value ID 索引）。
    /// 用于优化 passes 快速查询哪些指令使用某个值。
    /// 索引与 `value_count` 同步增长。
    use_lists: Vec<Vec<Value>>,
}

impl Function {
    pub fn new(name: &str, signature: Signature) -> Self {
        Self {
            name: name.to_string(),
            signature,
            blocks: Vec::new(),
            value_count: 0,
            block_count: 0,
            constant_pool: ConstantPool::new(),
            is_const: false,
            attributes: FunctionAttributes::NONE,
            debug_info: None,
            value_types: Vec::new(),
            use_lists: Vec::new(),
        }
    }

    /// 分配一个新的 SSA Value。
    pub fn create_value(&mut self) -> Value {
        let v = Value(self.value_count);
        self.value_count += 1;
        v
    }

    /// 分配一个新的 SSA Value 并记录其类型。
    ///
    /// 推荐的创建方式 — 跟踪类型信息以供后续查询和自动推断。
    pub fn create_typed_value(&mut self, ty: Type) -> Value {
        let v = Value(self.value_count);
        self.value_count += 1;
        self.value_types.push(ty);
        v
    }

    /// 获取指定 Value 的类型（如果存在类型记录）。
    ///
    /// 只有在使用 `create_typed_value` 创建的值才能查到类型。
    /// 旧代码使用 `create_value()` 创建的值返回 None。
    pub fn value_type(&self, value: Value) -> Option<Type> {
        let idx = value.0 as usize;
        if idx < self.value_types.len() {
            Some(self.value_types[idx])
        } else {
            None
        }
    }

    /// 记录或更新一个 Value 的类型。
    ///
    /// 自动扩展 `value_types` 向量以容纳此 Value ID。
    /// 如果该值已有类型记录，则覆盖。
    pub fn set_value_type(&mut self, value: Value, ty: Type) {
        let idx = value.0 as usize;
        // 确保向量足够大
        if idx >= self.value_types.len() {
            self.value_types.resize(idx + 1, Type::Void);
        }
        self.value_types[idx] = ty;
    }

    // ============================================================
    // Def-Use 链
    // ============================================================

    /// 记录指令 `user` 使用了值 `val`。
    /// 如果 use_lists 向量不够大，自动扩展。
    pub fn add_use(&mut self, val: Value, user: Value) {
        let idx = val.0 as usize;
        if idx >= self.use_lists.len() {
            self.use_lists.resize(idx + 1, Vec::new());
        }
        self.use_lists[idx].push(user);
    }

    /// 返回使用某个值的所有使用者列表。
    pub fn uses(&self, val: Value) -> &[Value] {
        let idx = val.0 as usize;
        if idx < self.use_lists.len() {
            &self.use_lists[idx]
        } else {
            &[]
        }
    }

    /// 返回使用某个值的可变使用者列表。
    pub fn uses_mut(&mut self, val: Value) -> &mut Vec<Value> {
        let idx = val.0 as usize;
        if idx >= self.use_lists.len() {
            self.use_lists.resize(idx + 1, Vec::new());
        }
        &mut self.use_lists[idx]
    }

    /// 清空所有 def-use 信息（当指令被大量修改时使用）。
    pub fn clear_uses(&mut self) {
        for list in &mut self.use_lists {
            list.clear();
        }
    }

    /// 检查一个值是否有任何使用者。
    pub fn has_uses(&self, val: Value) -> bool {
        !self.uses(val).is_empty()
    }

    /// 从某个值的使用者列表中移除特定使用者。
    /// 返回是否成功移除。
    pub fn remove_use(&mut self, val: Value, user: Value) -> bool {
        let idx = val.0 as usize;
        if idx < self.use_lists.len()
            && let Some(pos) = self.use_lists[idx].iter().position(|&u| u == user)
        {
            self.use_lists[idx].swap_remove(pos);
            return true;
        }
        false
    }

    /// 从所有值的使用者列表中移除对 `dead_value` 的引用。
    /// 当删除一条指令时调用，清理所有对该指令结果值的引用。
    pub fn remove_all_uses_of(&mut self, dead_value: Value) {
        // 从 dead_value 使用者的 use_lists 中移除 dead_value 作为使用者
        let users: Vec<Value> = self.uses(dead_value).to_vec();
        for user_val in &users {
            self.remove_use(*user_val, dead_value);
        }
        // 清空 dead_value 的 use_list
        let idx = dead_value.0 as usize;
        if idx < self.use_lists.len() {
            self.use_lists[idx].clear();
        }
    }

    /// 重建所有指令的 def-use 链。
    /// 当 IR 被手动修改后调用，确保 use_lists 与当前指令一致。
    pub fn rebuild_use_lists(&mut self) {
        // 清空所有 use_lists
        for list in &mut self.use_lists {
            list.clear();
        }

        // 先收集所有的 (operand, user) 对
        let mut pairs: Vec<(Value, Value)> = Vec::new();
        for block in &self.blocks {
            for inst in &block.instructions {
                if let Some(result) = inst.result {
                    for &op in &inst.operands {
                        pairs.push((op, result));
                    }
                }
            }
        }
        for (op, user) in pairs {
            self.add_use(op, user);
        }
    }

    /// 分配一个新的基本块 ID（不创建块体，由 Builder 负责）。
    pub fn create_block_id(&mut self) -> BlockId {
        let id = BlockId(self.block_count);
        self.block_count += 1;
        id
    }

    /// 按 ID 获取块的不可变引用。
    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.blocks.get(id.0 as usize)
    }

    /// 按 ID 获取块的可变引用。
    pub fn block_mut(&mut self, id: BlockId) -> Option<&mut Block> {
        self.blocks.get_mut(id.0 as usize)
    }

    /// 迭代所有基本块。
    pub fn iter_blocks(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter()
    }

    /// 获取入口块。
    pub fn entry_block(&self) -> Option<&Block> {
        self.blocks.first()
    }
}

impl Default for Function {
    fn default() -> Self {
        Self {
            name: String::new(),
            signature: Signature::void(),
            blocks: Vec::new(),
            value_count: 0,
            block_count: 0,
            constant_pool: ConstantPool::new(),
            is_const: false,
            attributes: FunctionAttributes::NONE,
            debug_info: None,
            value_types: Vec::new(),
            use_lists: Vec::new(),
        }
    }
}

// ============================================================
// Display — IR 可读输出
// ============================================================

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "block {}(", self.id)?;
        for (i, (v, ty)) in self.params.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}: {}", v, ty)?;
        }
        writeln!(f, "):")?;

        for inst in &self.instructions {
            write!(f, "    ")?;
            if let Some(r) = inst.result {
                write!(f, "{} = ", r)?;
            }
            fmt_inst(f, inst)?;
            // 附加源码位置
            if let Some(ref loc) = inst.source_location {
                write!(f, "  ; {}", loc)?;
            }
            writeln!(f)?;
        }

        write!(f, "    ")?;
        fmt_terminator(f, &self.terminator)?;
        writeln!(f)
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}",
            if self.is_const { "const fn" } else { "fn" },
            self.name
        )?;
        write!(f, "(")?;
        for (i, (ty, name)) in self.signature.params.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}: {}", name, ty)?;
        }
        write!(f, ")")?;
        if !self.signature.returns.is_empty() {
            write!(f, " -> ")?;
            for (i, ty) in self.signature.returns.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", ty)?;
            }
        }
        writeln!(f, " {{")?;

        for block in &self.blocks {
            for line in format!("{}", block).lines() {
                writeln!(f, "  {}", line)?;
            }
        }

        writeln!(f, "}}")
    }
}

fn fmt_inst(f: &mut fmt::Formatter<'_>, inst: &Instruction) -> fmt::Result {
    match &inst.opcode {
        Opcode::Iadd => write!(f, "iadd {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Isub => write!(f, "isub {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Imul => write!(f, "imul {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Udiv => write!(f, "udiv {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Sdiv => write!(f, "sdiv {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Urem => write!(f, "urem {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Srem => write!(f, "srem {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Fadd { .. } => write!(f, "fadd {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Fsub { .. } => write!(f, "fsub {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Fmul { .. } => write!(f, "fmul {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Fdiv { .. } => write!(f, "fdiv {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Freeze => write!(f, "freeze {}", inst.operands[0]),
        Opcode::Fneg { .. } => write!(f, "fneg {}", inst.operands[0]),
        Opcode::Fabs { .. } => write!(f, "fabs {}", inst.operands[0]),
        Opcode::Fsqrt { .. } => write!(f, "fsqrt {}", inst.operands[0]),
        Opcode::Band => write!(f, "and {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Bor => write!(f, "or {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Bxor => write!(f, "xor {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Bnot => write!(f, "not {}", inst.operands[0]),
        Opcode::Ishl => write!(f, "shl {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Ushr => write!(f, "ushr {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Sshr => write!(f, "sshr {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Icmp { cond } => write!(
            f,
            "icmp.{:?} {}, {}",
            cond, inst.operands[0], inst.operands[1]
        ),
        Opcode::Fcmp { cond, .. } => write!(
            f,
            "fcmp.{:?} {}, {}",
            cond, inst.operands[0], inst.operands[1]
        ),
        Opcode::Load => write!(f, "load.{} {}", inst.ty, inst.operands[0]),
        Opcode::Store => write!(f, "store {} -> {}", inst.operands[0], inst.operands[1]),
        Opcode::StackLoad { offset } => write!(f, "stack_load [fp{}]", offset),
        Opcode::StackStore { offset } => {
            write!(f, "stack_store {} -> [fp{}]", inst.operands[0], offset)
        }
        Opcode::Iconst { index } => write!(f, "iconst.{} @{}", inst.ty, index),
        Opcode::Fconst { index } => write!(f, "fconst.{} @{}", inst.ty, index),
        Opcode::Sextend => write!(f, "sextend.{} {}", inst.ty, inst.operands[0]),
        Opcode::Uextend => write!(f, "uextend.{} {}", inst.ty, inst.operands[0]),
        Opcode::Ireduce => write!(f, "ireduce.{} {}", inst.ty, inst.operands[0]),
        Opcode::Bitcast => write!(f, "bitcast.{} {}", inst.ty, inst.operands[0]),
        Opcode::Call { func } => {
            write!(f, "call @{}(", func)?;
            for (i, op) in inst.operands.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", op)?;
            }
            write!(f, ")")
        }
        Opcode::CallIndirect => write!(f, "call_indirect {}", inst.operands[0]),
        Opcode::StackAddr { offset } => write!(f, "stack_addr [fp{}]", offset),
        Opcode::GlobalAddr { global } => write!(f, "global_addr @{}", global),
        Opcode::Copy => write!(f, "copy {}", inst.operands[0]),
        Opcode::Phi { incoming } => {
            write!(f, "phi ")?;
            for (i, (val, block)) in incoming.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "[{}, {}]", val, block)?;
            }
            Ok(())
        }
        Opcode::Select => write!(
            f,
            "select {}, {}, {}",
            inst.operands[0], inst.operands[1], inst.operands[2]
        ),
        Opcode::Vadd => write!(f, "vadd {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Vsub => write!(f, "vsub {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Vmul => write!(f, "vmul {}, {}", inst.operands[0], inst.operands[1]),
        Opcode::Vextract { lane } => write!(
            f,
            "vextract.{} {}, {}",
            lane, inst.operands[0], inst.operands[1]
        ),
        Opcode::Vinsert { lane } => write!(
            f,
            "vinsert.{} {}, {}",
            lane, inst.operands[0], inst.operands[1]
        ),
        Opcode::ShuffleVector { mask } => {
            write!(f, "shufflevector {}, {}, [", inst.operands[0], inst.operands[1])?;
            for (i, &m) in mask.iter().enumerate() {
                if i > 0 { write!(f, ", ")?; }
                write!(f, "{:x}", m)?;
            }
            write!(f, "]")
        }
        Opcode::AtomicRmw { op, ordering } => {
            write!(f, "atomicrmw {:?} {:?}, {}, {}", op, ordering, inst.operands[0], inst.operands[1])
        }
        Opcode::Cmpxchg { ordering } => {
            write!(f, "cmpxchg {:?}, {}, {}, {}", ordering, inst.operands[0], inst.operands[1], inst.operands[2])
        }
        Opcode::Fence { ordering } => write!(f, "fence {:?}", ordering),
        Opcode::ExtractValue { index } => write!(f, "extractvalue {}, [{}]", inst.operands[0], index),
        Opcode::InsertValue { index } => write!(f, "insertvalue {}, {}, [{}]", inst.operands[0], inst.operands[1], index),
        Opcode::Nop => write!(f, "nop"),
        Opcode::Alloca { count } => write!(f, "alloca {} [{}]", inst.ty, count),
        Opcode::GetElementPtr { indexed_ty } => {
            write!(f, "gep {} [", indexed_ty)?;
            for (i, idx) in inst.operands.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", idx)?;
            }
            write!(f, "]")
        }
    }
}

fn fmt_terminator(f: &mut fmt::Formatter<'_>, term: &Terminator) -> fmt::Result {
    match term {
        Terminator::Branch {
            cond,
            true_block,
            false_block,
            true_args,
            false_args,
        } => {
            write!(f, "br {}, {}, {}", cond, true_block, false_block)?;
            if !true_args.is_empty() || !false_args.is_empty() {
                write!(f, " [")?;
                for v in true_args {
                    write!(f, "{} ", v)?;
                }
                write!(f, "| ")?;
                for v in false_args {
                    write!(f, "{} ", v)?;
                }
                write!(f, "]")?;
            }
            Ok(())
        }
        Terminator::Jump { target, args } => {
            write!(f, "jmp {}", target)?;
            if !args.is_empty() {
                write!(f, " [")?;
                for v in args {
                    write!(f, "{} ", v)?;
                }
                write!(f, "]")?;
            }
            Ok(())
        }
        Terminator::Return { values } => {
            write!(f, "ret")?;
            if !values.is_empty() {
                write!(f, " ")?;
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", v)?;
                }
            }
            Ok(())
        }
        Terminator::Switch {
            discriminant,
            default_block,
            cases,
        } => {
            write!(f, "switch {} default={} [", discriminant, default_block)?;
            for (val, target, _) in cases.iter() {
                write!(f, " {}->{},", val, target)?;
            }
            write!(f, " ]")
        }
        Terminator::Unreachable => write!(f, "unreachable"),
    }
}

// ============================================================
// IR 验证
// ============================================================

/// IR 验证结果。
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationResult {
    pub fn new() -> Self {
        Self {
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn error(&mut self, msg: String) {
        self.errors.push(msg);
    }

    pub fn warn(&mut self, msg: String) {
        self.warnings.push(msg);
    }
}

impl Default for ValidationResult {
    fn default() -> Self {
        Self::new()
    }
}

impl Function {
    /// 验证 IR 的正确性。
    ///
    /// 检查：
    /// - 入口块存在
    /// - 所有使用的 Value 都已定义
    /// - 所有引用的块都存在
    /// - Phi 节点有操作数
    /// - 终止指令格式正确
    pub fn validate(&self) -> ValidationResult {
        let mut r = ValidationResult::new();

        // 1. 入口块必须存在
        if self.blocks.is_empty() {
            r.error("function has no blocks".into());
            return r;
        }

        // 2. 收集所有定义的 Value
        let mut defined: HashSet<Value> = HashSet::new();
        for block in &self.blocks {
            for (v, _) in &block.params {
                defined.insert(*v);
            }
            for inst in &block.instructions {
                if let Some(v) = inst.result {
                    defined.insert(v);
                }
            }
        }

        // 3. 检查所有使用的 Value 都已定义
        for block in &self.blocks {
            for inst in &block.instructions {
                for operand in &inst.operands {
                    if !defined.contains(operand) {
                        r.error(format!(
                            "block {}: instruction uses undefined value {}",
                            block.id, operand
                        ));
                    }
                }
            }
            // 检查终止指令中的值
            match &block.terminator {
                Terminator::Branch {
                    cond,
                    true_args,
                    false_args,
                    ..
                } => {
                    if !defined.contains(cond) {
                        r.error(format!(
                            "block {}: branch condition {} is undefined",
                            block.id, cond
                        ));
                    }
                    for v in true_args.iter().chain(false_args.iter()) {
                        if !defined.contains(v) {
                            r.error(format!("block {}: branch arg {} is undefined", block.id, v));
                        }
                    }
                }
                Terminator::Jump { args, .. } => {
                    for v in args {
                        if !defined.contains(v) {
                            r.error(format!("block {}: jump arg {} is undefined", block.id, v));
                        }
                    }
                }
                Terminator::Return { values } => {
                    for v in values {
                        if !defined.contains(v) {
                            r.error(format!(
                                "block {}: return value {} is undefined",
                                block.id, v
                            ));
                        }
                    }
                }
                Terminator::Switch {
                    discriminant,
                    cases,
                    ..
                } => {
                    if !defined.contains(discriminant) {
                        r.error(format!(
                            "block {}: switch discriminant {} is undefined",
                            block.id, discriminant
                        ));
                    }
                    for (_, _, args) in cases {
                        for v in args {
                            if !defined.contains(v) {
                                r.error(format!(
                                    "block {}: switch arg {} is undefined",
                                    block.id, v
                                ));
                            }
                        }
                    }
                }
                Terminator::Unreachable => {}
            }
        }

        // 4. 检查所有引用的块都存在
        let num_blocks = self.blocks.len() as u32;
        for block in &self.blocks {
            match &block.terminator {
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => {
                    if true_block.0 >= num_blocks {
                        r.error(format!(
                            "block {}: true target {} does not exist",
                            block.id, true_block
                        ));
                    }
                    if false_block.0 >= num_blocks {
                        r.error(format!(
                            "block {}: false target {} does not exist",
                            block.id, false_block
                        ));
                    }
                }
                Terminator::Jump { target, .. } => {
                    if target.0 >= num_blocks {
                        r.error(format!(
                            "block {}: jump target {} does not exist",
                            block.id, target
                        ));
                    }
                }
                Terminator::Switch {
                    default_block,
                    cases,
                    ..
                } => {
                    if default_block.0 >= num_blocks {
                        r.error(format!(
                            "block {}: switch default {} does not exist",
                            block.id, default_block
                        ));
                    }
                    for (_, target, _) in cases {
                        if target.0 >= num_blocks {
                            r.error(format!(
                                "block {}: switch target {} does not exist",
                                block.id, target
                            ));
                        }
                    }
                }
                Terminator::Return { .. } | Terminator::Unreachable => {}
            }
        }

        // 5. Phi 验证
        let preds = self.predecessors();
        for block in &self.blocks {
            for inst in &block.instructions {
                if matches!(inst.opcode, Opcode::Phi { .. }) {
                    if inst.operands.is_empty() {
                        r.error(format!("block {}: phi node has no operands", block.id));
                    }
                    if inst.result.is_none() {
                        r.error(format!("block {}: phi node has no result", block.id));
                    }
                    // Phi 操作数数量必须等于前驱数量
                    let pred_count = preds[block.id.0 as usize].len();
                    if inst.operands.len() != pred_count {
                        r.error(format!(
                            "block {}: phi has {} operands but {} predecessors",
                            block.id,
                            inst.operands.len(),
                            pred_count
                        ));
                    }
                }
            }
        }

        // 5.1 块参数验证：前驱的 Jump/Branch args 必须匹配目标块的 param 数量
        for block in &self.blocks {
            let targets = match &block.terminator {
                Terminator::Branch {
                    true_block,
                    false_block,
                    true_args,
                    false_args,
                    ..
                } => {
                    vec![
                        (*true_block, true_args.clone()),
                        (*false_block, false_args.clone()),
                    ]
                }
                Terminator::Jump { target, args } => vec![(*target, args.clone())],
                Terminator::Switch {
                    cases,
                    default_block,
                    ..
                } => {
                    let v: Vec<(BlockId, smallvec::SmallVec<[Value; 2]>)> =
                        cases.iter().map(|(_, b, a)| (*b, a.clone())).collect();
                    // Switch 的 default_block 不编码参数，仅检查 case 目标
                    // 对 default_block 发出警告而非错误
                    let ti = default_block.0 as usize;
                    if ti < self.blocks.len() && !self.blocks[ti].params.is_empty() {
                        r.warn(format!(
                            "block {}: switch default target {} expects {} params but none provided",
                            block.id, default_block, self.blocks[ti].params.len()
                        ));
                    }
                    v
                }
                _ => continue,
            };
            for (target, args) in targets {
                let ti = target.0 as usize;
                if ti < self.blocks.len() {
                    let target_params = self.blocks[ti].params.len();
                    if args.len() != target_params {
                        r.error(format!(
                            "block {}: jump to {} provides {} args but target expects {} params",
                            block.id,
                            target,
                            args.len(),
                            target_params
                        ));
                    }
                }
            }
        }

        // 5.5. 多返回块验证：所有 Return 的返回值类型必须匹配函数签名
        let expected_rets = &self.signature.returns;
        let return_blocks: Vec<_> = self
            .blocks
            .iter()
            .filter(|b| matches!(b.terminator, Terminator::Return { .. }))
            .collect();
        if !expected_rets.is_empty() && return_blocks.is_empty() {
            r.error("function has return type but no return blocks".into());
        }
        for ret_block in &return_blocks {
            if let Terminator::Return { values } = &ret_block.terminator
                && values.len() != expected_rets.len()
            {
                r.error(format!(
                    "block {}: return has {} values, expected {}",
                    ret_block.id,
                    values.len(),
                    expected_rets.len()
                ));
            }
        }

        // 6. 警告：死代码（有指令但 terminator 是 Unreachable 的非入口块）
        for (i, block) in self.blocks.iter().enumerate() {
            if i > 0
                && matches!(block.terminator, Terminator::Unreachable)
                && !block.instructions.is_empty()
            {
                r.warn(format!(
                    "block {}: has instructions but is unreachable",
                    block.id
                ));
            }
        }

        r
    }

    /// 收集所有前驱块的信息。
    /// 返回 `Vec<Vec<BlockId>>`，索引为 BlockId.0。
    pub fn predecessors(&self) -> Vec<Vec<BlockId>> {
        let mut preds: Vec<Vec<BlockId>> = vec![Vec::new(); self.blocks.len()];

        for block in &self.blocks {
            match &block.terminator {
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => {
                    preds[true_block.0 as usize].push(block.id);
                    preds[false_block.0 as usize].push(block.id);
                }
                Terminator::Jump { target, .. } => {
                    preds[target.0 as usize].push(block.id);
                }
                Terminator::Switch {
                    default_block,
                    cases,
                    ..
                } => {
                    preds[default_block.0 as usize].push(block.id);
                    for (_, target, _) in cases {
                        preds[target.0 as usize].push(block.id);
                    }
                }
                Terminator::Return { .. } | Terminator::Unreachable => {}
            }
        }

        preds
    }

    /// 计算支配树。
    ///
    /// 返回 `Vec<Vec<BlockId>>`，索引为 BlockId.0，值为该块直接支配的子块列表。
    /// 使用迭代数据流分析算法。
    #[deprecated(
        since = "0.2.0",
        note = "Use `crate::ir::DominatorTree::build(func)` for a formalized dominator tree with dominates/strictly_dominates/idom/dominance_frontier methods"
    )]
    pub fn dominator_tree(&self) -> Vec<Vec<BlockId>> {
        let n = self.blocks.len();
        if n == 0 {
            return Vec::new();
        }

        let preds = self.predecessors();

        // doms[b] = 所有块支配 b 的集合
        // 初始：entry 只有 entry 支配自己，其他块被所有块支配
        let all_blocks: HashSet<BlockId> = (0..n as u32).map(BlockId).collect();
        let mut doms: Vec<HashSet<BlockId>> = vec![all_blocks; n];
        doms[0] = {
            let mut s = HashSet::new();
            s.insert(BlockId(0));
            s
        };

        let mut changed = true;
        while changed {
            changed = false;
            for b in 1..n {
                // doms[b] = {b} ∪ ∩{doms[p] for p in preds[b]}
                let mut new_dom: HashSet<BlockId> = if preds[b].is_empty() {
                    HashSet::new()
                } else {
                    let mut iter = preds[b].iter();
                    let first = *iter.next().unwrap();
                    let mut intersection = doms[first.0 as usize].clone();
                    for p in iter {
                        intersection = intersection
                            .intersection(&doms[p.0 as usize])
                            .copied()
                            .collect();
                    }
                    intersection
                };
                new_dom.insert(BlockId(b as u32));

                if new_dom != doms[b] {
                    doms[b] = new_dom;
                    changed = true;
                }
            }
        }

        // 转换为支配树：dom_tree[b] = b 直接支配的块
        let mut dom_tree: Vec<Vec<BlockId>> = vec![Vec::new(); n];
        for b in 1..n {
            // 找到直接支配者：dom 中最接近的除了自己的块
            let mut idom = BlockId(0); // default to entry
            let mut min_size = usize::MAX;
            for &d in &doms[b] {
                if d.0 as usize != b && doms[d.0 as usize].len() < min_size {
                    // 更严格的支配者 = 更接近的直接支配者
                    if doms[b].contains(&d) {
                        idom = d;
                        min_size = doms[d.0 as usize].len();
                    }
                }
            }
            dom_tree[idom.0 as usize].push(BlockId(b as u32));
        }

        dom_tree
    }

    /// 检测循环。
    ///
    /// 返回后边列表 `Vec<(BlockId, BlockId)>`，每项为 `(循环头, 循环尾)`。
    /// 后边的定义：tail → head 的边，其中 head dominates tail。
    #[deprecated(
        since = "0.2.0",
        note = "Use `LoopForest::build(func, &DominatorTree::build(func))` for formalized loop analysis with nesting, depth, and exit block info"
    )]
    #[allow(deprecated)]
    pub fn detect_loops(&self) -> Vec<(BlockId, BlockId)> {
        let n = self.blocks.len();
        if n == 0 {
            return Vec::new();
        }

        let doms = self.dominator_tree_raw();
        let mut loops = Vec::new();

        for block in &self.blocks {
            let head = block.id;
            match &block.terminator {
                Terminator::Branch {
                    true_block,
                    false_block,
                    ..
                } => {
                    // CFG 边: head → target
                    for &target in &[*true_block, *false_block] {
                        // 检查 head 是否被 target 支配（即 target dom head）
                        if doms[head.0 as usize].contains(&target) {
                            loops.push((target, head));
                        }
                    }
                }
                Terminator::Jump { target, .. } if doms[head.0 as usize].contains(target) => {
                    loops.push((*target, head));
                }
                _ => {}
            }
        }

        loops
    }

    /// 返回原始支配信息：doms[b] = 支配 b 的所有块的集合。
    #[deprecated(
        since = "0.2.0",
        note = "Use `DominatorTree::build(func).dominates(a, b)` for O(1) dominance queries"
    )]
    pub(crate) fn dominator_tree_raw(&self) -> Vec<HashSet<BlockId>> {
        let n = self.blocks.len();
        if n == 0 {
            return Vec::new();
        }

        let preds = self.predecessors();
        let all_blocks: HashSet<BlockId> = (0..n as u32).map(BlockId).collect();
        let mut doms: Vec<HashSet<BlockId>> = vec![all_blocks; n];
        doms[0] = {
            let mut s = HashSet::new();
            s.insert(BlockId(0));
            s
        };

        let mut changed = true;
        while changed {
            changed = false;
            for b in 1..n {
                let mut new_dom: HashSet<BlockId> = if preds[b].is_empty() {
                    HashSet::new()
                } else {
                    let mut iter = preds[b].iter();
                    let first = *iter.next().unwrap();
                    let mut intersection = doms[first.0 as usize].clone();
                    for p in iter {
                        intersection = intersection
                            .intersection(&doms[p.0 as usize])
                            .copied()
                            .collect();
                    }
                    intersection
                };
                new_dom.insert(BlockId(b as u32));

                if new_dom != doms[b] {
                    doms[b] = new_dom;
                    changed = true;
                }
            }
        }

        doms
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::ir::builder::FunctionBuilder;

    #[test]
    fn test_validate_valid_function() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let v = b.iconst_i32(42);
        b.return_(&[v]);
        let func = b.finish();

        let result = func.validate();
        assert!(result.is_valid());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_validate_undefined_value() {
        let mut func = Function::new("test", Signature::void());
        let block_id = func.create_block_id();
        func.blocks.push(Block::new(block_id));
        // 使用一个未定义的 Value
        let bad_val = Value(999);
        let result_val = func.create_value();
        func.blocks[0].instructions.push(Instruction::new(
            Opcode::Iadd,
            smallvec::smallvec![bad_val, bad_val],
            Some(result_val),
            Type::I32,
        ));
        func.blocks[0].terminator = Terminator::Return {
            values: smallvec::smallvec![],
        };

        let result = func.validate();
        assert!(!result.is_valid());
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn test_predecessors() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, _) = b.create_block_with_params(&[(Type::I32, "x")]);
        let then_block = b.create_block();
        let else_block = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let cond = b.iconst_i32(1);
        b.branch(cond, then_block, else_block, &[], &[]);

        b.switch_to_block(then_block);
        b.jump(merge, &[]);

        b.switch_to_block(else_block);
        b.jump(merge, &[]);

        b.switch_to_block(merge);
        b.return_(&[]);

        let func = b.finish();
        let preds = func.predecessors();

        // merge 应该有两个前驱：then_block 和 else_block
        assert_eq!(preds[merge.0 as usize].len(), 2);
    }

    #[test]
    fn test_dominator_tree() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        let then_block = b.create_block();
        let else_block = b.create_block();
        let merge = b.create_block();

        b.switch_to_block(entry);
        let c = b.iconst_i32(1);
        b.branch(c, then_block, else_block, &[], &[]);

        b.switch_to_block(then_block);
        b.jump(merge, &[]);

        b.switch_to_block(else_block);
        b.jump(merge, &[]);

        b.switch_to_block(merge);
        b.return_(&[]);

        let func = b.finish();
        let dom_tree = func.dominator_tree();

        // entry 应该支配所有块
        // 直接支配：entry -> then, else, merge (在标准支配树中)
        assert!(!dom_tree.is_empty());
        // entry 应该至少直接支配 then_block 和 else_block
        assert!(dom_tree[0].len() >= 2);
    }

    #[test]
    fn test_detect_loops() {
        // 创建一个简单循环：entry -> loop_body -> (branch back to loop_body or exit)
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        let loop_body = b.create_block();
        let exit = b.create_block();

        b.switch_to_block(entry);
        b.jump(loop_body, &[]);

        b.switch_to_block(loop_body);
        let cond = b.iconst_i32(1);
        b.branch(cond, loop_body, exit, &[], &[]); // back-edge to loop_body

        b.switch_to_block(exit);
        b.return_(&[]);

        let func = b.finish();
        let loops = func.detect_loops();

        // 应该检测到至少一个循环（loop_body → loop_body 的 back-edge）
        assert!(!loops.is_empty());
    }

    #[test]
    fn test_display_function() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("add_one", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let one = b.iconst_i32(1);
        let result = b.iadd(params[0], one);
        b.return_(&[result]);
        let func = b.finish();

        let s = format!("{}", func);
        assert!(s.contains("fn add_one"));
        assert!(s.contains("iadd"));
        assert!(s.contains("ret"));
    }

    #[test]
    fn test_validate_empty_function() {
        let func = Function::new("empty", Signature::void());
        let result = func.validate();
        assert!(!result.is_valid());
    }

    #[test]
    fn test_multi_return_validation() {
        // Two return blocks both returning correct type
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("abs", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        let then_blk = b.create_block();
        let else_blk = b.create_block();

        b.switch_to_block(entry);
        let x = params[0];
        let zero = b.iconst_i32(0);
        let cond = b.icmp(IntCC::SignedGreaterThanOrEqual, x, zero);
        b.branch(cond, then_blk, else_blk, &[], &[]);

        b.switch_to_block(then_blk);
        b.return_(&[x]);

        b.switch_to_block(else_blk);
        let neg = b.isub(zero, x);
        b.return_(&[neg]);

        let func = b.finish();
        assert!(func.validate().is_valid());
    }

    #[test]
    fn test_return_type_mismatch() {
        // Returns void but signature expects i32
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("bad", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        b.return_(&[]); // wrong: should return 1 value

        let func = b.finish();
        let result = func.validate();
        assert!(!result.is_valid());
    }

    #[test]
    fn test_def_use_basic() {
        let sig = Signature::new(&[(Type::I32, "x")], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let (entry, params) = b.create_block_with_params(&[(Type::I32, "x")]);
        b.switch_to_block(entry);
        let x = params[0];
        let one = b.iconst_i32(1);
        let sum = b.iadd(x, one);
        b.return_(&[sum]);
        let func = b.finish();

        // x (block param) 应该被 iadd 使用
        let uses_of_x = func.uses(x);
        assert!(!uses_of_x.is_empty(), "x should have at least one use");
        assert!(
            uses_of_x.contains(&sum),
            "iadd result should be a user of x"
        );

        // one (iconst) 应该被 iadd 使用
        let uses_of_one = func.uses(one);
        assert!(!uses_of_one.is_empty(), "one should have at least one use");
        assert!(
            uses_of_one.contains(&sum),
            "iadd result should be a user of one"
        );

        // sum (iadd result) 不应该被任何指令使用（只有 terminator 引用它）
        let uses_of_sum = func.uses(sum);
        assert!(
            uses_of_sum.is_empty(),
            "sum should have no instruction users"
        );
    }

    #[test]
    fn test_def_use_add_remove() {
        let mut func = Function::new("test", Signature::void());
        let v1 = func.create_value();
        let v2 = func.create_value();
        let v3 = func.create_value();

        func.add_use(v1, v2);
        func.add_use(v1, v3);

        assert_eq!(func.uses(v1).len(), 2);
        assert!(func.has_uses(v1));
        assert!(!func.has_uses(v2));

        assert!(func.remove_use(v1, v2));
        assert_eq!(func.uses(v1).len(), 1);
        assert!(!func.remove_use(v2, v1)); // v2 has no users
    }

    #[test]
    fn test_rebuild_use_lists() {
        let sig = Signature::new(&[], &[Type::I32]);
        let mut b = FunctionBuilder::new("test", sig);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let a = b.iconst_i32(10);
        let b_val = b.iconst_i32(20);
        let c = b.iadd(a, b_val);
        b.return_(&[c]);

        let mut func = b.finish();

        // 验证 emit 已自动建立 def-use
        assert!(func.has_uses(a));
        assert!(func.has_uses(b_val));

        // 清空并重建
        func.clear_uses();
        assert!(!func.has_uses(a));

        func.rebuild_use_lists();
        assert!(func.has_uses(a));
        assert!(func.uses(a).contains(&c));
    }
}
