//! IR 构建器。
//!
//! `FunctionBuilder` — 创建 Function/Blocks，并支持直接发射指令 (委托给内部 IRBuilder)。
//! `IRBuilder` — 低级指令构建器。
//!
//! # Example
//! ```ignore
//! let mut fb = FunctionBuilder::new("add", TypeStore::new(), sig);
//! fb.create_block_here();
//! let v = fb.iconst_i32(42);
//! fb.ret(&[v]);
//! let func = fb.finish().expect("build");
//! ```

use crate::types::Vector;

use crate::error::IrError;

use super::debug_info::SourceLocation;
use super::entity::*;
use super::function::Function;
use super::imm_str::ImmStr;
use super::immediate::Immediate;
use super::inst_flags::InstFlags;
use super::mem_flags::MemFlags;
use super::opcode::{FloatCC, IntCC, Opcode};
use super::terminator::Terminator;
use super::types::{FunctionSignature, TypeContext};
use smallvec::SmallVec;

// === 指令构建宏（IRBuilder 方法样板）===
// 二元/一元运算的类型断言 + upcast + emit1 高度重复，用宏生成：
// - `int_binop!`     整数二元（结果 = upcast）
// - `int_binop_ub!`  整数二元（带 MAY_UB：div/rem/sat）
// - `float_binop!`   浮点二元（结果 = upcast）
// - `unop_int!` / `unop_float!`  一元
// - `overflow_binop!`  溢出二元（返回 (value, overflow_i1)）
macro_rules! int_binop {
    ($name:ident, $op:ident) => {
        pub fn $name(&mut self, a: Value, b: Value) -> Value {
            let at = self.type_of(a);
            let bt = self.type_of(b);
            assert!(
                at.is_int() && bt.is_int(),
                concat!(stringify!($name), " only support int")
            );
            let t = at.upcast(bt).unwrap();
            self.emit1(Opcode::$op, vec![a, b], vec![], t, InstFlags::NONE)
        }
    };
}
macro_rules! int_binop_ub {
    ($name:ident, $op:ident) => {
        pub fn $name(&mut self, a: Value, b: Value) -> Value {
            let at = self.type_of(a);
            let bt = self.type_of(b);
            assert!(
                at.is_int() && bt.is_int(),
                concat!(stringify!($name), " only support int")
            );
            let t = at.upcast(bt).unwrap();
            self.emit1(Opcode::$op, vec![a, b], vec![], t, InstFlags::MAY_UB)
        }
    };
}
macro_rules! float_binop {
    ($name:ident, $op:ident) => {
        pub fn $name(&mut self, a: Value, b: Value) -> Value {
            let at = self.type_of(a);
            let bt = self.type_of(b);
            assert!(
                at.is_float() && bt.is_float(),
                concat!(stringify!($name), " only support float")
            );
            let t = at.upcast(bt).unwrap();
            self.emit1(Opcode::$op, vec![a, b], vec![], t, InstFlags::NONE)
        }
    };
}
macro_rules! unop_int {
    ($name:ident, $op:ident) => {
        pub fn $name(&mut self, a: Value) -> Value {
            let t = self.type_of(a);
            assert!(t.is_int(), concat!(stringify!($name), " only support int"));
            self.emit1(Opcode::$op, vec![a], vec![], t, InstFlags::NONE)
        }
    };
}
macro_rules! unop_float {
    ($name:ident, $op:ident) => {
        pub fn $name(&mut self, a: Value) -> Value {
            let t = self.type_of(a);
            assert!(
                t.is_float(),
                concat!(stringify!($name), " only support float")
            );
            self.emit1(Opcode::$op, vec![a], vec![], t, InstFlags::NONE)
        }
    };
}
macro_rules! overflow_binop {
    ($name:ident, $op:ident) => {
        pub fn $name(&mut self, a: Value, b: Value) -> (Value, Value) {
            let at = self.type_of(a);
            let bt = self.type_of(b);
            assert!(
                at.is_int() && bt.is_int(),
                concat!(stringify!($name), " only support int")
            );
            let t = at.upcast(bt).unwrap();
            let results = self.emit(
                Opcode::$op,
                vec![a, b],
                vec![],
                &[t, self.ctx.bool_ty()],
                InstFlags::MAY_UB,
            );
            (results[0], results[1])
        }
    };
}

pub struct FunctionBuilder {
    pub(crate) func: Function,
    ctx: TypeContext,
    cur_block: Block,
    block_open: bool,
    /// Current source location — applied to all instructions emitted until changed.
    current_loc: Option<SourceLocation>,
}

impl FunctionBuilder {
    pub fn new(name: impl Into<ImmStr>, ctx: TypeContext, signature: FunctionSignature) -> Self {
        let cc = signature.calling_convention;
        let sig_ref = ctx.register_signature(signature);
        let func = Function::new(name, ctx.clone(), sig_ref, cc);
        Self {
            func,
            ctx,
            cur_block: Block(0),
            block_open: false,
            current_loc: None,
        }
    }

    pub fn type_ctx(&self) -> &TypeContext {
        &self.ctx
    }

    /// 设置后续发射指令的源码位置（B1 per-statement line-tables：
    /// forge-rustc 每条 MIR statement lower 前调用，loc 附加到每条
    /// forge-ir 指令（Instruction.loc），主库 emission 据此生成
    /// (机器码偏移, 行) 行号表）。
    pub fn set_current_loc(&mut self, loc: Option<SourceLocation>) {
        self.current_loc = loc;
    }

    /// 返回 Value 的代码生成类型（该值不存在时返回 `None`）。
    ///
    /// 跨 crate 查询值类型的唯一公开入口——外部不得直接访问
    /// `func.dfg`（P3.1：`func` 字段已收为 `pub(crate)`）。
    pub fn value_type(&self, value: Value) -> Option<TypeId> {
        self.func.dfg.value_type(value)
    }

    // --- 块管理 ---
    pub fn create_entry_block(&mut self) -> (Block, Vec<Value>) {
        let param_tys = self.func.param_types();
        let (block, params) = self.func.dfg.make_block_with_params(&param_tys);
        self.func.layout.push(block);
        self.func.entry_block = Some(block);
        self.cur_block = block;
        self.block_open = true;
        (block, params)
    }
    /// 可选绑定：给 value 绑定一个名称（display/调试输出用 `%name`）。
    /// 名称绑定是可选的——不绑定则 display 自动编号；重复绑定同名值由
    /// display 消歧（`%x`、`%x_1`…）保证输出 SSA 唯一。同名重复绑定覆盖更新。
    pub fn bind_name(&mut self, value: Value, name: &str) -> &mut Self {
        let name_id = self.ctx.borrow_mut().intern_str(name);
        self.func.value_names.insert(value, name_id);
        self
    }

    /// 可选绑定：给 block 绑定一个名称（display 输出 `%name` 作为块标签）。
    pub fn bind_block_name(&mut self, block: Block, name: &str) -> &mut Self {
        let name_id = self.ctx.borrow_mut().intern_str(name);
        self.func.block_names.insert(block, name_id);
        self
    }

    pub fn create_block_here_with<const N: usize>(
        &mut self,
        params: [(TypeId, &str); N],
    ) -> (Block, [Value; N]) {
        let (block, values) = self.create_block_with_params(&params);
        self.cur_block = block;
        self.block_open = true;
        let arr: [Value; N] = values.try_into().unwrap_or_else(|_| {
            panic!("create_block_here_with: expected {N} params");
        });
        (block, arr)
    }

