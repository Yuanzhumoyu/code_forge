//! IR 构建器 — 以编程方式构造 SSA 函数。
//!
//! # Example
//! ```ignore
//! let mut builder = FunctionBuilder::new("add", Signature::new(
//!     &[(Type::I32, "a"), (Type::I32, "b")],
//!     &[Type::I32],
//! ));
//! let (_, [a, b]) = builder.create_block_here_with([
//!     (Type::I32, "a"),
//!     (Type::I32, "b"),
//! ]);
//! let sum = builder.iadd(a, b);
//! builder.return_(&[sum]);
//! let func = builder.finish();
//! ```

use super::debug_info::{DebugInfo, SourceLocation};
use super::function::*;
use super::instructions::*;
use super::types::*;
use smallvec::{SmallVec, smallvec};

/// Builder error — returned when preconditions for builder methods are not met.
#[derive(Debug, Clone)]
pub enum BuilderError {
    /// No current block has been selected via `switch_to_block`.
    NoCurrentBlock,
    /// The specified block does not exist in the function.
    BlockNotFound(BlockId),
    /// The block has no parameters, but block_param was called.
    BlockHasNoParams(BlockId),
}

impl std::fmt::Display for BuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuilderError::NoCurrentBlock => {
                write!(f, "no current block selected — call switch_to_block first")
            }
            BuilderError::BlockNotFound(id) => {
                write!(f, "block {} does not exist", id)
            }
            BuilderError::BlockHasNoParams(id) => {
                write!(f, "block {} has no parameters", id)
            }
        }
    }
}

/// 函数 IR 构建器。
pub struct FunctionBuilder {
    func: Function,
    current_block: Option<BlockId>,
    /// 当前源码位置 — 后续 emit 的指令会附加此位置。
    current_location: Option<SourceLocation>,
}

impl FunctionBuilder {
    /// 创建一个新的函数构建器。
    pub fn new(name: &str, signature: Signature) -> Self {
        Self {
            func: Function::new(name, signature),
            current_block: None,
            current_location: None,
        }
    }

    /// 启用调试信息收集。后续 `set_source_location` 调用才会生效。
    pub fn enable_debug_info(&mut self) {
        self.func.debug_info = Some(DebugInfo::new());
    }

    /// 设置当前源码位置。后续 emit 的每条指令都会附加此位置。
    pub fn set_source_location(&mut self, file: &str, line: u32, column: u32) {
        self.current_location = Some(SourceLocation::new(file, line, column));
    }

    /// 清除当前源码位置（后续指令不附加位置）。
    pub fn clear_source_location(&mut self) {
        self.current_location = None;
    }

    /// 返回当前源码位置的引用。
    pub fn source_location(&self) -> Option<&SourceLocation> {
        self.current_location.as_ref()
    }

    /// 分配一个新的 SSA Value（不发射指令）。
    pub fn create_value(&mut self) -> Value {
        self.func.create_value()
    }

    /// 分配一个新的 SSA Value 并预先记录其类型。
    ///
    /// 后续可以通过 `func.value_type(v)` 查询此值的类型。
    pub fn create_typed_value(&mut self, ty: Type) -> Value {
        self.func.create_typed_value(ty)
    }

    /// 创建一个空的基本块并返回其 ID。
    /// 块以 `Unreachable` terminator 占位，后续由 `set_terminator` 替换。
    pub fn create_block(&mut self) -> BlockId {
        let id = self.func.create_block_id();
        let block = Block::new(id);
        // 确保 blocks 按索引对齐
        assert_eq!(id.0 as usize, self.func.blocks.len());
        self.func.blocks.push(block);
        id
    }

    /// 创建一个带参数的基本块，返回 `(block_id, param_values)`。
    pub fn create_block_with_params(
        &mut self,
        param_tys: &[(Type, &str)],
    ) -> (BlockId, Vec<Value>) {
        let id = self.func.create_block_id();
        let params: Vec<(Value, Type)> = param_tys
            .iter()
            .map(|(ty, _name)| {
                let v = self.func.create_typed_value(*ty);
                (v, *ty)
            })
            .collect();
        let param_values: Vec<Value> = params.iter().map(|(v, _)| *v).collect();
        let block = Block::with_params(id, params);
        assert_eq!(id.0 as usize, self.func.blocks.len());
        self.func.blocks.push(block);
        (id, param_values)
    }

    /// 获取已创建块的参数值列表。
    pub fn block_params(&self, block: BlockId) -> Vec<Value> {
        self.func
            .block(block)
            .map(|b| b.params.iter().map(|(v, _)| *v).collect())
            .unwrap_or_default()
    }

