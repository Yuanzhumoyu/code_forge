//! MIR → code-forge IR 降级（手写 lowering 核心）。
//!
//! 入口：`LowerCtxt::new` + `lower_body`。方法按职责拆分到子模块：
//! `statement`/`rvalue`/`place`/`vtable`/`const_eval`（同类型多 impl 块）。

pub(crate) use crate::error::ForgeError;
pub(crate) use crate::func_ref::FuncRefTable;
pub(crate) use crate::layout::{
    is_agg_mem, is_scalar_pair_abi, layout_bytes, layout_size, scalar_pair_offsets,
};
use crate::prelude::*;
pub(crate) use crate::rustc_compat::substs_first_ty;
pub(crate) use crate::types::{is_vector_abi, map_type};

pub(crate) mod const_eval;
pub(crate) mod intrinsics;
pub(crate) mod place;
pub(crate) mod rvalue;
pub(crate) mod statement;
pub(crate) mod vtable;

pub(crate) struct LocalSlot {
    /// 相对 rbp 的负偏移（-8, -16, ...，8 字节对齐）。
    offset: i32,
    /// 槽的 codegen 类型（bool 保持 BOOL，后端 opsize 按 4 字节存取）。
    ty: TypeId,
}

pub(crate) struct LowerCtxt<'tcx, 'f> {
    pub(crate) tcx: TyCtxt<'tcx>,
    pub(crate) body: &'tcx Body<'tcx>,
    /// 函数显示名(def_path_str),FORGE_TRACE_FN 诊断用。
    pub(crate) fn_name: String,
    pub(crate) builder: FunctionBuilder,
    /// MIR local → 栈槽。SSA 寄存器模型没有 phi，无法处理循环回边重定义，
    /// 因此所有局部变量都落在栈上，读写走 load/store。
    pub(crate) locals: HashMap<mir::Local, LocalSlot>,
    pub(crate) blocks: HashMap<mir::BasicBlock, Block>,
    /// 跨函数共享的 FuncRef 表（直接调用的重定位符号映射）。
    pub(crate) func_refs: &'f mut FuncRefTable,
    /// sret（>16 字节聚合返回）：隐藏 ret 指针参数（RCX），Return 时写 ret 缓冲。
    pub(crate) sret_ptr: Option<Value>,
    /// 聚合常量实参（Layout 等非 ScalarPair）的临时槽偏移：调用前写字节、
    /// 被调方入口复制后废弃。每函数一个（嵌套调用各用各的帧，不冲突）。
    pub(crate) temp_arg_off: Option<i32>,
}