    pub fn build_block<R, F>(&mut self, block: Block, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        let prev = self.cur_block;
        let prev_open = self.block_open;
        self.cur_block = block;
        self.block_open = true;
        let result = f(self);
        self.cur_block = prev;
        self.block_open = prev_open;
        result
    }

    pub fn finish(mut self) -> Result<Function, IrError> {
        // 防御：每个基本块必须有显式终结符（ret/jump/branch/unreachable/switch）。
        // 漏写时块终结符保持默认 Unreachable，会被编译期无条件 lower 成 UD2，
        // 运行到该块即非法指令崩溃（曾导致 forge-tests 进程 0xC000001D）。
        for (i, bd) in self.func.dfg.blocks.iter().enumerate() {
            if !bd.has_terminator {
                return Err(IrError::Internal(format!(
                    "FunctionBuilder::finish: fn {} block {i} has no explicit terminator — \
                     each block must end with ret/jump/branch/unreachable/switch",
                    self.func.name
                )));
            }
        }
        self.func.types = self.ctx;
        Ok(self.func)
    }

    /// 构造向量常量（lane 位模式列表，每 lane 一个 u64）。
    ///
    /// 主 API：从值语义动态数组构造向量常量。
    /// 元素类型由 `T` 推导（i8..i128/u8..u128/f32/f64），向量类型为动态
    /// `vector_ty(T::vector_ty(), lanes.len())`。元素按 LE 字节序拼接存储
    /// （u128 → 16 字节，全位宽不丢失）。
    ///
    /// 从值数组构造向量常量（元素类型由 `T` 推导，向量类型 = `vector_ty(elem, len)`）。
    /// 字节端序默认 Little（x86 后端规范存储）；大端框架用 `vconst_with_endian`
    /// 显式指定端序。端序随常量记录进常量池，DSL 还原时按端序 from_le/from_be。
    ///
    /// ```
    /// # use forge_ir::builder::FunctionBuilder;
    /// # use forge_ir::{TypeContext, FunctionSignature, TypeId};
    /// let sig = FunctionSignature::new(&[], &[TypeId::F32]);
    /// let mut b = FunctionBuilder::new("f", TypeContext::new(), sig);
    /// b.create_block_here();
    /// let v = b.vconst(vec![1.0f32, 2.0, 3.0, 4.0]); // <4 x f32>
    /// ```
    pub fn vconst<T: Vector>(&mut self, lanes: Vec<T>) -> Value {
        self.vconst_with_endian(lanes, Endianness::Little)
    }

    /// 显式端序版 `vconst`：按 `endian` 将各元素值转为字节（Little → `to_le_bytes`、
    /// Big → `to_be_bytes`）拼接存储。大端框架的数值常量可经此构造。
    pub fn vconst_with_endian<T: Vector>(&mut self, lanes: Vec<T>, endian: Endianness) -> Value {
        let len = lanes.len() as u32;
        let data: Vec<u8> = lanes
            .into_iter()
            .flat_map(|lane| lane.lane_bytes(endian))
            .collect();
        let ty = self.ctx.vector_ty(T::vector_ty(), len);
        self.vconst_bytes_with_endian(data, ty, endian)
    }

    /// 便捷：从静态数组构造向量常量（与 `vconst` 的区别 = `[T; N]` vs `Vec<T>`；
    /// 空数组返回 `None`）。元素类型由 `T` 推导，向量类型为 `vector_ty(elem, N)`。
    ///
    /// ```
    /// # use forge_ir::builder::FunctionBuilder;
    /// # use forge_ir::{TypeContext, FunctionSignature, TypeId};
    /// let sig = FunctionSignature::new(&[], &[TypeId::F32]);
    /// let mut b = FunctionBuilder::new("f", TypeContext::new(), sig);
    /// b.create_block_here();
    /// let v = b.vconst_array([1.0f32, 2.0, 3.0, 4.0]); // <4 x f32>
    /// ```
    pub fn vconst_array<const N: usize, T: Vector>(&mut self, v: [T; N]) -> Value {
        assert!(N != 0, "vconst_array: array length must not be 0");
        let data: Vec<u8> = v
            .into_iter()
            .flat_map(|lane| lane.lane_bytes(Endianness::Little))
            .collect();
        let ty = self.ctx.vector_ty(T::vector_ty(), N as u32);
        self.vconst_bytes(data, ty)
    }
}

// ============================================================
// 指令方法（单层 FunctionBuilder —— 原 IRBuilder 方法体并入）
// ============================================================
impl FunctionBuilder {
    /// 创建带参数类型的基本块（无参数名）。
    pub fn create_block_with_tys(&mut self, param_tys: &[TypeId]) -> (Block, Vec<Value>) {
        let (block, param_values) = self.func.dfg.make_block_with_params(param_tys);
        self.func.layout.push(block);
        if self.func.entry_block.is_none() {
            self.func.entry_block = Some(block);
        }
        self.cur_block = block;
        self.block_open = true;
        (block, param_values)
    }
    pub fn create_block(&mut self) -> Block {
        let block = self.func.dfg.make_block();
        self.func.layout.push(block);
        if self.func.entry_block.is_none() {
            self.func.entry_block = Some(block);
        }
        block
    }

    pub fn create_block_here(&mut self) -> Block {
        let id = self.create_block();
        self.switch_to_block(id);
        id
    }
    pub fn create_block_with_params(&mut self, params: &[(TypeId, &str)]) -> (Block, Vec<Value>) {
        let param_tys: Vec<TypeId> = params.iter().map(|(t, _)| *t).collect();
        let (block, values) = self.create_block_with_tys(&param_tys);
        for ((_, name), v) in params.iter().zip(values.iter()) {
            if !name.is_empty() {
                self.bind_name(*v, name);
            }
        }
        (block, values)
    }
    pub fn switch_to_block(&mut self, block: Block) {
        self.cur_block = block;
        self.block_open = true;
    }
    pub fn current_block(&self) -> Block {
        self.cur_block
    }
    pub fn block_params(&self, block: Block) -> Vec<Value> {
        self.func.dfg.block_param_values(block).to_vec()
    }

    fn emit(
        &mut self,
        opcode: Opcode,
        operands: Vec<Value>,
        immediates: Vec<Immediate>,
        result_tys: &[TypeId],
        flags: InstFlags,
    ) -> SmallVec<[Value; 2]> {
        self.emit_with_mem(
            opcode,
            operands,
            immediates,
            result_tys,
            flags,
            MemFlags::NONE,
        )
    }

