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
//! let func = fb.finish();
//! ```

use super::debug_info::SourceLocation;
use super::entity::*;
use super::function::Function;
use super::immediate::Immediate;
use super::inst_flags::InstFlags;
use super::mem_flags::MemFlags;
use super::opcode::{FloatCC, IntCC, Opcode};
use super::terminator::Terminator;
use super::types::{FunctionSignature, TypeContext, TypeStore};
use smallvec::SmallVec;

macro_rules! delegate_fb {
    ($name:ident($($arg:ident : $ty:ty),*) -> $ret:ty) => {
        pub fn $name(&mut self, $($arg: $ty),*) -> $ret {
            let block = self.cur_block.expect("no current block");
            self.irb(block).$name($($arg),*)
        }
    };
    ($name:ident($($arg:ident : $ty:ty),*)) => {
        pub fn $name(&mut self, $($arg: $ty),*) {
            let block = self.cur_block.expect("no current block");
            self.irb(block).$name($($arg),*)
        }
    };
}

pub struct FunctionBuilder {
    pub func: Function,
    ctx: TypeContext,
    cur_block: Option<Block>,
}

impl FunctionBuilder {
    pub fn new(name: &str, ctx: TypeContext, signature: FunctionSignature) -> Self {
        let param_tys = signature.param_types();
        let return_tys = signature.returns.clone();
        let cc = signature.calling_convention;
        let sig_ref = ctx.register_signature(signature);
        let func = Function::new(name.to_string(), sig_ref, param_tys, return_tys, cc);
        Self {
            func,
            ctx,
            cur_block: None,
        }
    }