impl<'tcx, 'f> LowerCtxt<'tcx, 'f> {
    pub(crate) fn new(
        tcx: TyCtxt<'tcx>,
        instance: &Instance<'tcx>,
        body: &'tcx Body<'tcx>,
        func_refs: &'f mut FuncRefTable,
    ) -> Self {
        let sig = build_signature(tcx, instance, body);
        // 函数名（display 用）：闭包/内部 shim 的 DefId 无 item_name（ICE），
        // 用 def_path_str 安全获取（如 `crate::main::{{closure}}`）
        let name = tcx.def_path_str(instance.def_id());
        // 为每个非零大小 local 分配栈槽（负数偏移，按 layout 大小对齐到 8）。
        // 槽覆盖全部 local（含返回槽 _0 与参数），循环回边后重新 load
        // 保证重定义可见；聚合类型（结构体/元组/数组）的槽按真实大小分配。
        let mut locals = HashMap::new();
        let mut next_offset = 0i32;
        for (i, local_decl) in body.local_decls.iter().enumerate() {
            let local = mir::Local::from_usize(i);
            let ty = map_type(local_decl.ty, tcx).unwrap_or(TypeId::I32);
            if ty == TypeId::VOID {
                // unit/never 类型（如 `?` 的 Err 分支、fmt 的 () 返回值）：MIR 仍可能
                // 经 place_addr/load/store 引用（probe7 的 `?` 链曾 ICE：
                // `self.locals[&place.local]` no entry found for key）。分配 8 字节
                // 占位槽保证索引安全；值读写由 load_place/store_place 的 VOID
                // 短路处理（load 返回 0、store no-op），不产生实际内存访问。
                next_offset -= 8;
                if crate::trace::trace_enabled("SLOT") {
                    eprintln!(
                        "[forge] slot _{} size=8 offset={} ty={} (void placeholder)",
                        i, next_offset, local_decl.ty
                    );
                }
                locals.insert(
                    local,
                    LocalSlot {
                        offset: next_offset,
                        ty,
                    },
                );
                continue;
            }
            let size = layout_size(tcx, local_decl.ty);
            next_offset -= size as i32;
            if crate::trace::trace_enabled("SLOT") {
                eprintln!(
                    "[forge] slot _{} size={} offset={} ty={}",
                    i, size, next_offset, local_decl.ty
                );
            }
            locals.insert(
                local,
                LocalSlot {
                    offset: next_offset,
                    ty,
                },
            );
        }
        // 聚合常量实参临时槽（32 字节，覆盖最大 16 字节聚合 + 对齐）
        next_offset -= 32;
        let temp_arg_off = Some(next_offset);
        LowerCtxt {
            tcx,
            body,
            fn_name: name.clone(),
            builder: FunctionBuilder::new(name.as_str(), TypeContext::new(), sig),
            locals,
            blocks: HashMap::new(),
            func_refs,
            sret_ptr: None,
            temp_arg_off,
        }
    }
    pub(crate) fn lower_body(mut self, body: &Body<'tcx>) -> Result<Function, ForgeError> {
        if crate::trace::trace_enabled("FN") {
            eprintln!("[forge] === fn: {}", self.fn_name);
        }
        if crate::trace::trace_enabled("MIR") {
            eprintln!("[forge] MIR body dump:\n{body:?}");
        }
        // B3 门控：向量类型（V64/V128/V256）参与跨函数 ABI（参数/返回）
        // 依赖主库向量 ABI（ymm-abi-plan 的 S1-S5）——当前主库 Call 返回
        // 只发 RAX 标量（V128 高 64 位丢失 → 静默错值）。未就绪前编译期
        // 拒绝，绝不产出静默错误结果（失败即报错原则）。
        {
            let mut vec_abi = None;
            if let Ok(ret_t) = map_type(body.return_ty(), self.tcx)
                && is_vector_abi(ret_t)
            {
                vec_abi = Some(format!("return {}", body.return_ty()));
            }
            if vec_abi.is_none() {
                for local in body.args_iter() {
                    let a_ty = body.local_decls[local].ty;
                    if layout_bytes(self.tcx, a_ty) > 0
                        && let Ok(t) = map_type(a_ty, self.tcx)
                        && is_vector_abi(t)
                    {
                        vec_abi = Some(format!("arg {a_ty}"));
                        break;
                    }
                }
            }
            if let Some(what) = vec_abi {
                return Err(ForgeError::Message(format!(
                    "{}: 向量 ABI（{what}）暂不支持——主库向量调用约定 \
                     （ymm-abi-plan S1-S5）未就绪；请改用标量传递",
                    self.fn_name
                )));
            }
        }
        // 1. 为入口块创建带参数（函数参数）的 block
        // 聚合参数（≤16 字节、2 标量——ScalarPair ABI）拆两个整数寄存器收参，
        // 否则 rustc 按 rcx/rdx 传值而我们只收 rcx（rdx 丢失 → 字段读 0/值当地址）
        let _ret_sz = layout_bytes(self.tcx, body.return_ty());
        let sret = is_agg_mem(self.tcx, body.return_ty());
        // P4.1：跳过 Ignore/ZST 参数（Global 等），与 build_signature/调用侧一致
        let mut entry_args: Vec<(TypeId, String)> = Vec::new();
        if sret {
            entry_args.push((TypeId::I64, "sret_ptr".to_string()));
        }
        entry_args.extend(
            body.args_iter()
                .filter(|local| layout_bytes(self.tcx, body.local_decls[*local].ty) > 0)
                .flat_map(|local| {
                    let ty = body.local_decls[local].ty;
                    let codegen_ty = map_type(ty, self.tcx).unwrap_or(TypeId::I32);
                    if is_scalar_pair_abi(self.tcx, ty) {
                        vec![
                            (TypeId::I64, format!("arg_{}_lo", local.index())),
                            (TypeId::I64, format!("arg_{}_hi", local.index())),
                        ]
                    } else if is_agg_mem(self.tcx, ty) {
                        // 16 字节非 ScalarPair 聚合参数：间接传递（&实参槽）
                        vec![(TypeId::I64, format!("arg_{}_ptr", local.index()))]
                    } else {
                        vec![(codegen_ty, format!("arg_{}", local.index()))]
                    }
                }),
        );

        let entry_bb = mir::BasicBlock::from_usize(0);
        let (entry_blk, entry_params) = self.builder.create_block_with_params(
            &entry_args
                .iter()
                .map(|(t, s)| (*t, s.as_str()))
                .collect::<Vec<_>>(),
        );
        self.blocks.insert(entry_bb, entry_blk);

        // 2. 为其他 MIR 基本块创建 block
        for (bb_idx, _) in body.basic_blocks.iter().enumerate().skip(1) {
            let bb = mir::BasicBlock::from_usize(bb_idx);
            if !self.blocks.contains_key(&bb) {
                let blk = self.builder.create_block();
                self.blocks.insert(bb, blk);
            }
        }

        // 3. 切换到入口块，把函数参数写入各自的栈槽
        self.builder.switch_to_block(entry_blk);
        let arg_locals: Vec<mir::Local> = body.args_iter().collect();
        let mut pi = 0;
        if sret {
            self.sret_ptr = Some(entry_params[0]);
            pi = 1;
        }
        // 3.1 槽零初始化：对非参数 local 的栈槽逐 8 字节写 0。消灭
        // "未初始化槽读垃圾"类崩溃（box_write 的 [rbp-0x68]=0/垃圾、Vec 的
        // ptr 槽残留）——未写槽的读取从"随机崩溃"变"确定性 0"，再配合
        // store 链修复根因；参数槽由收参写入（不需零初始化）。
        for local in body.local_decls.indices() {
            if local == mir::Local::from_usize(0) || body.args_iter().any(|a| a == local) {
                continue;
            }
            let ty = body.local_decls[local].ty;
            if map_type(ty, self.tcx).unwrap_or(TypeId::I32) == TypeId::VOID {
                continue;
            }
            if let Some(slot) = self.locals.get(&local) {
                let sz = layout_size(self.tcx, ty);
                let base = self.builder.stack_addr(slot.offset);
                let zero = self.builder.iconst(0, TypeId::I64);
                for k in (0..sz).step_by(8) {
                    let addr = if k == 0 {
                        base
                    } else {
                        let kv = self.builder.iconst(k as i64, TypeId::I64);
                        self.builder.iadd(base, kv)
                    };
                    self.builder.store(zero, addr);
                }
            }
        }
        for &local in arg_locals.iter() {
            if pi >= entry_params.len() {
                break;
            }
            let ty = body.local_decls[local].ty;
            // P4.6：跳过 ZST/Ignore 参数（RangeFull、Global 等）——与 entry_args
            // 构建的 layout>0 filter 对称。ZST 在有效参数之前时（如
            // <RangeFull as SliceIndex<[u8]>>::index(self: RangeFull, slice: &[u8])）
            // 若不跳过，lo(ptr) 被存进 ZST 槽、slice 只收到 hi(len) →
            // &[u8] 槽 = len 垃圾（vecfrom/slicelen 的 fat ptr 错位根因）。
            if layout_bytes(self.tcx, ty) == 0 {
                if crate::trace::trace_enabled("ARGS") {
                    eprintln!("[forge] entry skip ZST arg local={} ty={ty}", local.index());
                }
                continue;
            }
            if is_scalar_pair_abi(self.tcx, ty) && pi + 1 < entry_params.len() {
                // 聚合参数拆两个标量：按 pair 内存偏移写槽（重排后非固定 0/8）
                let slot = &self.locals[&local];
                let base = self.builder.stack_addr(slot.offset);
                self.pack_sp(base, entry_params[pi], entry_params[pi + 1], ty);
                pi += 2;
            } else if is_agg_mem(self.tcx, ty) {
                // 16 字节非 ScalarPair 聚合参数：间接传递——入口参数是
                // &实参槽 指针，解引用复制到本函数 local 槽。
                let slot = &self.locals[&local];
                let sz = layout_bytes(self.tcx, ty);
                let base = self.builder.stack_addr(slot.offset);
                let p = entry_params[pi];
                self.copy_agg(base, p, sz);
                pi += 1;
            } else {
                self.store_local(local, entry_params[pi]);
                pi += 1;
            }
        }

        // 4. 翻译基本块
        for (bb_idx, bb_data) in body.basic_blocks.iter().enumerate() {
            let bb = mir::BasicBlock::from_usize(bb_idx);
            let block_id = self.blocks[&bb];
            if crate::trace::trace_enabled("BLOCK") {
                eprintln!("[forge] lower block bb{bb_idx} -> {:?}", block_id);
            }
            self.builder.switch_to_block(block_id);

            for stmt in &bb_data.statements {
                // A4：错误增强——附函数名 + bb 序号 + 语句 Debug（否则深层
                // lowering 的裸错误无法定位到具体 MIR 语句）。
                self.lower_statement(stmt).map_err(|e| {
                    ForgeError::Message(format!(
                        "{} bb{bb_idx}: {} [stmt: {stmt:?}]",
                        self.fn_name, e
                    ))
                })?;
            }

            if crate::trace::trace_enabled("TERM") {
                eprintln!(
                    "[forge] term in bb{}: {:?}",
                    block_id.0,
                    bb_data.terminator().kind
                );
            }
            match &bb_data.terminator().kind {
                TerminatorKind::Return => {
                    let ret_local = mir::Local::from_usize(0);
                    if self.locals.contains_key(&ret_local) {
                        let ret_ty = body.local_decls[ret_local].ty;
                        let sz = layout_bytes(self.tcx, ret_ty);
                        let slot = &self.locals[&ret_local];
                        let base = self.builder.stack_addr(slot.offset);
                        // sret（≥16 字节非 ScalarPair 聚合）：写 ret 缓冲（复制
                        // 全部字节）+ void 返回
                        if let Some(rp) = self.sret_ptr
                            && is_agg_mem(self.tcx, ret_ty)
                        {
                            self.copy_agg(rp, base, sz);
                            self.builder.ret(&[]);
                        } else if is_scalar_pair_abi(self.tcx, ret_ty) {
                            let (lo, hi) = self.unpack_sp(base, ret_ty);
                            self.builder.ret(&[lo, hi]);
                        } else {
                            let val = self.load_local(ret_local);
                            self.builder.ret(&[val]);
                        }
                    } else {
                        self.builder.ret(&[]);
                    }
                }
                TerminatorKind::Goto { target } => {
                    let tgt = self.blocks[target];
                    self.builder.jump(tgt, &[]);
                }
                TerminatorKind::SwitchInt { discr, targets } => {
                    let discr_val = self.lower_operand(discr)?;
                    let otherwise = targets.otherwise();
                    let targets_vec: Vec<_> = targets.iter().collect();

                    if targets_vec.len() == 1 && targets_vec[0].0 == 0 {
                        // 布尔条件：switchInt(_1) -> [0: false_bb, otherwise: true_bb]
                        let false_blk = self.blocks[&targets_vec[0].1];
                        let true_blk = self.blocks[&otherwise];
                        self.builder
                            .branch(discr_val, true_blk, &[], false_blk, &[]);
                    } else {
                        // 通用 case：if-else 链。每个 case 独立重新 lower discr 与
                        // 常量（等价手工分支链）——discr_val 若跨比较块复用同一
                        // XReg，其长活区间与多个 case 常量的 XReg 在 regalloc 中
                        // 冲突（常量 XReg 重叠分配 → 运行时比较错乱）。
                        let mut cur_blk = block_id;
                        for &(value, tgt_bb) in &targets_vec {
                            self.builder.switch_to_block(cur_blk);
                            let d = self.lower_operand(discr)?;
                            let const_val = self.builder.iconst_i32(value as i32);
                            let eq = self.builder.icmp(IntCC::Equal, d, const_val);
                            let tgt_blk = self.blocks[&tgt_bb];
                            let next_blk = self.builder.create_block();
                            // create_block() auto-switches cur_block, switch back
                            self.builder.switch_to_block(cur_blk);
                            self.builder.branch(eq, tgt_blk, &[], next_blk, &[]);
                            cur_blk = next_blk;
                        }
                        let otherwise_blk = self.blocks[&otherwise];
                        self.builder.switch_to_block(cur_blk);
                        self.builder.jump(otherwise_blk, &[]);
                    }
                }
                TerminatorKind::Call {
                    func,
                    args,
                    destination,
                    target,
                    ..
                } => {
                    let dest_ty = body.local_decls[destination.local].ty;
                    let _dest_sz = layout_bytes(self.tcx, dest_ty);
                    if crate::trace::trace_enabled("CALL") {
                        let callee = match func {
                            Operand::Constant(c) => format!("{}", c.const_.ty()),
                            _ => "indirect".to_string(),
                        };
                        eprintln!(
                            "[forge] CALL {} dest={} dest_ty={} args={}",
                            callee,
                            destination.local.index(),
                            dest_ty,
                            args.len()
                        );
                    }
                    // 聚合参数（≤16 字节、2 标量）调用方按 rcx/rdx 两个寄存器传值
                    //（与被调方 entry 参数拆包一致；否则 rdx 传垃圾 → 字段读错/解引用崩）
                    let mut call_args: Vec<Value> = Vec::new();
                    if is_agg_mem(self.tcx, dest_ty) {
                        // sret：第 0 参数传 ret 缓冲指针（&destination 槽）
                        let slot = &self.locals[&destination.local];
                        call_args.push(self.builder.stack_addr(slot.offset));
                    }
                    // P4.1：跳过 Ignore/ZST 参数——Global 等零大小类型不占参数槽，
                    // 传了会导致与 LLVM 侧实例错位（to_vec 的 alloc 参数，断言
                    // expects 3 / packs 4）。ZST（size==0）判定与 rustc FnAbi 的
                    // Ignore mode 务实一致（打包循环先于 callee instance 解析，
                    // 无法查 FnAbi）。
                    for (arg_i, arg) in args.iter().enumerate() {
                        let a_ty = arg.node.ty(&self.body.local_decls, self.tcx);
                        if layout_bytes(self.tcx, a_ty) == 0 {
                            if crate::trace::trace_enabled("CALL") {
                                eprintln!("[forge]   arg[{arg_i}] {a_ty} ZST 跳过");
                            }
                            continue;
                        }
                        if crate::trace::trace_enabled("CALL") {
                            let kind = match &arg.node {
                                Operand::Move(p) => format!("move({:?})", p),
                                Operand::Copy(p) => format!("copy({:?})", p),
                                Operand::Constant(c) => format!("const({})", c.const_.ty()),
                                Operand::RuntimeChecks(_) => "runtime_checks".to_string(),
                            };
                            eprintln!(
                                "[forge]   arg {a_ty} sp={} mem={} op={}",
                                is_scalar_pair_abi(self.tcx, a_ty),
                                is_agg_mem(self.tcx, a_ty),
                                kind
                            );
                        }
                        // &str/&[T] 字面量实参（ConstValue::Slice）：ptr = rodata
                        // 地址（global_addr + intern_promoted 落盘）、meta = len。
                        // 拆 lo(ptr) / hi(len) 两个标量传（eval_const_bytes 对
                        // Slice 返回 None，退化 0 会传空指针）。
                        if let Operand::Constant(ct) = &arg.node
                            && let rustc_middle::mir::Const::Val(
                                rustc_middle::mir::ConstValue::Slice { alloc_id, meta },
                                _,
                            ) = ct.const_
                        {
                            let g = if let rustc_middle::mir::interpret::GlobalAlloc::Memory(
                                alloc,
                            ) = self.tcx.global_alloc(alloc_id)
                            {
                                let inner = &*alloc.0;
                                let size = inner.size().bytes_usize();
                                let bytes = inner
                                    .inspect_with_uninit_and_ptr_outside_interpreter(0..size)
                                    .to_vec();
                                let align = inner.align.bytes();
                                let sym = self.slice_sym(alloc_id);
                                self.func_refs.intern_promoted(alloc_id, &sym, bytes, align)
                            } else {
                                let sym = self.slice_sym(alloc_id);
                                self.func_refs.intern_global(alloc_id, &sym)
                            };
                            call_args.push(self.builder.global_addr(GlobalId(g)));
                            call_args.push(self.builder.iconst(meta as i64, TypeId::I64));
                            continue;
                        }
                        // ScalarPair 聚合常量实参（如 Layout 常量
                        // <i32 as SizedTypeProperties>::LAYOUT）：eval 字节拆
                        // lo/hi 两个标量传（lower_operand 的 const 分支只处理
                        // 标量，聚合常量会退化为 0）。
                        if let Operand::Constant(ct) = &arg.node {
                            let a_sz = layout_bytes(self.tcx, a_ty);
                            if a_sz > 8
                                && is_scalar_pair_abi(self.tcx, a_ty)
                                && let Some(bytes) = self.eval_const_bytes(ct.const_)
                            {
                                let lo = u64::from_le_bytes(
                                    bytes.get(..8).unwrap_or(&[0; 8]).try_into().unwrap(),
                                );
                                let hi = u64::from_le_bytes(
                                    bytes.get(8..16).unwrap_or(&[0; 8]).try_into().unwrap(),
                                );
                                call_args.push(self.builder.iconst(lo as i64, TypeId::I64));
                                call_args.push(self.builder.iconst(hi as i64, TypeId::I64));
                                continue;
                            }
                        }
                        if is_scalar_pair_abi(self.tcx, a_ty)
                            && let Operand::Move(p) | Operand::Copy(p) = &arg.node
                        {
                            let base = self.place_addr(p);
                            let (lo, hi) = self.unpack_sp(base, a_ty);
                            call_args.push(lo);
                            call_args.push(hi);
                        } else if is_agg_mem(self.tcx, a_ty) {
                            // 16 字节非 ScalarPair 聚合参数：间接传递（&实参槽）
                            if let Operand::Move(p) | Operand::Copy(p) = &arg.node {
                                call_args.push(self.place_addr(p));
                            } else if let Operand::Constant(ct) = &arg.node
                                && let Some(bytes) = self.eval_const_bytes(ct.const_)
                            {
                                // const 聚合（如 LAYOUT）：eval 字节写临时槽后传地址
                                let off = self.temp_arg_off.unwrap_or(-32);
                                let base = self.builder.stack_addr(off);
                                self.agg_const_bytes(base, &bytes);
                                call_args.push(self.builder.stack_addr(off));
                            } else {
                                call_args.push(self.lower_operand(&arg.node)?);
                            }
                        } else {
                            call_args.push(self.lower_operand(&arg.node)?);
                        }
                    }
                    let args = call_args;

                    let ret_ty = map_type(dest_ty, self.tcx).unwrap_or(TypeId::VOID);

                    let ret_tys: Vec<TypeId> = if ret_ty == TypeId::VOID {
                        vec![]
                    } else if is_agg_mem(self.tcx, dest_ty) {
                        // sret（16 字节及以上非 ScalarPair 聚合）：void 返回
                        //（ret 缓冲由隐藏参数写入）
                        vec![]
                    } else if is_scalar_pair_abi(self.tcx, dest_ty) {
                        // ScalarPair（如 (i32,bool) 8 字节、16 字节结构体）返回：
                        // RAX + RDX 两个标量。判断用 backend_repr 而非字节区间——
                        // 8 字节的 (value, bool) checked 结果也是 ScalarPair。
                        vec![TypeId::I64, TypeId::I64]
                    } else {
                        vec![ret_ty]
                    };

                    let results: Vec<Value> = if let Operand::Constant(constant) = func {
                        let fty = constant.const_.ty();
                        if let ty::TyKind::FnDef(def_id, substs) = fty.kind() {
                            // intrinsics（size_of_val/align_of_val/ctpop/transmute 等）
                            // 在单态化实例中内联处理，不生成外部调用
                            if let Some(intr) = self.tcx.intrinsic(*def_id) {
                                let name = intr.name.as_str();
                                self.lower_intrinsic(name, args, substs, fty, block_id)?
                            } else if self
                                .tcx
                                .is_lang_item(*def_id, rustc_hir::LangItem::DropGlue)
                                && !substs_first_ty(&substs)
                                    .unwrap_or_else(|| self.tcx.types.unit)
                                    .needs_drop(self.tcx, ty::TypingEnv::fully_monomorphized())
                            {
                                // drop_in_place<T>：T 无需 drop 时为空操作（no-op），
                                // 否则需编译 drop glue（collect_instances 会收集）
                                vec![]
                            } else {
                                // 直接调用：解析 FnDef → instance → 符号名 → FuncRef 序号。
                                // 跨 crate 函数（core/alloc 泛型）在符号表注册后，链接器
                                // 与同一模块内编译出的该函数实例重定位一致。
                                // 优先 try_resolve 完整单态化（trait 方法等部分替换的
                                // substs 会导致 symbol_name_for_instance 断言 panic）
                                let resolved = ty::Instance::try_resolve(
                                    self.tcx,
                                    ty::TypingEnv::fully_monomorphized(),
                                    *def_id,
                                    substs.skip_binder(),
                                )
                                .ok()
                                .flatten();
                                if crate::trace::trace_enabled("SUBSTS") {
                                    eprintln!(
                                        "[forge] call {def_id:?} substs={substs:?} resolved={}",
                                        resolved.is_some()
                                    );
                                }
                                let instance = resolved.unwrap_or_else(|| {
                                    ty::Instance::new_raw(*def_id, substs.skip_binder())
                                });
                                // 虚方法（<dyn Trait as Trait>::method）：从 receiver
                                // 的 fat pointer 加载 vtable，按槽索引间接调用。
                                // vtable 布局：receiver 是 (数据指针, vtable 指针) 双字；
                                // vtable[0]=drop_in_place，trait 方法从 vtable[1] 起
                                // （rustc 的 InstanceKind::Virtual(def_id, idx)）。
                                if let ty::InstanceKind::Virtual(_, idx) = instance.def {
                                    // &dyn Trait 参数按 ScalarPair 传（data, vtable 两个
                                    // 值）——args[0]=数据指针、args[1]=vtable 指针。
                                    let vtable = args
                                        .get(1)
                                        .copied()
                                        .unwrap_or_else(|| self.builder.iconst(0, TypeId::I64));
                                    // vtable 槽：rustc 的 idx 是绝对索引（含
                                    // drop_in_place/size/align 公共槽偏移），直接用
                                    let slot_v = self.builder.iconst(idx as i64 * 8, TypeId::I64);
                                    let slot_addr = self.builder.iadd(vtable, slot_v);
                                    let fn_ptr = self.builder.load(slot_addr, TypeId::I64);
                                    // 方法只收 receiver（data 指针）——vtable 是 fat
                                    // pointer 元数据，不传给方法
                                    let method_args =
                                        args.first().copied().map(|a| vec![a]).unwrap_or_default();
                                    self.builder.call_indirect(fn_ptr, &method_args, &ret_tys)
                                } else {
                                    // P4.1：按 rustc FnAbi 补齐参数（track_caller
                                    // 隐藏 &Location 等），否则被调方收参错位。
                                    // A2：补齐后做 arg count 一致性校验（release
                                    // 也生效，FORGE_STRICT_ABI=0 可关闭）。
                                    let args = crate::abi::pad_call_args(
                                        self.tcx,
                                        &instance,
                                        args,
                                        || self.builder.iconst(0, TypeId::I64),
                                    );
                                    crate::abi::check_arg_count_consistency(
                                        self.tcx,
                                        &instance,
                                        args.len(),
                                        &format!("call in {}", self.fn_name),
                                    );
                                    let sym = symbol_name_for_instance(self.tcx, &instance);
                                    let fr = self.func_refs.intern(&sym);
                                    self.builder.call(FuncRef(fr), &args, &ret_tys)
                                }
                            }
                        } else {
                            let func_val = self.lower_operand(func)?;
                            self.builder.call_indirect(func_val, &args, &ret_tys)
                        }
                    } else {
                        let func_val = self.lower_operand(func)?;
                        self.builder.call_indirect(func_val, &args, &ret_tys)
                    };

                    if !results.is_empty() {
                        let dty = body.local_decls[destination.local].ty;
                        if is_scalar_pair_abi(self.tcx, dty) && results.len() >= 2 {
                            let slot = &self.locals[&destination.local];
                            let base = self.builder.stack_addr(slot.offset);
                            self.pack_sp(base, results[0], results[1], dty);
                        } else {
                            self.store_local(destination.local, results[0]);
                        }
                    }

                    if let Some(target_bb) = target {
                        let tgt = self.blocks[target_bb];
                        self.builder.jump(tgt, &[]);
                    } else {
                        // diverging call（被调函数 `-> !` 不返回）：后续不可达，
                        // 必须显式发 unreachable，否则当前块无终结符（finish 断言）
                        self.builder.unreachable();
                    }
                }
                TerminatorKind::Unreachable => {
                    self.builder.unreachable();
                }
                TerminatorKind::Assert {
                    cond,
                    expected,
                    target,
                    ..
                } => {
                    let cond_val = self.lower_operand(cond)?;
                    let fail_blk = self.builder.create_block();
                    if crate::trace::trace_enabled("ASSERT") {
                        eprintln!(
                            "[forge] assert in bb{} target=bb{} expected={} -> Block({:?}) cond={cond_val:?}",
                            block_id.0,
                            target.index(),
                            expected,
                            self.blocks.get(target).map(|b| b.0),
                        );
                    }
                    // 成功路径必须跳转 MIR 的 success target 块（blocks 已预建），
                    // 不能新建 merge 块——否则 target 块（含其全部语句，如数组
                    // 索引链）成为孤立块，执行路径跳过它，局部变量槽未写 → 0。
                    let succ_blk = Some(self.blocks[target]);
                    self.builder.switch_to_block(block_id);
                    if let Some(succ) = succ_blk {
                        if *expected {
                            self.builder.branch(cond_val, succ, &[], fail_blk, &[]);
                        } else {
                            self.builder.branch(cond_val, fail_blk, &[], succ, &[]);
                        }
                        // 失败路径：调用 panic_handler（rust_begin_unwind）——
                        // assert 失败走 panic 语义而非裸 ud2 abort；panic_handler
                        // 返回 !，其后 unreachable 兜底（finish 断言要求终结符）。
                        self.builder.switch_to_block(fail_blk);
                        if let Some(panic_def) = self.tcx.lang_items().panic_impl() {
                            let inst = ty::Instance::new_raw(panic_def, ty::GenericArgs::empty());
                            let sym = symbol_name_for_instance(self.tcx, &inst);
                            let fr = self.func_refs.intern(&sym);
                            // 占位 &PanicInfo（当前后端不构造 PanicInfo；panic_handler
                            // 若访问 info 需自备占位布局——e2e 用例的 handler 为 loop）
                            let zero = self.builder.iconst(0, TypeId::I64);
                            self.builder.call(FuncRef(fr), &[zero], &[]);
                        }
                        self.builder.unreachable();
                        // 成功路径：后续语句继续在 success 块（lower_body 按
                        // MIR 块序翻译，switch 到 succ 保持块内语句连续性）
                        self.builder.switch_to_block(succ);
                    } else {
                        // 无 target（如 never 路径）：失败/成功都 unreachable
                        if *expected {
                            self.builder.branch(cond_val, fail_blk, &[], fail_blk, &[]);
                        }
                        self.builder.switch_to_block(fail_blk);
                        self.builder.unreachable();
                    }
                }
                TerminatorKind::Yield { .. } => {
                    self.builder.unreachable();
                }
                // Drop terminator：作用域末的自动 drop。当前后端不生成 drop glue
                // 调用（显式 drop() 是 Call terminator，会正确走 drop glue shim）；
                // 这里只延续控制流到 target。若未来需要 Drop flag 语义（drop 后
                // 再访问的 UB 检查），在此补 drop_glue 调用。
                TerminatorKind::Drop { target, .. } => {
                    let tgt = self.blocks[target];
                    self.builder.jump(tgt, &[]);
                }
                _ => {
                    self.builder.unreachable();
                }
            }
        }

        self.builder.finish().map_err(ForgeError::from)
    }
    pub(crate) fn mono_symbol(&self, instance: &Instance<'tcx>) -> String {
        let def_id = instance.def_id();
        let instantiating_crate = if def_id.is_local() {
            rustc_hir::def_id::LOCAL_CRATE
        } else {
            def_id.krate
        };
        rustc_symbol_mangling::symbol_name_for_instance_in_crate(
            self.tcx,
            *instance,
            instantiating_crate,
        )
    }
    pub(crate) fn field_offset(&self, ty: Ty<'tcx>, idx: usize) -> i64 {
        if let ty::TyKind::Adt(def, _) = ty.kind()
            && def.is_enum()
        {
            let layout = match self.tcx.layout_of(ty::PseudoCanonicalInput {
                typing_env: ty::TypingEnv::fully_monomorphized(),
                value: ty,
            }) {
                Ok(l) => l,
                Err(e) => {
                    // 不 panic：报告后回退偏移 0（调用方按普通字段路径兜底）
                    self.tcx
                        .dcx()
                        .warn(format!("code-forge: layout_of enum failed for {ty}: {e:?}"));
                    return 0;
                }
            };
            let f = self.enum_data_fields(&layout);
            let r = f.get(idx).copied().unwrap_or(0);
            if crate::trace::trace_enabled("FIELD") {
                eprintln!("[forge] field enum {ty} idx={idx} -> {r} (data_fields={f:?})");
            }
            r
        } else {
            let l = self
                .tcx
                .layout_of(ty::PseudoCanonicalInput {
                    typing_env: ty::TypingEnv::fully_monomorphized(),
                    value: ty,
                })
                .ok();
            if crate::trace::trace_enabled("FIELD")
                && let Some(l) = &l
            {
                let f = l.layout.fields();
                let mut offs = Vec::new();
                for i in 0..f.count() {
                    offs.push(f.offset(i).bytes());
                }
                eprintln!(
                    "[forge] field {ty} idx={idx} offsets={offs:?} size={}",
                    l.layout.size().bytes()
                );
            }
            // FieldsShape::Primitive（标量）无字段：offset() 会 panic
            // （"`Primitive`s have no fields" ICE——嵌套枚举投影如
            // CF::Break(Err(1u8)) 的 `.0` 对 payload 标量触发）。标量
            // 的"字段"偏移恒为 0。
            l.map(|l| match l.layout.fields() {
                rustc_abi::FieldsShape::Primitive => 0,
                f => f.offset(idx).bytes() as i64,
            })
            .unwrap_or(0)
        }
    }
    pub(crate) fn field_ty(&self, ty: Ty<'tcx>, idx: usize) -> Ty<'tcx> {
        match ty.kind() {
            ty::TyKind::Adt(def, substs) if def.is_struct() || def.is_union() => {
                def.non_enum_variant().fields.raw[idx]
                    .ty(self.tcx, substs)
                    .skip_normalization()
            }
            // enum 的 Field 投影：返回 payload 字段类型（与 enum_data_fields 的
            // "第一个非空 variant" 一致）。若返回 enum 自身，嵌套投影
            // `((e as Some).0).0` 的第二步会再次按 enum 算偏移——niche enum
            // （payload@0）碰巧对，Memory enum（payload@8）偏移双加 → 读错槽
            // （Vec grow 链 ptr=0 根因）。
            // 无 variant 的 enum（不应出现）：报告后落到普通字段路径兜底。
            ty::TyKind::Adt(def, substs)
                if def.is_enum()
                    && let Some(v) = def
                        .variants()
                        .iter()
                        .find(|v| !v.fields.is_empty())
                        .or_else(|| def.variants().iter().next()) =>
            {
                v.fields.raw[idx].ty(self.tcx, substs).skip_normalization()
            }
            ty::TyKind::Tuple(tys) => tys[idx],
            ty::TyKind::Array(elem, _) => *elem,
            _ => ty,
        }
    }
    pub(crate) fn pointee_ty(&self, ty: Ty<'tcx>) -> Ty<'tcx> {
        match ty.kind() {
            ty::TyKind::Ref(_, inner, _) | ty::TyKind::RawPtr(inner, _) => *inner,
            _ => ty,
        }
    }
    pub(crate) fn elem_ty(&self, ty: Ty<'tcx>) -> Ty<'tcx> {
        match ty.kind() {
            ty::TyKind::Array(elem, _) | ty::TyKind::Slice(elem) => *elem,
            _ => ty,
        }
    }
}

fn build_signature<'tcx>(
    tcx: TyCtxt<'tcx>,
    _instance: &Instance<'tcx>,
    body: &Body<'tcx>,
) -> FunctionSignature {
    // 提取参数类型（P4.1：跳过 Ignore/ZST 参数 + ScalarPair 拆 2 / 聚合指针，
    // 与 lower_body 入口收参 / call 打包三处一致——否则 FunctionSignature 的
    // param_tys 与 entry block 参数数错位，review 应-fix）
    let params: Vec<(TypeId, String)> = body
        .args_iter()
        .filter(|local| layout_bytes(tcx, body.local_decls[*local].ty) > 0)
        .flat_map(|local| {
            let ty = body.local_decls[local].ty;
            if is_scalar_pair_abi(tcx, ty) {
                // ScalarPair 参数：两个整数寄存器（与入口收参/调用侧一致）
                vec![
                    (TypeId::I64, format!("arg_{}_lo", local.index())),
                    (TypeId::I64, format!("arg_{}_hi", local.index())),
                ]
            } else if is_agg_mem(tcx, ty) {
                // 16 字节非 ScalarPair 聚合参数：间接传递（&实参槽）
                vec![(TypeId::I64, format!("arg_{}_ptr", local.index()))]
            } else {
                let codegen_ty = map_type(ty, tcx).unwrap_or(TypeId::I32);
                vec![(codegen_ty, format!("arg_{}", local.index()))]
            }
        })
        .collect();

    // 提取返回值类型
    let return_ty = body.return_ty();
    // sret（≥16 字节非 ScalarPair 聚合返回）：调用约定为隐藏 ret 指针参数
    // （RCX）+ void 返回。判定与 lower_body 的入口收参严格一致（is_agg_mem），
    // 不能用 ret_sz > 16——16 字节非 ScalarPair 聚合（如 Result<Layout,
    // LayoutError>）在两条路径判定不一致会导致签名与入口错位。
    let sret = is_agg_mem(tcx, return_ty);
    let returns = if return_ty.is_unit() || return_ty.is_never() || sret {
        vec![]
    } else if is_scalar_pair_abi(tcx, return_ty) {
        // ScalarPair 返回（如 (i32,bool) 8 字节）：RAX + RDX 两个标量
        vec![TypeId::I64, TypeId::I64]
    } else {
        vec![map_type(return_ty, tcx).unwrap_or(TypeId::I32)]
    };
    let mut all_params = params.clone();
    if sret {
        all_params.insert(0, (TypeId::I64, "sret_ptr".to_string()));
    }

    FunctionSignature::new(
        &all_params
            .iter()
            .map(|(t, s)| (*t, s.as_str()))
            .collect::<Vec<_>>(),
        &returns,
    )
}

pub(crate) fn symbol_name_for_instance<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: &Instance<'tcx>,
) -> String {
    if crate::trace::trace_enabled("SYM") {
        eprintln!("[forge] sym for {:?}", instance.def_id());
    }
    let krate = instance.def_id().krate;
    rustc_symbol_mangling::symbol_name_for_instance_in_crate(tcx, *instance, krate).to_string()
}