    /// 带内存访问标志的发射（load/store 系列）。
    fn emit_with_mem(
        &mut self,
        opcode: Opcode,
        operands: Vec<Value>,
        immediates: Vec<Immediate>,
        result_tys: &[TypeId],
        flags: InstFlags,
        mem_flags: MemFlags,
    ) -> SmallVec<[Value; 2]> {
        // 每指令查询环境变量是热路径浪费（Windows 上每次都是真实环境块扫描）——
        // 用 OnceLock 缓存一次查询结果。
        static TRACE_IR: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *TRACE_IR.get_or_init(|| std::env::var_os("FORGE_TRACE_IR").is_some()) {
            eprintln!("[ir] {:?} ops={:?} imms={:?}", opcode, operands, immediates);
        }
        assert!(self.block_open);
        let block = self.cur_block;
        // 一次拷贝：Vec → SmallVec（record_inst 复用原始 operands 引用）
        let ops: SmallVec<[Value; 4]> = SmallVec::from_iter(operands.iter().copied());
        let imms: SmallVec<[Immediate; 4]> = SmallVec::from_vec(immediates);
        let inst = self.func.dfg.make_inst_with_meta_and_loc(
            opcode,
            block,
            ops,
            imms,
            result_tys,
            flags,
            mem_flags,
            SmallVec::new(),
            self.current_loc.clone(),
        );
        self.func.use_lists.record_inst(inst, &operands);
        SmallVec::from_iter(self.func.dfg.inst_results(inst).iter().copied())
    }
    pub fn emit1(
        &mut self,
        opcode: Opcode,
        operands: Vec<Value>,
        immediates: Vec<Immediate>,
        result_ty: TypeId,
        flags: InstFlags,
    ) -> Value {
        self.emit(opcode, operands, immediates, &[result_ty], flags)[0]
    }
    fn type_of(&self, v: Value) -> TypeId {
        self.func
            .dfg
            .value_type(v)
            .unwrap_or_else(|| panic!("v{} no type", v.0))
    }