    /// 获取指定块的单个参数值（用于入口块的参数获取）。
    ///
    /// # Panics
    ///
    /// Panics if the block does not exist or has no parameters.
    pub fn block_param(&self, block: BlockId) -> Value {
        let block = self
            .func
            .block(block)
            .unwrap_or_else(|| panic!("{}", BuilderError::BlockNotFound(block)));
        block
            .params
            .first()
            .map(|(v, _)| *v)
            .unwrap_or_else(|| panic!("{}", BuilderError::BlockHasNoParams(block.id)))
    }

    /// 切换到指定块，后续的 `emit` 和 `set_terminator` 操作将作用于该块。
    ///
    /// # Panics
    ///
    /// Panics if the block does not exist.
    pub fn switch_to_block(&mut self, block: BlockId) {
        if self.func.block(block).is_none() {
            panic!("{}", BuilderError::BlockNotFound(block));
        }
        self.current_block = Some(block);
    }

    // ============================================================
    // P0: 自动切换块 — create + switch 一步完成
    // ============================================================

    /// 创建空块并自动切换到此块。
    /// 等价于 `create_block()` + `switch_to_block()`。
    pub fn create_block_here(&mut self) -> BlockId {
        let id = self.create_block();
        self.switch_to_block(id);
        id
    }

    /// 创建带参数的块并自动切换到此块。
    /// 等价于 `create_block_with_params()` + `switch_to_block()`。
    pub fn create_block_here_with_params(
        &mut self,
        param_tys: &[(Type, &str)],
    ) -> (BlockId, Vec<Value>) {
        let (id, params) = self.create_block_with_params(param_tys);
        self.switch_to_block(id);
        (id, params)
    }

    /// 从函数签名创建入口块（含参数），并自动切换到此块。
    ///
    /// 避免在 `Signature::new()` 和 `create_block_with_params()` 中重复声明参数类型。
    /// 返回参数值列表，配合 `block_params` 或数组解构使用。
    pub fn create_entry_block(&mut self) -> Vec<Value> {
        let param_tys: Vec<(Type, String)> = self
            .func
            .signature
            .params
            .iter()
            .map(|(ty, name)| (*ty, name.clone()))
            .collect();
        let param_refs: Vec<(Type, &str)> = param_tys
            .iter()
            .map(|(ty, name)| (*ty, name.as_str()))
            .collect();
        let (id, params) = self.create_block_with_params(&param_refs);
        self.switch_to_block(id);
        params
    }

    /// 创建带 N 个参数的块并自动切换。返回静态数组 `[Value; N]`（零堆分配）。
    ///
    /// 支持 Rust 原生数组解构：
    /// ```ignore
    /// let (entry, [a, b]) = b.create_block_here_with([
    ///     (Type::I32, "a"),
    ///     (Type::I32, "b"),
    /// ]);
    /// ```
    pub fn create_block_here_with<const N: usize>(
        &mut self,
        param_tys: [(Type, &str); N],
    ) -> (BlockId, [Value; N]) {
        let id = self.func.create_block_id();
        let values: [Value; N] = std::array::from_fn(|i| {
            self.func.create_typed_value(param_tys[i].0)
        });
        let params: Vec<(Value, Type)> = values
            .iter()
            .zip(param_tys.iter())
            .map(|(&v, (ty, _))| (v, *ty))
            .collect();
        let block = Block::with_params(id, params);
        assert_eq!(id.0 as usize, self.func.blocks.len());
        self.func.blocks.push(block);
        self.current_block = Some(id);
        (id, values)
    }

    // ============================================================
    // P1: RAII 块构建 + 当前块查询
    // ============================================================

    /// 返回当前选中的块 ID。
    pub fn current_block(&self) -> Option<BlockId> {
        self.current_block
    }

    /// 在指定块内执行闭包构建指令，闭包结束后自动恢复到之前的块。
    ///
    /// # Example
    /// ```ignore
    /// // 无返回值 — 和之前一样
    /// b.build_block(then_blk, |b| {
    ///     let v = b.iconst_i32(42);
    ///     b.jump(merge, &[v]);
    /// });
    ///
    /// // 有返回值 — 可将块内变量传递到外层（配合 Phi 使用）
    /// let v1 = b.build_block(then_blk, |b| {
    ///     let v = b.iconst_i32(42);
    ///     b.jump(merge, &[v]);
    ///     v // 返回给外层
    /// });
    /// ```
    pub fn build_block<R, F>(&mut self, block: BlockId, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        let prev = self.current_block;
        self.current_block = Some(block);
        let result = f(self);
        self.current_block = prev;
        result
    }

    // === 便捷构建方法（所有整数运算携带类型参数） ===