    pub fn type_store(&self) -> std::sync::RwLockReadGuard<'_, TypeStore> {
        self.ctx.borrow()
    }

    pub fn type_ctx(&self) -> &TypeContext {
        &self.ctx
    }

    /// 创建块/函数后获取 IRBuilder (高级用法).
    ///
    /// TypeContext is `Clone` (cheap `Rc` bump), so the IRBuilder gets
    /// its own handle — no more unsafe raw pointer cast.
    pub fn irb(&mut self, block: Block) -> IRBuilder<'_> {
        self.cur_block = Some(block);
        IRBuilder::new(&mut self.func, self.ctx.clone(), block)
    }

    // --- 块管理 ---
    pub fn create_entry_block(&mut self) -> (Block, Vec<Value>) {
        let param_tys: Vec<TypeId> = self.func.param_tys.clone();
        let (block, params) = self.func.dfg.make_block_with_params(&param_tys);
        self.func.layout.push(block);
        self.func.entry_block = Some(block);
        self.cur_block = Some(block);
        (block, params)
    }
    /// 创建带参数的基本块，并把参数名绑定到参数 value（可选绑定——
    /// 绑定后 display/调试输出使用 `%name`；`create_block_with_tys` 不绑定则
    /// 自动编号 `%v{index}`）。
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

    /// 创建带参数类型的基本块（无参数名）。
    pub fn create_block_with_tys(&mut self, param_tys: &[TypeId]) -> (Block, Vec<Value>) {
        let (block, param_values) = self.func.dfg.make_block_with_params(param_tys);
        self.func.layout.push(block);
        if self.func.entry_block.is_none() {
            self.func.entry_block = Some(block);
        }
        self.cur_block = Some(block);
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
        let block = self.create_block();
        self.switch_to_block(block);
        block
    }
    pub fn create_block_here_with<const N: usize>(
        &mut self,
        params: [(TypeId, &str); N],
    ) -> (Block, [Value; N]) {
        let (block, values) = self.create_block_with_params(&params);
        self.cur_block = Some(block);
        let arr: [Value; N] = values.try_into().unwrap_or_else(|_| {
            panic!("create_block_here_with: expected {N} params");
        });
        (block, arr)
    }
    pub fn switch_to_block(&mut self, block: Block) {
        self.cur_block = Some(block);
    }

    pub fn build_block<R, F>(&mut self, block: Block, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        let prev = self.cur_block;
        self.cur_block = Some(block);
        let result = f(self);
        self.cur_block = prev;
        result
    }

    pub fn build(&mut self, block: Block) -> IRBuilder<'_> {
        self.irb(block)
    }

    pub fn finish(self) -> Function {
        self.func
    }

    // --- 指令方法 (委托给 IRBuilder) ---

    delegate_fb!(iadd(a: Value, b: Value) -> Value);
    delegate_fb!(isub(a: Value, b: Value) -> Value);
    delegate_fb!(imul(a: Value, b: Value) -> Value);
    delegate_fb!(udiv(a: Value, b: Value) -> Value);
    delegate_fb!(sdiv(a: Value, b: Value) -> Value);
    delegate_fb!(urem(a: Value, b: Value) -> Value);
    delegate_fb!(srem(a: Value, b: Value) -> Value);
    delegate_fb!(band(a: Value, b: Value) -> Value);
    delegate_fb!(bor(a: Value, b: Value) -> Value);
    delegate_fb!(bxor(a: Value, b: Value) -> Value);
    delegate_fb!(bnot(a: Value) -> Value);
    delegate_fb!(ishl(a: Value, b: Value) -> Value);
    delegate_fb!(ushr(a: Value, b: Value) -> Value);
    delegate_fb!(sshr(a: Value, b: Value) -> Value);
    delegate_fb!(icmp(cond: IntCC, a: Value, b: Value) -> Value);
    delegate_fb!(fcmp(cond: FloatCC, a: Value, b: Value) -> Value);
    delegate_fb!(fadd(a: Value, b: Value) -> Value);
    delegate_fb!(fsub(a: Value, b: Value) -> Value);
    delegate_fb!(fmul(a: Value, b: Value) -> Value);
    delegate_fb!(fdiv(a: Value, b: Value) -> Value);
    delegate_fb!(fneg(a: Value) -> Value);
    delegate_fb!(fabs(a: Value) -> Value);
    delegate_fb!(fsqrt(a: Value) -> Value);
    delegate_fb!(fadd_fast(a: Value, b: Value) -> Value);
    delegate_fb!(fadd_with_flags(a: Value, b: Value, flags: InstFlags) -> Value);
    delegate_fb!(stack_addr(offset: i32) -> Value);
    delegate_fb!(global_addr(global: GlobalId) -> Value);
    delegate_fb!(alloca(ty: TypeId, count: u32) -> Value);
    delegate_fb!(gep(ptr: Value, indices: &[Value], indexed_ty: TypeId) -> Value);
    delegate_fb!(load(addr: Value, ty: TypeId) -> Value);
    pub fn store(&mut self, value: Value, addr: Value) {
        self.irb(self.cur_block.expect("no current block"))
            .store(value, addr);
    }
    delegate_fb!(vadd(a: Value, b: Value) -> Value);
    delegate_fb!(vsub(a: Value, b: Value) -> Value);
    delegate_fb!(vmul(a: Value, b: Value) -> Value);
    delegate_fb!(vextract(vec: Value, index: u32) -> Value);
    delegate_fb!(vinsert(vec: Value, elem: Value, index: u32) -> Value);
    delegate_fb!(extract_value(agg: Value, field_idx: u32) -> Value);
    delegate_fb!(insert_value(agg: Value, elem: Value, field_idx: u32) -> Value);
    pub fn shuffle_vector(&mut self, a: Value, b: Value, mask: &[u32]) -> Value {
        self.irb(self.cur_block.expect("no current block"))
            .shuffle_vector(a, b, mask)
    }
    pub fn atomic_rmw(
        &mut self,
        op: crate::opcode::AtomicRmwOp,
        ptr: Value,
        val: Value,
        ordering: crate::opcode::Ordering,
    ) -> Value {
        self.irb(self.cur_block.expect("no current block"))
            .atomic_rmw(op, ptr, val, ordering)
    }
    pub fn cmpxchg(
        &mut self,
        ptr: Value,
        cmp: Value,
        new: Value,
        ordering_success: crate::opcode::Ordering,
        ordering_failure: crate::opcode::Ordering,
    ) -> Value {
        self.irb(self.cur_block.expect("no current block")).cmpxchg(
            ptr,
            cmp,
            new,
            ordering_success,
            ordering_failure,
        )
    }
    pub fn fence(&mut self, ordering: crate::opcode::Ordering) {
        self.irb(self.cur_block.expect("no current block"))
            .fence(ordering)
    }
    delegate_fb!(iconst(value: i64, ty: TypeId) -> Value);
    delegate_fb!(iconst_i8(value: i8) -> Value);
    delegate_fb!(iconst_i16(value: i16) -> Value);
    delegate_fb!(iconst_i32(value: i32) -> Value);
    delegate_fb!(iconst_i64(value: i64) -> Value);
    delegate_fb!(fconst(bits: u64, ty: TypeId) -> Value);
    delegate_fb!(fconst_f32(value: f32) -> Value);
    delegate_fb!(fconst_f64(value: f64) -> Value);
    delegate_fb!(sextend(val: Value, to_ty: TypeId) -> Value);
    delegate_fb!(uextend(val: Value, to_ty: TypeId) -> Value);
    delegate_fb!(ireduce(val: Value, to_ty: TypeId) -> Value);
    delegate_fb!(bitcast(val: Value, to_ty: TypeId) -> Value);
    delegate_fb!(copy(val: Value) -> Value);
    delegate_fb!(select(cond: Value, a: Value, b: Value) -> Value);
    delegate_fb!(freeze(val: Value) -> Value);
    delegate_fb!(nop());
    delegate_fb!(ret(values: &[Value]));
    delegate_fb!(jump(target: Block, args: &[Value]));
    delegate_fb!(branch(cond: Value, then_block: Block, then_args: &[Value], else_block: Block, else_args: &[Value]));
    delegate_fb!(unreachable());
    pub fn switch(
        &mut self,
        discriminant: Value,
        default: Block,
        cases: &[(i64, Block, &[Value])],
    ) {
        let block = self.cur_block.expect("no current block");
        self.irb(block).switch(discriminant, default, cases)
    }

    pub fn call(&mut self, func: FuncRef, args: &[Value], ret_tys: &[TypeId]) -> Vec<Value> {
        self.irb(self.cur_block.expect("no current block"))
            .call(func, args, ret_tys)
    }
    pub fn call_indirect(&mut self, ptr: Value, args: &[Value], ret_tys: &[TypeId]) -> Vec<Value> {
        self.irb(self.cur_block.expect("no current block"))
            .call_indirect(ptr, args, ret_tys)
    }

    // --- Bit manipulation (6) ---

    delegate_fb!(clz(a: Value) -> Value);
    delegate_fb!(ctz(a: Value) -> Value);
    delegate_fb!(popcnt(a: Value) -> Value);
    delegate_fb!(bitreverse(a: Value) -> Value);
    delegate_fb!(rotl(a: Value, amount: Value) -> Value);
    delegate_fb!(rotr(a: Value, amount: Value) -> Value);

    // --- Integer extended (10) ---

    delegate_fb!(abs(a: Value) -> Value);
    delegate_fb!(smin(a: Value, b: Value) -> Value);
    delegate_fb!(smax(a: Value, b: Value) -> Value);
    delegate_fb!(umin(a: Value, b: Value) -> Value);
    delegate_fb!(umax(a: Value, b: Value) -> Value);
    delegate_fb!(sadd_sat(a: Value, b: Value) -> Value);
    delegate_fb!(ssub_sat(a: Value, b: Value) -> Value);
    delegate_fb!(uadd_sat(a: Value, b: Value) -> Value);
    delegate_fb!(usub_sat(a: Value, b: Value) -> Value);
    delegate_fb!(bswap(a: Value) -> Value);

    // --- Float extended (8) ---

    delegate_fb!(fma(a: Value, b: Value, c: Value) -> Value);
    delegate_fb!(fmin(a: Value, b: Value) -> Value);
    delegate_fb!(fmax(a: Value, b: Value) -> Value);
    delegate_fb!(fcopysign(a: Value, b: Value) -> Value);
    delegate_fb!(ffloor(a: Value) -> Value);
    delegate_fb!(fceil(a: Value) -> Value);
    delegate_fb!(ftrunc(a: Value) -> Value);
    delegate_fb!(fround(a: Value) -> Value);

    // --- Overflow arithmetic (6) — returns (value, overflow_flag) ---

    pub fn sadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.irb(self.cur_block.expect("no current block"))
            .sadd_overflow(a, b)
    }
    pub fn uadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.irb(self.cur_block.expect("no current block"))
            .uadd_overflow(a, b)
    }
    pub fn ssub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.irb(self.cur_block.expect("no current block"))
            .ssub_overflow(a, b)
    }
    pub fn usub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.irb(self.cur_block.expect("no current block"))
            .usub_overflow(a, b)
    }
    pub fn smul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.irb(self.cur_block.expect("no current block"))
            .smul_overflow(a, b)
    }
    pub fn umul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.irb(self.cur_block.expect("no current block"))
            .umul_overflow(a, b)
    }

    // --- Value semantics (2) ---

    delegate_fb!(poison(ty: TypeId) -> Value);
    delegate_fb!(undef(ty: TypeId) -> Value);

    // --- SIMD extended (5) ---

    delegate_fb!(vdiv(a: Value, b: Value) -> Value);
    delegate_fb!(vneg(a: Value) -> Value);
    delegate_fb!(vabs(a: Value) -> Value);
    delegate_fb!(vbitcast(v: Value, to_ty: TypeId) -> Value);
    delegate_fb!(vbroadcast(elem: Value, vec_ty: TypeId) -> Value);

    // --- Trap / pointer predicates (3) ---

    pub fn trap(&mut self) {
        self.irb(self.cur_block.expect("no current block")).trap();
    }
    delegate_fb!(is_null(ptr: Value) -> Value);
    delegate_fb!(is_not_null(ptr: Value) -> Value);
}