    pub fn iadd(&mut self, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(at.is_int() && bt.is_int(), "iadd only support int");
        if at == TypeId::BOOL && bt == TypeId::BOOL {
            // bool 加法 = 模 2（XOR）：1+1=0。后端 32 位 ADD 会产生 2（脏值，
            // 高位非 0），用 Bxor 保证结果 0/1 规范化且结果类型保持 BOOL。
            return self.emit1(Opcode::Bxor, vec![a, b], vec![], at, InstFlags::NONE);
        }
        let t = at.upcast(bt).unwrap();
        self.emit1(Opcode::Iadd, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn isub(&mut self, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        // 指针差（PTR - PTR，ptrdiff）是合法操作；is_int 已涵盖 bool/ptr
        assert!(at.is_int() && bt.is_int(), "isub only support int/ptr");
        if at == TypeId::BOOL && bt == TypeId::BOOL {
            // bool 减法 = 模 2（XOR）：0-1=1。后端 32 位 SUB 会产生 0xFFFFFFFF
            // （脏值），用 Bxor 保证结果 0/1 规范化且结果类型保持 BOOL。
            return self.emit1(Opcode::Bxor, vec![a, b], vec![], at, InstFlags::NONE);
        }
        let t = at.upcast(bt).unwrap();
        self.emit1(Opcode::Isub, vec![a, b], vec![], t, InstFlags::NONE)
    }
    int_binop!(imul, Imul);
    int_binop_ub!(udiv, Udiv);
    int_binop_ub!(sdiv, Sdiv);
    int_binop_ub!(urem, Urem);
    int_binop_ub!(srem, Srem);
    int_binop!(band, Band);
    int_binop!(bor, Bor);
    int_binop!(bxor, Bxor);
    pub fn bnot(&mut self, a: Value) -> Value {
        let t = self.type_of(a);
        assert!(t.is_int(), "bnot only support int");
        if t == TypeId::BOOL {
            // bool 取反规范化：~x & 1 == x ^ 1。后端 32 位 NOT 对 bool 输入
            // 产生 0xFFFFFFFF（-1），破坏 0/1 布尔语义；用 xor true（预置常量槽）
            // 保证结果规范化，且指令类型仍为 BOOL。
            let one = self.iconst_bool(true);
            self.emit1(Opcode::Bxor, vec![a, one], vec![], t, InstFlags::NONE)
        } else {
            self.emit1(Opcode::Bnot, vec![a], vec![], t, InstFlags::NONE)
        }
    }
    pub fn ishl(&mut self, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(at.is_int() && bt.is_int(), "ishl only support int");
        self.emit1(Opcode::Ishl, vec![a, b], vec![], at, InstFlags::NONE)
    }
    pub fn ushr(&mut self, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(at.is_int() && bt.is_int(), "ushr only support int");
        self.emit1(Opcode::Ushr, vec![a, b], vec![], at, InstFlags::NONE)
    }
    pub fn sshr(&mut self, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(at.is_int() && bt.is_int(), "sshr only support int");
        self.emit1(Opcode::Sshr, vec![a, b], vec![], at, InstFlags::NONE)
    }
    pub fn icmp(&mut self, cond: IntCC, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(at.is_int() && bt.is_int(), "icmp only support int");
        self.emit1(
            Opcode::Icmp { cond },
            vec![a, b],
            vec![],
            TypeId::BOOL,
            InstFlags::NONE,
        )
    }
    pub fn fcmp(&mut self, cond: FloatCC, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(at.is_float() && bt.is_float(), "fcmp only support float");
        self.emit1(
            Opcode::Fcmp { cond },
            vec![a, b],
            vec![],
            TypeId::BOOL,
            InstFlags::NONE,
        )
    }
    float_binop!(fadd, Fadd);
    float_binop!(fsub, Fsub);
    float_binop!(fmul, Fmul);
    float_binop!(fdiv, Fdiv);
    float_binop!(frem, Frem);
    unop_float!(fneg, Fneg);
    unop_float!(fabs, Fabs);
    unop_float!(fsqrt, Fsqrt);
    pub fn fadd_fast(&mut self, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(
            at.is_float() && bt.is_float(),
            "fadd_fast only support float"
        );
        let t = at.upcast(bt).unwrap();
        self.emit1(Opcode::Fadd, vec![a, b], vec![], t, InstFlags::FMF_FAST)
    }
    pub fn fadd_with_flags(&mut self, a: Value, b: Value, flags: InstFlags) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(
            at.is_float() && bt.is_float(),
            "fadd_with_flags only support float"
        );
        let t = at.upcast(bt).unwrap();
        self.emit1(Opcode::Fadd, vec![a, b], vec![], t, flags)
    }
    pub fn stack_addr(&mut self, offset: i32) -> Value {
        self.emit1(
            Opcode::StackAddr,
            vec![],
            vec![Immediate::Int(offset as i64)],
            TypeId::PTR,
            InstFlags::NONE,
        )
    }
    pub fn global_addr(&mut self, global: GlobalId) -> Value {
        self.emit1(
            Opcode::GlobalAddr,
            vec![],
            vec![Immediate::Global(global)],
            TypeId::PTR,
            InstFlags::NONE,
        )
    }
    pub fn alloca(&mut self, ty: TypeId, count: u32) -> Value {
        self.emit1(
            Opcode::Alloca,
            vec![],
            vec![Immediate::Type(ty), Immediate::Uint(count as u64)],
            TypeId::PTR,
            InstFlags::NONE,
        )
    }
    pub fn gep(&mut self, ptr: Value, indices: &[Value], indexed_ty: TypeId) -> Value {
        let mut operands = vec![ptr];
        operands.extend_from_slice(indices);
        self.emit1(
            Opcode::GetElementPtr,
            operands,
            vec![Immediate::Type(indexed_ty)],
            TypeId::PTR,
            InstFlags::NONE,
        )
    }
    pub fn load(&mut self, addr: Value, ty: TypeId) -> Value {
        self.emit1(Opcode::Load, vec![addr], vec![], ty, InstFlags::NONE)
    }
    /// 浮点 load：结果类型为 F32/F64 时用 Fload（后端按类型分派到
    /// MOVSD_RM 等浮点内存移动指令——普通 Load 的 mov_mem 是 GPR 指令，
    /// 会把 FPR 值当 GPR 编码，值错乱）。
    pub fn fload(&mut self, addr: Value, ty: TypeId) -> Value {
        self.emit1(Opcode::Fload, vec![addr], vec![], ty, InstFlags::NONE)
    }
    pub fn load_with_flags(&mut self, addr: Value, ty: TypeId, mem_flags: MemFlags) -> Value {
        self.emit_with_mem(
            Opcode::Load,
            vec![addr],
            vec![],
            &[ty],
            InstFlags::NONE,
            mem_flags,
        )[0]
    }
    pub fn store(&mut self, value: Value, addr: Value) {
        self.emit(
            Opcode::Store,
            vec![value, addr],
            vec![],
            &[],
            InstFlags::SIDE_EFFECT,
        );
    }
    /// 浮点 store：value 为 F32/F64 时用 Fstore（后端按类型分派到
    /// MOVSD_MR 等浮点内存移动指令）。
    pub fn fstore(&mut self, value: Value, addr: Value) {
        self.emit(
            Opcode::Fstore,
            vec![value, addr],
            vec![],
            &[],
            InstFlags::SIDE_EFFECT,
        );
    }
    /// store + 内存访问标志。参数顺序与 [`IRBuilder::store`] 一致：`(value, addr)`。
    pub fn store_with_flags(&mut self, value: Value, addr: Value, mem_flags: MemFlags) {
        self.emit_with_mem(
            Opcode::Store,
            vec![value, addr],
            vec![],
            &[],
            InstFlags::SIDE_EFFECT,
            mem_flags,
        );
    }

    // --- 向量/SIMD 操作 ---

    pub fn vadd(&mut self, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(
            self.ctx.is_vector(at) && self.ctx.is_vector(bt) && at == bt,
            "vadd only support same-typed vectors"
        );
        self.emit1(Opcode::Vadd, vec![a, b], vec![], at, InstFlags::NONE)
    }
    pub fn vsub(&mut self, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(
            self.ctx.is_vector(at) && self.ctx.is_vector(bt) && at == bt,
            "vsub only support same-typed vectors"
        );
        self.emit1(Opcode::Vsub, vec![a, b], vec![], at, InstFlags::NONE)
    }
    pub fn vmul(&mut self, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(
            self.ctx.is_vector(at) && self.ctx.is_vector(bt) && at == bt,
            "vmul only support same-typed vectors"
        );
        self.emit1(Opcode::Vmul, vec![a, b], vec![], at, InstFlags::NONE)
    }
    pub fn vextract(&mut self, vec: Value, index: Value) -> Value {
        let t = self.type_of(vec);
        assert!(self.ctx.is_vector(t), "vextract only support vector");
        // 结果类型 = 向量元素类型（如 <4 x f32> → f32），而非向量本身
        let elem_ty = self
            .ctx
            .element_type(t)
            .expect("vextract: vector elem type");
        // LLVM：idx 是操作数（i32 Value）——常量与变量统一
        self.emit1(
            Opcode::Vextract,
            vec![vec, index],
            vec![],
            elem_ty,
            InstFlags::NONE,
        )
    }
    pub fn vinsert(&mut self, vec: Value, elem: Value, index: Value) -> Value {
        let t = self.type_of(vec);
        assert!(self.ctx.is_vector(t), "vinsert only support vector");
        // LLVM：idx 是操作数（i32 Value）——常量与变量统一
        self.emit1(
            Opcode::Vinsert,
            vec![vec, elem, index],
            vec![],
            t,
            InstFlags::NONE,
        )
    }
    pub fn shuffle_vector(&mut self, a: Value, b: Value, mask: &[u32]) -> Value {
        let t = self.type_of(a);
        assert!(self.ctx.is_vector(t), "shuffle_vector only support vector");
        let immediates: Vec<Immediate> = mask.iter().map(|&m| Immediate::Uint(m as u64)).collect();
        self.emit1(
            Opcode::ShuffleVector,
            vec![a, b],
            immediates,
            t,
            InstFlags::NONE,
        )
    }

    // --- 原子操作 ---

    pub fn atomic_rmw(
        &mut self,
        op: crate::opcode::AtomicRmwOp,
        ptr: Value,
        val: Value,
        ordering: crate::opcode::Ordering,
    ) -> Value {
        let t = self.type_of(val);
        self.emit1(
            Opcode::AtomicRmw,
            vec![ptr, val],
            vec![Immediate::Uint(op as u64), Immediate::Uint(ordering as u64)],
            t,
            InstFlags::ATOMIC | InstFlags::SIDE_EFFECT,
        )
    }
    pub fn cmpxchg(
        &mut self,
        ptr: Value,
        cmp: Value,
        new: Value,
        ordering_success: crate::opcode::Ordering,
        ordering_failure: crate::opcode::Ordering,
        weak: bool,
    ) -> Value {
        let t = self.type_of(cmp);
        // Cmpxchg returns a 2-element struct { old_value, success_flag }
        self.emit1(
            Opcode::Cmpxchg,
            vec![ptr, cmp, new],
            vec![
                Immediate::Uint(ordering_success as u64),
                Immediate::Uint(ordering_failure as u64),
                Immediate::Uint(weak as u64),
            ],
            t,
            InstFlags::ATOMIC | InstFlags::SIDE_EFFECT,
        )
    }
    pub fn fence(&mut self, ordering: crate::opcode::Ordering) {
        self.emit(
            Opcode::Fence,
            vec![],
            vec![Immediate::Uint(ordering as u64)],
            &[],
            InstFlags::SIDE_EFFECT,
        );
    }

    // --- 复合类型操作 ---

    pub fn extract_value(&mut self, agg: Value, field_idx: u32) -> Value {
        let t = self.type_of(agg);
        // 结果类型 = 聚合类型（struct/array）第 field_idx 个元素/字段的类型
        let elem_ty = self
            .ctx
            .borrow()
            .aggregate_elem_type(t, field_idx)
            .unwrap_or(TypeId::VOID);
        self.emit1(
            Opcode::ExtractValue,
            vec![agg],
            vec![Immediate::Uint(field_idx as u64)],
            elem_ty,
            InstFlags::NONE,
        )
    }
    pub fn insert_value(&mut self, agg: Value, elem: Value, field_idx: u32) -> Value {
        let t = self.type_of(agg);
        self.emit1(
            Opcode::InsertValue,
            vec![agg, elem],
            vec![Immediate::Uint(field_idx as u64)],
            t,
            InstFlags::NONE,
        )
    }
    pub fn iconst_bool(&mut self, value: bool) -> Value {
        let cid = self.func.constants.bool_const(value);
        self.emit1(
            Opcode::Iconst,
            vec![],
            vec![Immediate::Const(cid)],
            TypeId::BOOL,
            InstFlags::NONE,
        )
    }
    pub fn iconst(&mut self, value: i64, ty: TypeId) -> Value {
        let bits = ty.bits().max(1); // use type's actual bit width, min 1
        let cid = self.func.constants.insert_int(value as i128, bits);
        self.emit1(
            Opcode::Iconst,
            vec![],
            vec![Immediate::Const(cid)],
            ty,
            InstFlags::NONE,
        )
    }
    pub fn iconst_i8(&mut self, v: i8) -> Value {
        self.iconst(v as i64, self.ctx.i8_ty())
    }
    pub fn iconst_i16(&mut self, v: i16) -> Value {
        self.iconst(v as i64, self.ctx.i16_ty())
    }
    pub fn iconst_i32(&mut self, v: i32) -> Value {
        self.iconst(v as i64, self.ctx.i32_ty())
    }
    pub fn iconst_i64(&mut self, v: i64) -> Value {
        self.iconst(v, self.ctx.i64_ty())
    }
    pub fn fconst(&mut self, bits: u64, ty: TypeId) -> Value {
        let cid = self.func.constants.insert_float(bits);
        self.emit1(
            Opcode::Fconst,
            vec![],
            vec![Immediate::Const(cid)],
            ty,
            InstFlags::NONE,
        )
    }

    /// f128 常量（128 位 IEEE 754 bits）。
    pub fn fconst128(&mut self, bits: u128, ty: TypeId) -> Value {
        let cid = self.func.constants.insert_float128(bits);
        self.emit1(
            Opcode::Fconst,
            vec![],
            vec![Immediate::Const(cid)],
            ty,
            InstFlags::NONE,
        )
    }

    pub fn fconst_f32(&mut self, v: f32) -> Value {
        self.fconst(v.to_bits() as u64, self.ctx.f32_ty())
    }

    pub fn fconst_f64(&mut self, v: f64) -> Value {
        self.fconst(v.to_bits(), self.ctx.f64_ty())
    }

    /// 底层：从原始字节构造向量常量（元素 Little 端序拼接，存入扁平字节池）。
    pub fn vconst_bytes(&mut self, data: Vec<u8>, ty: TypeId) -> Value {
        self.vconst_bytes_with_endian(data, ty, Endianness::Little)
    }

    /// 底层：显式端序版——字节按 `endian` 解释，端序记录进常量池供 DSL 还原。
    /// debug 下校验段长 = vector_len × elem_size（与 FunctionBuilder 版一致）。
    pub fn vconst_bytes_with_endian(
        &mut self,
        data: Vec<u8>,
        ty: TypeId,
        endian: Endianness,
    ) -> Value {
        let expect = self.ctx.borrow().vector_len(ty).unwrap_or(0) as usize
            * self
                .ctx
                .borrow()
                .element_type(ty)
                .map(|e| self.ctx.borrow().size_bytes(e) as usize)
                .unwrap_or(0);
        debug_assert_eq!(
            data.len(),
            expect,
            "vconst_bytes: byte length {data_len} must equal vector_len*elem_size = {expect}",
            data_len = data.len()
        );
        let cid = self.func.constants.insert_vector_with_endian(&data, endian);
        self.emit1(
            Opcode::Vconst,
            vec![],
            vec![Immediate::Const(cid)],
            ty,
            InstFlags::NONE,
        )
    }

    pub fn sextend(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Sextend,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn uextend(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Uextend,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn ireduce(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Ireduce,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn bitcast(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Bitcast,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn fptrunc(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Fptrunc,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn fpext(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Fpext,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn fptosi(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Fptosi,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn sitofp(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Sitofp,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn fptoui(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Fptoui,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn uitofp(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Uitofp,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn ptrtoint(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Ptrtoint,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn inttoptr(&mut self, v: Value, to: TypeId) -> Value {
        self.emit1(
            Opcode::Inttoptr,
            vec![v],
            vec![Immediate::Type(to)],
            to,
            InstFlags::NONE,
        )
    }
    pub fn copy(&mut self, v: Value) -> Value {
        let t = self.type_of(v);
        self.emit1(Opcode::Copy, vec![v], vec![], t, InstFlags::NONE)
    }
    pub fn select(&mut self, cond: Value, a: Value, b: Value) -> Value {
        let t = self.type_of(a);
        self.emit1(Opcode::Select, vec![cond, a, b], vec![], t, InstFlags::NONE)
    }
    pub fn freeze(&mut self, v: Value) -> Value {
        let t = self.type_of(v);
        self.emit1(Opcode::Freeze, vec![v], vec![], t, InstFlags::NONE)
    }

    // --- Bit manipulation (6) ---

    unop_int!(clz, Clz);
    unop_int!(ctz, Ctz);
    unop_int!(popcnt, Popcnt);
    unop_int!(bitreverse, Bitreverse);
    pub fn rotl(&mut self, a: Value, amount: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(amount);
        assert!(at.is_int() && bt.is_int(), "rotl only support int");
        self.emit1(Opcode::Rotl, vec![a, amount], vec![], at, InstFlags::NONE)
    }
    pub fn rotr(&mut self, a: Value, amount: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(amount);
        assert!(at.is_int() && bt.is_int(), "rotr only support int");
        self.emit1(Opcode::Rotr, vec![a, amount], vec![], at, InstFlags::NONE)
    }

    // --- Integer extended (10) ---

    unop_int!(abs, Abs);
    int_binop!(smin, Smin);
    int_binop!(smax, Smax);
    int_binop!(umin, Umin);
    int_binop!(umax, Umax);
    int_binop_ub!(sadd_sat, SaddSat);
    int_binop_ub!(ssub_sat, SsubSat);
    int_binop_ub!(uadd_sat, UaddSat);
    int_binop_ub!(usub_sat, UsubSat);
    unop_int!(bswap, Bswap);

    // --- Float extended (8) ---

    pub fn fma(&mut self, a: Value, b: Value, c: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        let ct = self.type_of(c);
        assert!(
            at.is_float() && bt.is_float() && ct.is_float(),
            "fma only support float"
        );
        let t = at.upcast(bt).unwrap();
        self.emit1(Opcode::Fma, vec![a, b, c], vec![], t, InstFlags::NONE)
    }
    float_binop!(fmin, Fmin);
    float_binop!(fmax, Fmax);
    float_binop!(fcopysign, Fcopysign);
    unop_float!(ffloor, Ffloor);
    unop_float!(fceil, Fceil);
    unop_float!(ftrunc, Ftrunc);
    unop_float!(fround, Fround);

    // --- Overflow arithmetic (6) — returns (value, overflow_flag: i1) ---

    overflow_binop!(sadd_overflow, SaddOverflow);
    overflow_binop!(uadd_overflow, UaddOverflow);
    overflow_binop!(ssub_overflow, SsubOverflow);
    overflow_binop!(usub_overflow, UsubOverflow);
    overflow_binop!(smul_overflow, SmulOverflow);
    overflow_binop!(umul_overflow, UmulOverflow);

    // --- Value semantics (2) ---

    pub fn poison(&mut self, ty: TypeId) -> Value {
        self.emit1(
            Opcode::Poison,
            vec![],
            vec![Immediate::Type(ty)],
            ty,
            InstFlags::NONE,
        )
    }
    pub fn undef(&mut self, ty: TypeId) -> Value {
        self.emit1(
            Opcode::Undef,
            vec![],
            vec![Immediate::Type(ty)],
            ty,
            InstFlags::NONE,
        )
    }

    // --- SIMD extended (5) ---

    pub fn vdiv(&mut self, a: Value, b: Value) -> Value {
        let at = self.type_of(a);
        let bt = self.type_of(b);
        assert!(
            self.ctx.is_vector(at) && self.ctx.is_vector(bt) && at == bt,
            "vdiv only support same-typed vectors"
        );
        self.emit1(Opcode::Vdiv, vec![a, b], vec![], at, InstFlags::MAY_UB)
    }
    pub fn vneg(&mut self, a: Value) -> Value {
        let t = self.type_of(a);
        assert!(self.ctx.is_vector(t), "vneg only support vector");
        self.emit1(Opcode::Vneg, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn vabs(&mut self, a: Value) -> Value {
        let t = self.type_of(a);
        assert!(self.ctx.is_vector(t), "vabs only support vector");
        self.emit1(Opcode::Vabs, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn vbitcast(&mut self, v: Value, to_ty: TypeId) -> Value {
        self.emit1(
            Opcode::Vbitcast,
            vec![v],
            vec![Immediate::Type(to_ty)],
            to_ty,
            InstFlags::NONE,
        )
    }
    pub fn vbroadcast(&mut self, elem: Value, vec_ty: TypeId) -> Value {
        self.emit1(
            Opcode::Vbroadcast,
            vec![elem],
            vec![Immediate::Type(vec_ty)],
            vec_ty,
            InstFlags::NONE,
        )
    }

    /// 宽向量 → 128 位片段提取（V256 → V128；fragment=0 低半 / 1 高半）。
    /// 结果类型 = 128 位向量（动态 `vector_ty(elem, 4)`，与 V128 同 id）。
    pub fn vsplit(&mut self, vec: Value, fragment: u32) -> Value {
        let t = self.type_of(vec);
        assert!(
            self.ctx.is_vector(t),
            "vsplit only support vector (got {t:?})"
        );
        let elem = self.ctx.element_type(t).expect("vsplit: vector elem type");
        let out_ty = self.ctx.vector_ty(elem, 4);
        self.emit1(
            Opcode::Vsplit,
            vec![vec],
            vec![Immediate::Uint(fragment as u64)],
            out_ty,
            InstFlags::NONE,
        )
    }

    /// 128 位片段拼接成宽向量（lo 为低 128 位、hi 为高 128 位 → 256 位）。
    /// 结果 lane 数 = 256 / 元素位宽（f32 → 8、f64/i64 → 4），与内建 V256 去重。
    pub fn vconcat(&mut self, lo: Value, hi: Value) -> Value {
        let lt = self.type_of(lo);
        assert!(
            self.ctx.is_vector(lt),
            "vconcat only support vector operands"
        );
        let elem = self
            .ctx
            .element_type(lt)
            .expect("vconcat: vector elem type");
        let elem_bits = self.ctx.borrow().size_bytes(elem) * 8;
        let out_ty = self.ctx.vector_ty(elem, 256 / elem_bits.max(1));
        self.emit1(
            Opcode::Vconcat,
            vec![lo, hi],
            vec![],
            out_ty,
            InstFlags::NONE,
        )
    }

    // --- Trap / pointer predicates (3) ---

    pub fn trap(&mut self) {
        self.emit(Opcode::Trap, vec![], vec![], &[], InstFlags::SIDE_EFFECT);
    }
    pub fn is_null(&mut self, ptr: Value) -> Value {
        self.emit1(
            Opcode::IsNull,
            vec![ptr],
            vec![],
            self.ctx.bool_ty(),
            InstFlags::NONE,
        )
    }
    pub fn is_not_null(&mut self, ptr: Value) -> Value {
        self.emit1(
            Opcode::IsNotNull,
            vec![ptr],
            vec![],
            self.ctx.bool_ty(),
            InstFlags::NONE,
        )
    }

    pub fn nop(&mut self) {
        self.emit(Opcode::Nop, vec![], vec![], &[], InstFlags::NONE);
    }
    pub fn ret(&mut self, values: &[Value]) {
        self.func.dfg.set_terminator(
            self.cur_block,
            Terminator::Return {
                values: values.iter().copied().collect(),
                metadata: SmallVec::new(),
            },
        );
        self.block_open = false;
    }

    pub fn jump(&mut self, target: Block, args: &[Value]) {
        self.func.dfg.set_terminator(
            self.cur_block,
            Terminator::Jump {
                target,
                args: args.iter().copied().collect(),
                metadata: SmallVec::new(),
            },
        );
        self.block_open = false;
    }
    pub fn branch(
        &mut self,
        cond: Value,
        then_block: Block,
        then_args: &[Value],
        else_block: Block,
        else_args: &[Value],
    ) {
        self.func.dfg.set_terminator(
            self.cur_block,
            Terminator::Branch {
                cond,
                then_block,
                then_args: then_args.iter().copied().collect(),
                else_block,
                else_args: else_args.iter().copied().collect(),
                metadata: SmallVec::new(),
            },
        );
        self.block_open = false;
    }
    pub fn unreachable(&mut self) {
        self.func
            .dfg
            .set_terminator(self.cur_block, Terminator::Unreachable);
        self.block_open = false;
    }
    pub fn switch(
        &mut self,
        discriminant: Value,
        default: Block,
        cases: &[(i64, Block, &[Value])],
    ) {
        #[allow(clippy::type_complexity)]
        let cases_sv: SmallVec<[(i64, Block, SmallVec<[Value; 2]>); 4]> = cases
            .iter()
            .map(|(v, b, args)| (*v, *b, args.iter().copied().collect()))
            .collect();
        self.func.dfg.set_terminator(
            self.cur_block,
            Terminator::Switch {
                discriminant,
                default_block: default,
                default_args: SmallVec::new(),
                cases: cases_sv,
                metadata: SmallVec::new(),
            },
        );
        self.block_open = false;
    }
    pub fn call(&mut self, func: FuncRef, args: &[Value], ret_tys: &[TypeId]) -> Vec<Value> {
        self.emit(
            Opcode::Call,
            args.to_vec(),
            vec![Immediate::Func(func)],
            ret_tys,
            InstFlags::SIDE_EFFECT,
        )
        .to_vec()
    }

    /// invoke 调用（LLVM `invoke <retty> @f(args) to label %ok unwind label %pad`）。
    /// 正常返回走 `normal`（返回值经 `normal_args` 传块参数），异常走 `unwind`。
    /// P1.1 文本层：仅构建/展示；codegen 对含 invoke 的函数报 Unsupported。
    #[allow(clippy::too_many_arguments)]
    pub fn invoke(
        &mut self,
        callee: FuncRef,
        args: &[Value],
        ret_ty: TypeId,
        normal_block: Block,
        normal_args: &[Value],
        unwind_block: Block,
        unwind_args: &[Value],
    ) {
        self.func.dfg.set_terminator(
            self.cur_block,
            Terminator::Invoke {
                callee,
                args: args.iter().copied().collect(),
                ret_ty,
                normal_block,
                normal_args: normal_args.iter().copied().collect(),
                unwind_block,
                unwind_args: unwind_args.iter().copied().collect(),
                metadata: SmallVec::new(),
            },
        );
        self.block_open = false;
    }

    /// resume 终结符（LLVM `resume <ty> %l`；重新抛出异常）。
    pub fn resume(&mut self, value: Value) {
        self.func.dfg.set_terminator(
            self.cur_block,
            Terminator::Resume {
                value,
                metadata: SmallVec::new(),
            },
        );
        self.block_open = false;
    }

    /// landingpad 指令（LLVM `landingpad <ty> cleanup`；返回异常值）。
    pub fn landingpad(&mut self, ty: TypeId) -> Value {
        self.emit1(
            Opcode::LandingPad,
            vec![],
            vec![],
            ty,
            InstFlags::SIDE_EFFECT,
        )
    }

    pub fn call_indirect(&mut self, ptr: Value, args: &[Value], ret_tys: &[TypeId]) -> Vec<Value> {
        let mut operands = vec![ptr];
        operands.extend_from_slice(args);
        self.emit(
            Opcode::CallIndirect,
            operands,
            vec![],
            ret_tys,
            InstFlags::SIDE_EFFECT,
        )
        .to_vec()
    }
}
// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_simple() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut fb = FunctionBuilder::new("test", TypeContext::new(), sig);
        fb.create_block_here();
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let func = fb.finish().expect("build");
        assert_eq!(func.dfg.block_count(), 1);
    }

    /// 内建向量类型注册：is_vector(V128) 可用、element_type 返回 f32。
    #[test]
    fn test_builtin_vector_types_registered() {
        let ctx = TypeContext::new();
        assert!(ctx.is_vector(TypeId::V64), "V64 应为向量类型");
        assert!(ctx.is_vector(TypeId::V128), "V128 应为向量类型");
        assert!(ctx.is_vector(TypeId::V256), "V256 应为向量类型");
        assert_eq!(ctx.element_type(TypeId::V128), Some(TypeId::F32));
        assert_eq!(ctx.size_bytes(TypeId::V128), 16);
        // 动态 vector_ty 与内建 V128 去重命中同一 TypeId
        assert_eq!(ctx.vector_ty(TypeId::F32, 4), TypeId::V128);
    }

    /// vconst 构造向量常量；vextract 结果类型为标量 f32。
    #[test]
    fn test_vconst_and_vextract_ty() {
        let sig = FunctionSignature::new(&[], &[TypeId::F32]);
        let mut fb = FunctionBuilder::new("vconst", TypeContext::new(), sig);
        fb.create_block_here();
        let v = fb.vconst(vec![1.5f32, 2.5, 3.5, 4.5]);
        assert_eq!(fb.func.dfg.value_type(v), Some(TypeId::V128));
        let zero = fb.iconst_i32(0);
        let e = fb.vextract(v, zero);
        assert_eq!(fb.func.dfg.value_type(e), Some(TypeId::F32));
        fb.ret(&[e]);
        fb.finish().expect("build");
    }

    /// vconst_array：数组 → 动态向量常量，类型为 vector_ty(elem, N)。
    #[test]
    fn test_vconst_array_ty() {
        let sig = FunctionSignature::new(&[], &[TypeId::F32]);
        let mut fb = FunctionBuilder::new("vconst_array", TypeContext::new(), sig);
        fb.create_block_here();

        // f32 数组 → <4 x f32>（去重命中内建 V128）
        let v = fb.vconst_array([1.0f32, 2.0, 3.0, 4.0]);
        assert_eq!(fb.func.dfg.value_type(v), Some(TypeId::V128));

        // f64 数组 → 动态 <2 x f64>（非内建）
        let v2 = fb.vconst_array([1.5f64, -2.5]);
        let ty2 = fb.type_ctx().vector_ty(TypeId::F64, 2);
        assert_eq!(fb.func.dfg.value_type(v2), Some(ty2));

        // i32 数组 → 动态 <4 x i32>（V128 内建仅 <4 x f32>，不参与去重）
        let v3 = fb.vconst_array([1i32, -1, 7, 2]);
        let ty3 = fb.type_ctx().vector_ty(TypeId::I32, 4);
        assert_eq!(fb.func.dfg.value_type(v3), Some(ty3));
        assert_eq!(
            fb.type_ctx()
                .element_type(fb.func.dfg.value_type(v3).unwrap()),
            Some(TypeId::I32)
        );

        fb.ret(&[]);
        fb.finish().expect("build");
    }

    /// vconst 泛型 u128：16 字节完整存储（不截断）——<1 x i128> 数据可查。
    #[test]
    fn test_vconst_u128_bytes_integrity() {
        let sig = FunctionSignature::new(&[], &[TypeId::I128]);
        let mut fb = FunctionBuilder::new("u128", TypeContext::new(), sig);
        fb.create_block_here();
        let v = fb.vconst(vec![u128::MAX]);
        let ty = fb.type_ctx().vector_ty(TypeId::I128, 1);
        assert_eq!(fb.func.dfg.value_type(v), Some(ty));
    }

    #[test]
    fn test_branch() {
        let ctx = TypeContext::new();
        let sig =
            FunctionSignature::new(&[(ctx.i32_ty(), "a"), (ctx.i32_ty(), "b")], &[ctx.i32_ty()]);
        let bi = sig.params[0].0;
        let mut fb = FunctionBuilder::new("max", ctx, sig);
        let (entry, params) = fb.create_entry_block();
        let then_blk = fb.create_block();
        let else_blk = fb.create_block();
        let merge_blk = fb.create_block_with_params(&[(bi, "r")]).0;
        fb.switch_to_block(entry);
        let cond = fb.icmp(IntCC::SignedGreaterThan, params[0], params[1]);
        fb.branch(cond, then_blk, &[], else_blk, &[]);
        fb.build_block(then_blk, |fb| {
            fb.jump(merge_blk, &[params[0]]);
        });
        fb.build_block(else_blk, |fb| {
            fb.jump(merge_blk, &[params[1]]);
        });
        fb.switch_to_block(merge_blk);
        let mp = fb.block_params(merge_blk);
        fb.ret(&[mp[0]]);
        let func = fb.finish().expect("build");
        assert_eq!(func.dfg.block_count(), 4);
    }

    // ── 安全检查：二元运算返回值向上转型 ──

    #[test]
    fn test_int_binary_upcast_result_ty() {
        let sig = FunctionSignature::new(&[], &[TypeId::I64]);
        let mut fb = FunctionBuilder::new("upcast", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.iconst_i32(1);
        let b = fb.iconst_i64(2);
        // 新覆盖的整数二元运算：结果类型应向上转型为 i64
        let r = fb.band(a, b);
        assert_eq!(fb.func.dfg.value_type(r), Some(TypeId::I64));
        let r2 = fb.sadd_sat(a, b);
        assert_eq!(fb.func.dfg.value_type(r2), Some(TypeId::I64));
        // 溢出运算：value 向上转型，overflow flag 为 bool
        let (v, f) = fb.sadd_overflow(a, b);
        assert_eq!(fb.func.dfg.value_type(v), Some(TypeId::I64));
        assert_eq!(fb.func.dfg.value_type(f), Some(TypeId::BOOL));
        fb.ret(&[r]);
    }

    #[test]
    fn test_float_binary_upcast_result_ty() {
        let sig = FunctionSignature::new(&[], &[TypeId::F64]);
        let mut fb = FunctionBuilder::new("fupcast", TypeContext::new(), sig);
        fb.create_block_here();
        let x = fb.fconst_f32(1.0);
        let y = fb.fconst_f64(2.0);
        let r = fb.fmin(x, y);
        assert_eq!(fb.func.dfg.value_type(r), Some(TypeId::F64));
        let r2 = fb.fma(x, y, y);
        assert_eq!(fb.func.dfg.value_type(r2), Some(TypeId::F64));
        fb.ret(&[r]);
    }

    #[test]
    fn test_shift_result_ty_keeps_lhs() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut fb = FunctionBuilder::new("shift", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.iconst_i32(1);
        let amt = fb.iconst_i8(2);
        // 移位结果类型取左操作数，不随 amount 类型改变
        let r = fb.ishl(a, amt);
        assert_eq!(fb.func.dfg.value_type(r), Some(TypeId::I32));
        fb.ret(&[r]);
    }

    // ── 安全检查：非法类型操作数触发断言 ──

    #[test]
    #[should_panic(expected = "band only support int")]
    fn test_band_float_panics() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("p1", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.fconst_f64(1.0);
        let b = fb.fconst_f64(2.0);
        fb.band(a, b);
    }

    #[test]
    #[should_panic(expected = "bnot only support int")]
    fn test_bnot_float_panics() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("p2", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.fconst_f64(1.0);
        fb.bnot(a);
    }

    #[test]
    #[should_panic(expected = "ishl only support int")]
    fn test_ishl_float_amount_panics() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("p3", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.iconst_i32(1);
        let amt = fb.fconst_f64(1.0);
        fb.ishl(a, amt);
    }

    #[test]
    #[should_panic(expected = "sadd_overflow only support int")]
    fn test_overflow_float_panics() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("p4", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.iconst_i32(1);
        let b = fb.fconst_f64(1.0);
        fb.sadd_overflow(a, b);
    }

    #[test]
    #[should_panic(expected = "fmin only support float")]
    fn test_fmin_int_panics() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("p5", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.iconst_i32(1);
        let b = fb.iconst_i32(2);
        fb.fmin(a, b);
    }

    #[test]
    #[should_panic(expected = "ffloor only support float")]
    fn test_ffloor_int_panics() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("p6", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.iconst_i32(1);
        fb.ffloor(a);
    }

    #[test]
    #[should_panic(expected = "vadd only support same-typed vectors")]
    fn test_vadd_scalar_panics() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("p7", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.iconst_i32(1);
        let b = fb.iconst_i32(2);
        fb.vadd(a, b);
    }

    #[test]
    #[should_panic(expected = "vextract only support vector")]
    fn test_vextract_scalar_panics() {
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("p8", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.iconst_i32(1);
        let zero = fb.iconst_i32(0);
        fb.vextract(a, zero);
    }

    // ── bool 特殊处理：is_int 涵盖 bool/ptr；bool×bool 保持 bool，bool×int 按 upcast 提升 ──

    /// iconst_bool 复用常量池预置的 0/1 槽位，不膨胀常量池。
    #[test]
    fn test_iconst_bool_reuses_preset_slot() {
        let sig = FunctionSignature::new(&[], &[TypeId::BOOL]);
        let mut fb = FunctionBuilder::new("bool_slot", TypeContext::new(), sig);
        fb.create_block_here();
        let _t1 = fb.iconst_bool(true);
        let _t2 = fb.iconst_bool(true);
        let f1 = fb.iconst_bool(false);
        // 常量池保持预置的 2 个槽位，反复调用不膨胀
        assert_eq!(fb.func.constants.total_len(), 2);
        // 收集所有 Iconst 指令的 ConstId，验证 true/false 各命中固定槽
        let mut const_ids = Vec::new();
        for (_, inst) in fb.func.dfg.insts() {
            if inst.opcode == Opcode::Iconst
                && let Some(Immediate::Const(cid)) = inst.immediates.first()
            {
                const_ids.push(*cid);
            }
        }
        assert_eq!(const_ids.len(), 3);
        assert_eq!(const_ids[0], const_ids[1]); // t1/t2 引用同一槽
        assert_eq!(const_ids[0].index(), 1); // true → index 1
        assert_eq!(const_ids[2].index(), 0); // false → index 0
        fb.ret(&[f1]);
    }

    /// bnot(bool) 规范化为 bxor(x, true)，结果保持 BOOL 类型。
    #[test]
    fn test_bnot_bool_normalized() {
        let sig = FunctionSignature::new(&[], &[TypeId::BOOL]);
        let mut fb = FunctionBuilder::new("bnot_bool", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.iconst_i32(1);
        let b = fb.iconst_i32(2);
        let cond = fb.icmp(IntCC::Equal, a, b); // bool false(0)
        let not = fb.bnot(cond);
        assert_eq!(fb.func.dfg.value_type(not), Some(TypeId::BOOL));
        // not 的定义指令应为 Bxor（而非 Bnot），且第二个操作数为预置 true 常量
        let crate::ValueDef::Inst(inst, _) = fb.func.dfg.value_def(not).unwrap() else {
            panic!("bnot result should be inst def");
        };
        let inst_data = fb.func.dfg.insts.get(inst.0 as usize).unwrap();
        assert_eq!(inst_data.opcode, Opcode::Bxor);
        fb.ret(&[not]);
    }

    /// iadd/isub 对 bool×bool 发射 Bxor（模 2 语义），混合 bool×int 保持 Iadd。
    #[test]
    fn test_bool_arith_normalized() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut fb = FunctionBuilder::new("bool_arith_norm", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.iconst_i32(1);
        let b = fb.iconst_i32(2);
        let c1 = fb.icmp(IntCC::Equal, a, a); // bool true
        let c2 = fb.icmp(IntCC::Equal, b, a); // bool false
        // bool×bool：iadd/isub 均生成 Bxor，结果保持 BOOL
        let add = fb.iadd(c1, c2);
        let sub = fb.isub(c1, c2);
        assert_eq!(fb.func.dfg.value_type(add), Some(TypeId::BOOL));
        assert_eq!(fb.func.dfg.value_type(sub), Some(TypeId::BOOL));
        for v in [add, sub] {
            let crate::ValueDef::Inst(inst, _) = fb.func.dfg.value_def(v).unwrap() else {
                panic!("bool arith result should be inst def");
            };
            let inst_data = fb.func.dfg.insts.get(inst.0 as usize).unwrap();
            assert_eq!(inst_data.opcode, Opcode::Bxor);
        }
        // bool×int 混合：保持 Iadd，结果 upcast 到 I32
        let mix = fb.iadd(c1, a);
        assert_eq!(fb.func.dfg.value_type(mix), Some(TypeId::I32));
        let crate::ValueDef::Inst(mix_inst, _) = fb.func.dfg.value_def(mix).unwrap() else {
            panic!("mixed result should be inst def");
        };
        let mix_data = fb.func.dfg.insts.get(mix_inst.0 as usize).unwrap();
        assert_eq!(mix_data.opcode, Opcode::Iadd);
        fb.ret(&[mix]);
    }

    #[test]
    fn test_bool_special_handling() {
        let sig = FunctionSignature::new(&[], &[TypeId::BOOL]);
        let mut fb = FunctionBuilder::new("bool_special", TypeContext::new(), sig);
        fb.create_block_here();
        let a = fb.iconst_i32(1);
        let b = fb.iconst_i32(2);
        let c1 = fb.icmp(IntCC::Equal, a, a); // bool（true）
        let c2 = fb.icmp(IntCC::NotEqual, b, b); // bool（false）
        // 逻辑运算：band(bool, bool) 保持 bool 结果
        let and = fb.band(c1, c2);
        assert_eq!(fb.func.dfg.value_type(and), Some(TypeId::BOOL));
        // 算术运算：iadd(bool, bool) 结果仍为 bool（upcast BOOL×BOOL → BOOL）
        let sum = fb.iadd(c1, c2);
        assert_eq!(fb.func.dfg.value_type(sum), Some(TypeId::BOOL));
        // 一元逻辑运算：bnot(bool) 保持 bool
        let not = fb.bnot(c1);
        assert_eq!(fb.func.dfg.value_type(not), Some(TypeId::BOOL));
        // bool×int 混合：upcast 提升为 int
        let mix = fb.iadd(c1, a);
        assert_eq!(fb.func.dfg.value_type(mix), Some(TypeId::I32));
        // ptr 参与整数运算：isub(ptr, ptr) 为合法 ptrdiff
        let p1 = fb.stack_addr(0);
        let p2 = fb.stack_addr(8);
        let diff = fb.isub(p1, p2);
        assert_eq!(fb.func.dfg.value_type(diff), Some(TypeId::PTR));
        fb.ret(&[and]);
    }
}