    pub fn iadd(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Iadd, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn isub(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Isub, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn imul(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Imul, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn udiv(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Udiv, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn sdiv(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Sdiv, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn urem(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Urem, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn srem(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Srem, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn band(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Band, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn bor(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Bor, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn bxor(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Bxor, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn bnot(&mut self, a: Value) -> Value {
        let ty = self.infer_unary_type(a).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Bnot, smallvec![a], Some(result), ty);
        result
    }

    pub fn ishl(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Ishl, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn ushr(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Ushr, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn sshr(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Sshr, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn icmp(&mut self, cond: IntCC, a: Value, b: Value) -> Value {
        let result = self.create_typed_value(Type::Bool);
        self.emit(
            Opcode::Icmp { cond },
            smallvec![a, b],
            Some(result),
            Type::Bool,
        );
        result
    }

    pub fn fcmp(&mut self, cond: FloatCC, a: Value, b: Value) -> Value {
        let result = self.create_typed_value(Type::Bool);
        self.emit(
            Opcode::Fcmp { cond, flags: FastMathFlags::NONE },
            smallvec![a, b],
            Some(result),
            Type::Bool,
        );
        result
    }

    pub fn load(&mut self, addr: Value, ty: Type) -> Value {
        let result = self.func.create_value();
        self.emit(Opcode::Load, smallvec![addr], Some(result), ty);
        result
    }

    pub fn store(&mut self, value: Value, addr: Value) {
        self.emit(Opcode::Store, smallvec![value, addr], None, Type::Void);
    }

    pub fn stack_load(&mut self, offset: i32, ty: Type) -> Value {
        let result = self.func.create_value();
        self.emit(Opcode::StackLoad { offset }, smallvec![], Some(result), ty);
        result
    }

    pub fn stack_store(&mut self, offset: i32, value: Value) {
        self.emit(
            Opcode::StackStore { offset },
            smallvec![value],
            None,
            Type::Void,
        );
    }

    pub fn stack_addr(&mut self, offset: i32) -> Value {
        let result = self.func.create_value();
        self.emit(
            Opcode::StackAddr { offset },
            smallvec![],
            Some(result),
            Type::Ptr,
        );
        result
    }

    pub fn global_addr(&mut self, global: u32) -> Value {
        let result = self.create_typed_value(Type::Ptr);
        self.emit(
            Opcode::GlobalAddr { global },
            smallvec![],
            Some(result),
            Type::Ptr,
        );
        result
    }

    pub fn iconst(&mut self, value: i64, ty: Type) -> Value {
        let result = self.create_typed_value(ty);
        let index = self
            .func
            .constant_pool
            .insert(crate::ir::Big::from_i64(value));
        self.emit(Opcode::Iconst { index }, smallvec![], Some(result), ty);
        result
    }

    // === 类型特定常量便捷方法 ===

    pub fn iconst_i8(&mut self, value: i8) -> Value {
        self.iconst(value as i64, Type::I8)
    }
    pub fn iconst_i16(&mut self, value: i16) -> Value {
        self.iconst(value as i64, Type::I16)
    }
    pub fn iconst_i32(&mut self, value: i32) -> Value {
        self.iconst(value as i64, Type::I32)
    }
    pub fn iconst_i64(&mut self, value: i64) -> Value {
        self.iconst(value, Type::I64)
    }

    pub fn fconst(&mut self, bits: u64, ty: Type) -> Value {
        let result = self.create_typed_value(ty);
        let fmt = ty.float_format().unwrap_or(crate::ir::FloatFormat::F64);
        let big = crate::ir::Big::from_bits(bits, fmt);
        let index = self.func.constant_pool.insert(big);
        self.emit(Opcode::Fconst { index }, smallvec![], Some(result), ty);
        result
    }

    /// 便捷方法：从 `f32` 值创建浮点常量。
    pub fn fconst_f32(&mut self, value: f32) -> Value {
        self.fconst(value.to_bits() as u64, Type::F32)
    }

    /// 便捷方法：从 `f64` 值创建浮点常量。
    pub fn fconst_f64(&mut self, value: f64) -> Value {
        self.fconst(value.to_bits(), Type::F64)
    }

    /// 条件选择 (conditional move): `if cond { a } else { b }`
    /// 结果类型自动从 `a` 和 `b` 的操作数类型推断。
    pub fn select(&mut self, cond: Value, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Select, smallvec![cond, a, b], Some(result), ty);
        result
    }

    // === 浮点算术 ===

    pub fn fadd(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::F64);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Fadd { flags: FastMathFlags::NONE }, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn fsub(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::F64);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Fsub { flags: FastMathFlags::NONE }, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn fmul(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::F64);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Fmul { flags: FastMathFlags::NONE }, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn fdiv(&mut self, a: Value, b: Value) -> Value {
        let ty = self.infer_binary_type(a, b).unwrap_or(Type::F64);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Fdiv { flags: FastMathFlags::NONE }, smallvec![a, b], Some(result), ty);
        result
    }

    pub fn fneg(&mut self, a: Value) -> Value {
        let ty = self.infer_unary_type(a).unwrap_or(Type::F64);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Fneg { flags: FastMathFlags::NONE }, smallvec![a], Some(result), ty);
        result
    }

    pub fn freeze(&mut self, a: Value) -> Value {
        let ty = self.infer_unary_type(a).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Freeze, smallvec![a], Some(result), ty);
        result
    }

    pub fn fsqrt(&mut self, a: Value) -> Value {
        let ty = self.infer_unary_type(a).unwrap_or(Type::F64);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Fsqrt { flags: FastMathFlags::NONE }, smallvec![a], Some(result), ty);
        result
    }

    pub fn fabs(&mut self, a: Value) -> Value {
        let ty = self.infer_unary_type(a).unwrap_or(Type::F64);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Fabs { flags: FastMathFlags::NONE }, smallvec![a], Some(result), ty);
        result
    }

    // === 类型转换 ===

    pub fn sextend(&mut self, a: Value, to_ty: Type) -> Value {
        let result = self.func.create_value();
        self.emit(Opcode::Sextend, smallvec![a], Some(result), to_ty);
        result
    }

    pub fn uextend(&mut self, a: Value, to_ty: Type) -> Value {
        let result = self.func.create_value();
        self.emit(Opcode::Uextend, smallvec![a], Some(result), to_ty);
        result
    }

    pub fn ireduce(&mut self, a: Value, to_ty: Type) -> Value {
        let result = self.func.create_value();
        self.emit(Opcode::Ireduce, smallvec![a], Some(result), to_ty);
        result
    }

    pub fn bitcast(&mut self, a: Value, to_ty: Type) -> Value {
        let result = self.func.create_value();
        self.emit(Opcode::Bitcast, smallvec![a], Some(result), to_ty);
        result
    }

    pub fn copy(&mut self, a: Value) -> Value {
        let ty = self.infer_unary_type(a).unwrap_or(Type::I32);
        let result = self.create_typed_value(ty);
        self.emit(Opcode::Copy, smallvec![a], Some(result), ty);
        result
    }

    /// 显式 NOP（用于对齐或占位）。
    pub fn nop(&mut self) {
        self.emit(Opcode::Nop, smallvec![], None, Type::Void);
    }

    // === 栈分配 ===

    /// 在栈上分配指定类型的空间。
    ///
    /// `allocated_ty`: 分配的元素类型。
    /// `count`: 元素数量（默认 1）。
    /// 返回指向分配空间的指针。
    pub fn alloca(&mut self, allocated_ty: Type, count: u32) -> Value {
        let result = self.create_typed_value(Type::Ptr);
        self.emit(
            Opcode::Alloca { count },
            smallvec![],
            Some(result),
            allocated_ty,
        );
        result
    }

    // === 地址计算 (GEP) ===

    /// GetElementPtr — 计算复合类型中嵌套元素的地址。
    ///
    /// `ptr`: 基地址指针。
    /// `indices`: 各层索引（整数 Value），依次遍历复合类型的嵌套层级。
    /// `indexed_ty`: 被索引的复合类型。
    /// `result_ty`: 结果指针类型（指向元素类型的指针）。
    ///
    /// 例如，对于 `struct { i32, [4 x i8] }`，
    /// `gep(ptr, &[i32_idx, array_idx], struct_ty, ptr_to_i8)`
    /// 可计算数组中某个元素的地址。
    pub fn gep(
        &mut self,
        ptr: Value,
        indices: &[Value],
        indexed_ty: Type,
        result_ty: Type,
    ) -> Value {
        let result = self.create_typed_value(result_ty);
        let mut operands = smallvec![ptr];
        operands.extend_from_slice(indices);
        self.emit(
            Opcode::GetElementPtr { indexed_ty },
            operands,
            Some(result),
            result_ty,
        );
        result
    }

    // === Phi 节点 ===

    /// 创建一个 Phi 节点，预先指定传入值-块配对。
    ///
    /// `incoming` 是 `(value, predecessor_block)` 配对列表。
    /// 例如：`phi(&[(v1, block_a), (v2, block_b)], Type::I32)`
    ///
    /// # Panics
    ///
    /// Panics if no current block has been selected or the block cannot be found.
    pub fn phi(&mut self, incoming: &[(Value, BlockId)], ty: Type) -> Value {
        let result = self.create_typed_value(ty);
        let inc: smallvec::SmallVec<[(Value, BlockId); 4]> =
            incoming.iter().map(|&(v, b)| (v, b)).collect();
        let operands: smallvec::SmallVec<[Value; 4]> = incoming.iter().map(|&(v, _)| v).collect();
        let mut inst = Instruction::new(Opcode::Phi { incoming: inc }, operands, Some(result), ty);
        inst.source_location = self.current_location.clone();
        if let Some(ref loc) = self.current_location
            && let Some(ref mut di) = self.func.debug_info
        {
            di.set_location(result, loc.clone());
        }
        let block_id = self
            .current_block_or_err()
            .unwrap_or_else(|e| panic!("{}", e));
        let block = Self::block_mut_or_err(&mut self.func, block_id)
            .unwrap_or_else(|e| panic!("{}", e));
        block.instructions.push(inst);
        result
    }

    /// 向当前块中已有的 Phi 节点添加一个传入值-块配对。
    ///
    /// `phi_val` 是 phi 节点的结果 Value。
    /// 等价于 phi 的 `[val, pred_block]` 条目。
    ///
    /// # Panics
    ///
    /// Panics if no current block has been selected or the block cannot be found.
    pub fn add_incoming(&mut self, phi_val: Value, val: Value, pred_block: BlockId) {
        let block_id = self
            .current_block_or_err()
            .unwrap_or_else(|e| panic!("{}", e));
        let block = Self::block_mut_or_err(&mut self.func, block_id)
            .unwrap_or_else(|e| panic!("{}", e));
        for inst in &mut block.instructions {
            if inst.result == Some(phi_val) {
                if let Opcode::Phi { ref mut incoming } = inst.opcode {
                    incoming.push((val, pred_block));
                    inst.operands.push(val);
                }
                break;
            }
        }
    }

    pub fn call(&mut self, func: FuncRef, args: &[Value], ret_tys: &[Type]) -> Vec<Value> {
        let results: Vec<Value> = ret_tys.iter().map(|_| self.func.create_value()).collect();
        let result = results.first().copied();
        let ret_ty = ret_tys.first().copied().unwrap_or(Type::Void);
        self.emit(
            Opcode::Call { func },
            SmallVec::from_slice(args),
            result,
            ret_ty,
        );
        results
    }

    pub fn call_indirect(&mut self, addr: Value, args: &[Value], ret_tys: &[Type]) -> Vec<Value> {
        let results: Vec<Value> = ret_tys.iter().map(|_| self.func.create_value()).collect();
        let mut operands = smallvec![addr];
        operands.extend_from_slice(args);
        let result = results.first().copied();
        let ret_ty = ret_tys.first().copied().unwrap_or(Type::Void);
        self.emit(Opcode::CallIndirect, operands, result, ret_ty);
        results
    }

    /// 调用返回单个值的函数。等价于 `self.call(func, args, &[ret_ty])[0]`。
    pub fn call_single(&mut self, func: FuncRef, args: &[Value], ret_ty: Type) -> Value {
        self.call(func, args, &[ret_ty])[0]
    }

    /// 间接调用返回单个值的函数。等价于 `self.call_indirect(addr, args, &[ret_ty])[0]`。
    pub fn call_indirect_single(&mut self, addr: Value, args: &[Value], ret_ty: Type) -> Value {
        self.call_indirect(addr, args, &[ret_ty])[0]
    }

    // === 终止指令 ===

    pub fn branch(
        &mut self,
        cond: Value,
        true_block: BlockId,
        false_block: BlockId,
        true_args: &[Value],
        false_args: &[Value],
    ) {
        self.set_terminator(Terminator::Branch {
            cond,
            true_block,
            false_block,
            true_args: SmallVec::from_slice(true_args),
            false_args: SmallVec::from_slice(false_args),
        });
    }

    pub fn jump(&mut self, target: BlockId, args: &[Value]) {
        self.set_terminator(Terminator::Jump {
            target,
            args: SmallVec::from_slice(args),
        });
    }

    pub fn return_(&mut self, values: &[Value]) {
        self.set_terminator(Terminator::Return {
            values: SmallVec::from_slice(values),
        });
    }

    pub fn unreachable(&mut self) {
        self.set_terminator(Terminator::Unreachable);
    }

    /// 多路分支（switch）：根据 `discriminant` 跳转到匹配的 case 块，
    /// 或 `default_block`（无匹配时）。
    ///
    /// `cases` 是 `(case_value, target_block, block_args)` 的切片。
    pub fn switch(
        &mut self,
        discriminant: Value,
        default_block: BlockId,
        cases: &[(i64, BlockId, &[Value])],
    ) {
        self.set_terminator(Terminator::Switch {
            discriminant,
            default_block,
            cases: cases
                .iter()
                .map(|(v, b, args)| (*v, *b, smallvec::SmallVec::from_slice(args)))
                .collect(),
        });
    }

    // === 内部方法 ===

    /// Returns the currently selected block ID, or `BuilderError::NoCurrentBlock`.
    fn current_block_or_err(&self) -> Result<BlockId, BuilderError> {
        self.current_block.ok_or(BuilderError::NoCurrentBlock)
    }

    /// Returns a mutable reference to the block, or `BuilderError::BlockNotFound`.
    fn block_mut_or_err(
        func: &mut Function,
        block_id: BlockId,
    ) -> Result<&mut Block, BuilderError> {
        func.block_mut(block_id)
            .ok_or(BuilderError::BlockNotFound(block_id))
    }

    /// 向当前块发射一条指令。
    ///
    /// # Panics
    ///
    /// Panics if no current block has been selected via `switch_to_block`.
    #[track_caller]
    pub fn emit(
        &mut self,
        opcode: Opcode,
        operands: SmallVec<[Value; 4]>,
        result: Option<Value>,
        ty: Type,
    ) {
        let block_id = self
            .current_block_or_err()
            .unwrap_or_else(|e| panic!("{e}"));
        let mut inst = Instruction::new(opcode, operands, result, ty);
        // 如果结果值已存在且未记录类型，立即记录
        if let Some(v) = result {
            if self.func.value_type(v).is_none() {
                self.func.set_value_type(v, ty);
            }
            // 记录 def-use：每个操作数被此结果值使用
            for &op in &inst.operands {
                self.func.add_use(op, v);
            }
        }
        // 附加当前源码位置
        if let Some(ref loc) = self.current_location {
            inst.source_location = Some(loc.clone());
            // 同时注册到 DebugInfo
            if let Some(ref mut di) = self.func.debug_info
                && let Some(v) = result
            {
                di.set_location(v, loc.clone());
            }
        }
        let block = Self::block_mut_or_err(&mut self.func, block_id)
            .unwrap_or_else(|e| panic!("{e}"));
        block.instructions.push(inst);
    }

    /// 设置当前块的终止指令。
    ///
    /// # Panics
    ///
    /// Panics if no current block has been selected via `switch_to_block`.
    #[track_caller]
    pub fn set_terminator(&mut self, term: Terminator) {
        let block_id = self
            .current_block_or_err()
            .unwrap_or_else(|e| panic!("{e}"));
        let block = Self::block_mut_or_err(&mut self.func, block_id)
            .unwrap_or_else(|e| panic!("{e}"));
        block.terminator = term;
    }

    // === 原子操作 ===

    /// 原子 Read-Modify-Write: `result = atomic_rmw(op, ptr, val, ordering)`
    /// 对 `ptr` 指向的内存执行原子操作并返回旧值。
    pub fn atomic_rmw(
        &mut self,
        op: AtomicRmwOp,
        ptr: Value,
        val: Value,
        ordering: Ordering,
        result_ty: Type,
    ) -> Value {
        let result = self.create_typed_value(result_ty);
        self.emit(
            Opcode::AtomicRmw { op, ordering },
            smallvec![ptr, val],
            Some(result),
            result_ty,
        );
        result
    }

    /// 原子 Compare-and-Exchange: CAS(ptr, cmp, new, ordering)
    /// 返回旧值。若旧值 == cmp，写入 new。
    pub fn cmpxchg(
        &mut self,
        ptr: Value,
        cmp: Value,
        new: Value,
        ordering: Ordering,
        result_ty: Type,
    ) -> Value {
        let result = self.create_typed_value(result_ty);
        self.emit(
            Opcode::Cmpxchg { ordering },
            smallvec![ptr, cmp, new],
            Some(result),
            result_ty,
        );
        result
    }

    /// 内存屏障 (Fence)：阻止内存访问跨此屏障重排。
    pub fn fence(&mut self, ordering: Ordering) {
        self.emit(Opcode::Fence { ordering }, smallvec![], None, Type::Void);
    }

    // === 复合类型操作 ===

    /// 从聚合体值中提取字段: `result = extractvalue(aggregate, index)`
    pub fn extract_value(&mut self, aggregate: Value, index: u32, result_ty: Type) -> Value {
        let result = self.create_typed_value(result_ty);
        self.emit(
            Opcode::ExtractValue { index },
            smallvec![aggregate],
            Some(result),
            result_ty,
        );
        result
    }

    /// 向聚合体中插入字段: `result = insertvalue(aggregate, value, index)`
    pub fn insert_value(
        &mut self,
        aggregate: Value,
        value: Value,
        index: u32,
        result_ty: Type,
    ) -> Value {
        let result = self.create_typed_value(result_ty);
        self.emit(
            Opcode::InsertValue { index },
            smallvec![aggregate, value],
            Some(result),
            result_ty,
        );
        result
    }

    // === SIMD Shuffle ===

    /// 向量 Shuffle: `result = shufflevector(v1, v2, mask)`
    pub fn shuffle_vector(&mut self, v1: Value, v2: Value, mask: [u8; 16], result_ty: Type) -> Value {
        let result = self.create_typed_value(result_ty);
        self.emit(
            Opcode::ShuffleVector { mask },
            smallvec![v1, v2],
            Some(result),
            result_ty,
        );
        result
    }

    /// 标记此函数为 const（可在编译时求值）。
    ///
    /// const 函数在 lowering 阶段，当所有实参是常量时，
    /// 会被 `IrInterpreter` 编译时求值，而非生成运行时调用。
    ///
    /// # 约束
    ///
    /// const 函数不能有副作用（Store、调用非 const 函数等）。
    pub fn set_const(&mut self, is_const: bool) {
        self.func.is_const = is_const;
    }

    // ============================================================
    // 类型推断辅助方法
    // ============================================================

    /// 尝试从操作数推断结果类型。
    ///
    /// 对二元整数/浮点算术运算，结果类型与操作数类型相同。
    fn infer_binary_type(&self, a: Value, b: Value) -> Option<Type> {
        let ta = self.func.value_type(a)?;
        let tb = self.func.value_type(b)?;
        if ta == tb { Some(ta) } else { None }
    }

    /// 尝试推断一元运算的结果类型。
    #[allow(dead_code)]
    fn infer_unary_type(&self, a: Value) -> Option<Type> {
        self.func.value_type(a)
    }

    /// 消费构建器，返回完成的 Function。
    pub fn finish(self) -> Function {
        self.func
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_simple_add() {
        let mut b = FunctionBuilder::new(
            "add",
            Signature::new(&[(Type::I32, "a"), (Type::I32, "b")], &[Type::I32]),
        );

        let (_, [a, b_val]) = b.create_block_here_with([
            (Type::I32, "a"),
            (Type::I32, "b"),
        ]);
        let sum = b.iadd(a, b_val);
        b.return_(&[sum]);

        let func = b.finish();
        assert_eq!(func.name, "add");
        assert_eq!(func.blocks.len(), 1);
        assert_eq!(func.blocks[0].instructions.len(), 1);
        assert_eq!(func.blocks[0].params.len(), 2);
        match &func.blocks[0].terminator {
            Terminator::Return { values } => assert_eq!(values.len(), 1),
            _ => panic!("expected Return terminator"),
        }
    }

    #[test]
    fn test_build_conditional() {
        let mut b = FunctionBuilder::new(
            "max",
            Signature::new(&[(Type::I32, "x"), (Type::I32, "y")], &[Type::I32]),
        );

        let (_, [x, y]) = b.create_block_here_with([
            (Type::I32, "x"),
            (Type::I32, "y"),
        ]);
        let then_block = b.create_block();
        let else_block = b.create_block();
        let merge_block = b.create_block();

        let cond = b.icmp(IntCC::SignedGreaterThan, x, y);
        b.branch(cond, then_block, else_block, &[], &[]);

        b.build_block(then_block, |b| {
            b.jump(merge_block, &[x]);
        });
        b.build_block(else_block, |b| {
            b.jump(merge_block, &[y]);
        });
        b.build_block(merge_block, |b| {
            b.return_(&[]);
        });

        let func = b.finish();
        assert_eq!(func.blocks.len(), 4);
    }

    #[test]
    fn test_build_with_load_store() {
        let mut b = FunctionBuilder::new(
            "load_add",
            Signature::new(&[(Type::Ptr, "p")], &[Type::I32]),
        );

        let (_, [p]) = b.create_block_here_with([(Type::Ptr, "p")]);

        let v = b.load(p, Type::I32);
        let c = b.iconst_i32(1);
        let sum = b.iadd(v, c);
        b.store(sum, p);
        b.return_(&[sum]);

        let func = b.finish();
        assert_eq!(func.blocks[0].instructions.len(), 4); // load, iconst, iadd, store
    }

    #[test]
    fn test_typed_value_tracking() {
        // 验证通过 emit 自动记录的值类型可以正确查询
        let mut b = FunctionBuilder::new(
            "typed_test",
            Signature::new(&[(Type::I64, "a"), (Type::I64, "b")], &[Type::I64]),
        );

        let (_, [a, b_val]) = b.create_block_here_with([
            (Type::I64, "a"),
            (Type::I64, "b"),
        ]);

        // 使用类型推断版方法
        let sum = b.iadd(a, b_val);
        let result = b.isub(sum, a);

        // 验证自动推断的类型
        assert_eq!(b.func.value_type(sum), Some(Type::I64));
        assert_eq!(b.func.value_type(result), Some(Type::I64));

        b.return_(&[result]);
        let func = b.finish();

        // 验证完成后的函数仍可查询类型
        assert_eq!(func.value_type(sum), Some(Type::I64));
        assert_eq!(func.value_type(result), Some(Type::I64));
        assert_eq!(func.blocks[0].instructions.len(), 2);
    }

    #[test]
    fn test_typed_create_value() {
        let mut b = FunctionBuilder::new("typed_create", Signature::new(&[], &[Type::I32]));

        b.create_block_here();

        // 使用 create_typed_value 提前创建并指定类型
        let v = b.create_typed_value(Type::F64);
        assert_eq!(b.func.value_type(v), Some(Type::F64));

        let c = b.iconst_i32(42);
        assert_eq!(b.func.value_type(c), Some(Type::I32));

        b.return_(&[c]);
        b.finish();
    }

    #[test]
    fn test_phi_builder() {
        let mut b = FunctionBuilder::new("phi_test", Signature::new(&[], &[Type::I32]));

        b.create_block_here();
        let left = b.create_block();
        let right = b.create_block();
        let merge = b.create_block();

        // entry: 条件分支到 left/right
        let cond = b.iconst_i32(1);
        b.branch(cond, left, right, &[], &[]);

        // left: iconst 10, jump merge
        b.switch_to_block(left);
        let v1 = b.iconst_i32(10);
        b.jump(merge, &[]);

        // right: iconst 20, jump merge
        b.switch_to_block(right);
        let v2 = b.iconst_i32(20);
        b.jump(merge, &[]);

        // merge: phi (从 left 或 right 来的值通过 phi 选择)
        b.switch_to_block(merge);
        let phi_val = b.phi(&[(v1, left), (v2, right)], Type::I32);
        b.return_(&[phi_val]);

        let func = b.finish();
        let validation = func.validate();
        assert!(
            validation.is_valid(),
            "Validation errors: {:?}",
            validation.errors
        );

        // 验证 phi 的 incoming 信息
        let merge_block = func.block(merge).unwrap();
        let phi_inst = &merge_block.instructions[0];
        if let Opcode::Phi { incoming } = &phi_inst.opcode {
            assert_eq!(incoming.len(), 2);
            assert_eq!(incoming[0], (v1, left));
            assert_eq!(incoming[1], (v2, right));
        } else {
            panic!("Expected Phi opcode");
        }
        assert_eq!(phi_inst.ty, Type::I32);
    }

    #[test]
    fn test_add_incoming() {
        let mut b = FunctionBuilder::new("phi_add_inc", Signature::new(&[], &[Type::I32]));

        b.create_block_here();
        let left = b.create_block();
        let right = b.create_block();
        let merge = b.create_block();

        let cond = b.iconst_i32(1);
        b.branch(cond, left, right, &[], &[]);

        b.switch_to_block(left);
        let v1 = b.iconst_i32(10);
        b.jump(merge, &[]);

        b.switch_to_block(right);
        let v2 = b.iconst_i32(20);
        b.jump(merge, &[]);

        // 先创建空 phi，再添加 incoming
        b.switch_to_block(merge);
        let phi_val = b.phi(&[], Type::I32);
        b.add_incoming(phi_val, v1, left);
        b.add_incoming(phi_val, v2, right);
        b.return_(&[phi_val]);

        let func = b.finish();
        let validation = func.validate();
        assert!(
            validation.is_valid(),
            "Validation errors: {:?}",
            validation.errors
        );

        let merge_block = func.block(merge).unwrap();
        let phi_inst = &merge_block.instructions[0];
        if let Opcode::Phi { incoming } = &phi_inst.opcode {
            assert_eq!(incoming.len(), 2);
        } else {
            panic!("Expected Phi opcode");
        }
    }

    #[test]
    fn test_alloca_instruction() {
        let mut b = FunctionBuilder::new("alloca_test", Signature::new(&[], &[Type::Ptr]));

        let entry = b.create_block_here();

        // 在栈上分配 1 个 i32
        let ptr = b.alloca(Type::I32, 1);
        // 存值到栈
        let val = b.iconst_i32(42);
        b.store(val, ptr);
        // 从栈加载
        let loaded = b.load(ptr, Type::I32);
        b.return_(&[loaded]);

        let func = b.finish();
        let result = func.validate();
        assert!(result.is_valid(), "Validation errors: {:?}", result.errors);

        // 验证 alloca 指令
        let entry_block = func.block(entry).unwrap();
        let alloca_inst = &entry_block.instructions[0];
        if let Opcode::Alloca { count } = alloca_inst.opcode {
            assert_eq!(count, 1);
        } else {
            panic!("Expected Alloca opcode, got {:?}", alloca_inst.opcode);
        }
        assert_eq!(alloca_inst.ty, Type::I32);
        assert!(alloca_inst.result.is_some());
    }

    #[test]
    fn test_gep_instruction() {
        let mut b = FunctionBuilder::new(
            "gep_test",
            Signature::new(&[(Type::Ptr, "arr")], &[Type::Ptr]),
        );

        let (entry, [arr_ptr]) = b.create_block_here_with([(Type::Ptr, "arr")]);

        let idx = b.iconst_i64(3);
        // gep(arr_ptr, [3], array_of_i32, ptr_to_i32)
        let elem_ptr = b.gep(arr_ptr, &[idx], Type::Array(0), Type::Ptr);
        b.return_(&[elem_ptr]);

        let func = b.finish();
        let result = func.validate();
        assert!(result.is_valid(), "Validation errors: {:?}", result.errors);

        // 验证 gep 指令
        let entry_block = func.block(entry).unwrap();
        let gep_inst = &entry_block.instructions[1]; // iconst then gep
        if let Opcode::GetElementPtr { indexed_ty } = gep_inst.opcode {
            assert_eq!(indexed_ty, Type::Array(0));
        } else {
            panic!("Expected GetElementPtr opcode, got {:?}", gep_inst.opcode);
        }
        assert_eq!(gep_inst.operands.len(), 2); // base_ptr + 1 index
    }
}