// ============================================================
// IRBuilder
// ============================================================

pub struct IRBuilder<'f> {
    func: &'f mut Function,
    ctx: TypeContext,
    cur_block: Block,
    block_open: bool,
    /// Current source location — applied to all instructions emitted until changed.
    current_loc: Option<SourceLocation>,
}

impl<'f> IRBuilder<'f> {
    pub fn new(func: &'f mut Function, ctx: TypeContext, initial_block: Block) -> Self {
        Self {
            func,
            ctx,
            cur_block: initial_block,
            block_open: true,
            current_loc: None,
        }
    }

    /// Access the TypeContext for operations that need mutable type store access.
    pub fn type_ctx(&self) -> &TypeContext {
        &self.ctx
    }

    /// Set the source location for subsequently emitted instructions.
    /// Pass `None` to clear.
    pub fn set_loc(&mut self, loc: Option<SourceLocation>) {
        self.current_loc = loc;
    }

    /// Get the current source location.
    pub fn current_loc(&self) -> Option<&SourceLocation> {
        self.current_loc.as_ref()
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
    /// 创建带参数的基本块，并把参数名绑定到参数 value。
    pub fn create_block_with_params(&mut self, params: &[(TypeId, &str)]) -> (Block, Vec<Value>) {
        let tys: Vec<TypeId> = params.iter().map(|(t, _)| *t).collect();
        let (block, values) = self.create_block_with_tys(&tys);
        for ((_, name), v) in params.iter().zip(values.iter()) {
            if !name.is_empty() {
                let name_id = self.ctx.borrow_mut().intern_str(name);
                self.func.value_names.insert(*v, name_id);
            }
        }
        (block, values)
    }

    /// 创建带参数类型的基本块（无参数名）。
    pub fn create_block_with_tys(&mut self, param_tys: &[TypeId]) -> (Block, Vec<Value>) {
        let (b, v) = self.func.dfg.make_block_with_params(param_tys);
        self.func.layout.push(b);
        (b, v)
    }
    pub fn switch_to_block(&mut self, block: Block) {
        self.cur_block = block;
        self.block_open = true;
    }
    pub fn current_block(&self) -> Block {
        self.cur_block
    }
    pub fn build_block<R, F>(&mut self, block: Block, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        let prev = self.cur_block;
        self.cur_block = block;
        self.block_open = true;
        let r = f(self);
        self.cur_block = prev;
        r
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
        assert!(self.block_open);
        let block = self.cur_block;
        let ops: SmallVec<[Value; 4]> = operands.iter().copied().collect();
        let imms: SmallVec<[Immediate; 4]> = immediates.iter().copied().collect();
        let inst = self.func.dfg.make_inst_with_meta_and_loc(
            opcode,
            block,
            ops.clone(),
            imms,
            result_tys,
            flags,
            MemFlags::NONE,
            SmallVec::new(),
            self.current_loc.clone(),
        );
        self.func.use_lists.record_inst(inst, &operands);
        self.func
            .dfg
            .inst_results(inst)
            .to_vec()
            .into_iter()
            .collect()
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
    fn value_type(&self, v: Value) -> TypeId {
        self.func
            .dfg
            .value_type(v)
            .unwrap_or_else(|| panic!("v{} no type", v.0))
    }

    pub fn iadd(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Iadd, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn isub(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Isub, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn imul(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Imul, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn udiv(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Udiv, vec![a, b], vec![], t, InstFlags::MAY_UB)
    }
    pub fn sdiv(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Sdiv, vec![a, b], vec![], t, InstFlags::MAY_UB)
    }
    pub fn urem(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Urem, vec![a, b], vec![], t, InstFlags::MAY_UB)
    }
    pub fn srem(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Srem, vec![a, b], vec![], t, InstFlags::MAY_UB)
    }
    pub fn band(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Band, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn bor(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Bor, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn bxor(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Bxor, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn bnot(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Bnot, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn ishl(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Ishl, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn ushr(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Ushr, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn sshr(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Sshr, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn icmp(&mut self, cond: IntCC, a: Value, b: Value) -> Value {
        self.emit1(
            Opcode::Icmp { cond },
            vec![a, b],
            vec![],
            self.ctx.bool_ty(),
            InstFlags::NONE,
        )
    }
    pub fn fcmp(&mut self, cond: FloatCC, a: Value, b: Value) -> Value {
        self.emit1(
            Opcode::Fcmp { cond },
            vec![a, b],
            vec![],
            self.ctx.bool_ty(),
            InstFlags::NONE,
        )
    }
    pub fn fadd(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fadd, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn fsub(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fsub, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn fmul(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fmul, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn fdiv(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fdiv, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn fneg(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fneg, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn fabs(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fabs, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn fsqrt(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fsqrt, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn fadd_fast(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fadd, vec![a, b], vec![], t, InstFlags::FMF_FAST)
    }
    pub fn fadd_with_flags(&mut self, a: Value, b: Value, flags: InstFlags) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fadd, vec![a, b], vec![], t, flags)
    }
    pub fn stack_addr(&mut self, offset: i32) -> Value {
        self.emit1(
            Opcode::StackAddr,
            vec![],
            vec![Immediate::Int(offset as i64)],
            self.ctx.ptr_ty(),
            InstFlags::NONE,
        )
    }
    pub fn global_addr(&mut self, global: GlobalId) -> Value {
        self.emit1(
            Opcode::GlobalAddr,
            vec![],
            vec![Immediate::Global(global)],
            self.ctx.ptr_ty(),
            InstFlags::NONE,
        )
    }
    pub fn alloca(&mut self, ty: TypeId, count: u32) -> Value {
        self.emit1(
            Opcode::Alloca,
            vec![],
            vec![Immediate::Type(ty), Immediate::Uint(count as u64)],
            self.ctx.ptr_ty(),
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
            self.ctx.ptr_ty(),
            InstFlags::NONE,
        )
    }
    pub fn load(&mut self, addr: Value, ty: TypeId) -> Value {
        self.emit1(Opcode::Load, vec![addr], vec![], ty, InstFlags::NONE)
    }
    pub fn load_with_flags(&mut self, addr: Value, ty: TypeId, mem_flags: MemFlags) -> Value {
        let block = self.cur_block;
        let ops: SmallVec<[Value; 4]> = smallvec::smallvec![addr];
        let operands_vec = vec![addr];
        let inst = self.func.dfg.make_inst_with_meta_and_loc(
            Opcode::Load,
            block,
            ops,
            SmallVec::new(),
            &[ty],
            InstFlags::NONE,
            mem_flags,
            SmallVec::new(),
            self.current_loc.clone(),
        );
        self.func.use_lists.record_inst(inst, &operands_vec);
        self.func.dfg.inst_results(inst)[0]
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
    pub fn store_with_flags(&mut self, addr: Value, value: Value, mem_flags: MemFlags) {
        let block = self.cur_block;
        let ops: SmallVec<[Value; 4]> = smallvec::smallvec![value, addr];
        let operands_vec = vec![value, addr];
        let inst = self.func.dfg.make_inst_with_meta_and_loc(
            Opcode::Store,
            block,
            ops,
            SmallVec::new(),
            &[],
            InstFlags::SIDE_EFFECT,
            mem_flags,
            SmallVec::new(),
            self.current_loc.clone(),
        );
        self.func.use_lists.record_inst(inst, &operands_vec);
    }

    // --- 向量/SIMD 操作 ---

    pub fn vadd(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Vadd, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn vsub(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Vsub, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn vmul(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Vmul, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn vextract(&mut self, vec: Value, index: u32) -> Value {
        let t = self.value_type(vec);
        self.emit1(
            Opcode::Vextract,
            vec![vec],
            vec![Immediate::Uint(index as u64)],
            t,
            InstFlags::NONE,
        )
    }
    pub fn vinsert(&mut self, vec: Value, elem: Value, index: u32) -> Value {
        let t = self.value_type(vec);
        self.emit1(
            Opcode::Vinsert,
            vec![vec, elem],
            vec![Immediate::Uint(index as u64)],
            t,
            InstFlags::NONE,
        )
    }
    pub fn shuffle_vector(&mut self, a: Value, b: Value, mask: &[u32]) -> Value {
        let t = self.value_type(a);
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
        let t = self.value_type(val);
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
    ) -> Value {
        let t = self.value_type(cmp);
        // Cmpxchg returns a 2-element struct { old_value, success_flag }
        self.emit1(
            Opcode::Cmpxchg,
            vec![ptr, cmp, new],
            vec![
                Immediate::Uint(ordering_success as u64),
                Immediate::Uint(ordering_failure as u64),
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
        // Determine result type from the aggregate type — use a default for now
        self.emit1(
            Opcode::ExtractValue,
            vec![agg],
            vec![Immediate::Uint(field_idx as u64)],
            TypeId::VOID, // caller should know the result type
            InstFlags::NONE,
        )
    }
    pub fn insert_value(&mut self, agg: Value, elem: Value, field_idx: u32) -> Value {
        let t = self.value_type(agg);
        self.emit1(
            Opcode::InsertValue,
            vec![agg, elem],
            vec![Immediate::Uint(field_idx as u64)],
            t,
            InstFlags::NONE,
        )
    }
    pub fn iconst(&mut self, value: i64, ty: TypeId) -> Value {
        let bits = ty.bits().max(1) as u16; // use type's actual bit width, min 1
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

    pub fn fconst_f32(&mut self, v: f32) -> Value {
        self.fconst(v.to_bits() as u64, self.ctx.f32_ty())
    }

    pub fn fconst_f64(&mut self, v: f64) -> Value {
        self.fconst(v.to_bits(), self.ctx.f64_ty())
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
    pub fn copy(&mut self, v: Value) -> Value {
        let t = self.value_type(v);
        self.emit1(Opcode::Copy, vec![v], vec![], t, InstFlags::NONE)
    }
    pub fn select(&mut self, cond: Value, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Select, vec![cond, a, b], vec![], t, InstFlags::NONE)
    }
    pub fn freeze(&mut self, v: Value) -> Value {
        let t = self.value_type(v);
        self.emit1(Opcode::Freeze, vec![v], vec![], t, InstFlags::NONE)
    }

    // --- Bit manipulation (6) ---

    pub fn clz(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Clz, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn ctz(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Ctz, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn popcnt(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Popcnt, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn bitreverse(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Bitreverse, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn rotl(&mut self, a: Value, amount: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Rotl, vec![a, amount], vec![], t, InstFlags::NONE)
    }
    pub fn rotr(&mut self, a: Value, amount: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Rotr, vec![a, amount], vec![], t, InstFlags::NONE)
    }

    // --- Integer extended (10) ---

    pub fn abs(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Abs, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn smin(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Smin, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn smax(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Smax, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn umin(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Umin, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn umax(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Umax, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn sadd_sat(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::SaddSat, vec![a, b], vec![], t, InstFlags::MAY_UB)
    }
    pub fn ssub_sat(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::SsubSat, vec![a, b], vec![], t, InstFlags::MAY_UB)
    }
    pub fn uadd_sat(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::UaddSat, vec![a, b], vec![], t, InstFlags::MAY_UB)
    }
    pub fn usub_sat(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::UsubSat, vec![a, b], vec![], t, InstFlags::MAY_UB)
    }
    pub fn bswap(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Bswap, vec![a], vec![], t, InstFlags::NONE)
    }

    // --- Float extended (8) ---

    pub fn fma(&mut self, a: Value, b: Value, c: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fma, vec![a, b, c], vec![], t, InstFlags::NONE)
    }
    pub fn fmin(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fmin, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn fmax(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fmax, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn fcopysign(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fcopysign, vec![a, b], vec![], t, InstFlags::NONE)
    }
    pub fn ffloor(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Ffloor, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn fceil(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fceil, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn ftrunc(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Ftrunc, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn fround(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Fround, vec![a], vec![], t, InstFlags::NONE)
    }

    // --- Overflow arithmetic (6) — returns (value, overflow_flag: i1) ---

    pub fn sadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let t = self.value_type(a);
        let results = self.emit(
            Opcode::SaddOverflow,
            vec![a, b],
            vec![],
            &[t, self.ctx.bool_ty()],
            InstFlags::MAY_UB,
        );
        (results[0], results[1])
    }
    pub fn uadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let t = self.value_type(a);
        let results = self.emit(
            Opcode::UaddOverflow,
            vec![a, b],
            vec![],
            &[t, self.ctx.bool_ty()],
            InstFlags::MAY_UB,
        );
        (results[0], results[1])
    }
    pub fn ssub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let t = self.value_type(a);
        let results = self.emit(
            Opcode::SsubOverflow,
            vec![a, b],
            vec![],
            &[t, self.ctx.bool_ty()],
            InstFlags::MAY_UB,
        );
        (results[0], results[1])
    }
    pub fn usub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let t = self.value_type(a);
        let results = self.emit(
            Opcode::UsubOverflow,
            vec![a, b],
            vec![],
            &[t, self.ctx.bool_ty()],
            InstFlags::MAY_UB,
        );
        (results[0], results[1])
    }
    pub fn smul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let t = self.value_type(a);
        let results = self.emit(
            Opcode::SmulOverflow,
            vec![a, b],
            vec![],
            &[t, self.ctx.bool_ty()],
            InstFlags::MAY_UB,
        );
        (results[0], results[1])
    }
    pub fn umul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let t = self.value_type(a);
        let results = self.emit(
            Opcode::UmulOverflow,
            vec![a, b],
            vec![],
            &[t, self.ctx.bool_ty()],
            InstFlags::MAY_UB,
        );
        (results[0], results[1])
    }

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
        let t = self.value_type(a);
        self.emit1(Opcode::Vdiv, vec![a, b], vec![], t, InstFlags::MAY_UB)
    }
    pub fn vneg(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.emit1(Opcode::Vneg, vec![a], vec![], t, InstFlags::NONE)
    }
    pub fn vabs(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
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

    pub fn finish(self) -> &'f mut Function {
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
    fn test_simple() {
        let sig = FunctionSignature::new(&[], &[TypeId::I32]);
        let mut fb = FunctionBuilder::new("test", TypeContext::new(), sig);
        fb.create_block_here();
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let func = fb.finish();
        assert_eq!(func.dfg.block_count(), 1);
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
        let mp = fb.irb(merge_blk).block_params(merge_blk);
        fb.ret(&[mp[0]]);
        let func = fb.finish();
        assert_eq!(func.dfg.block_count(), 4);
    }
}
