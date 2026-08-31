//! Statement lowering（含 terminator 分发）。
use super::*;

impl<'tcx, 'f> LowerCtxt<'tcx, 'f> {
    /// slice 常量（&str/&[T] 字面量）的 rodata 符号名：alloc_id 唯一。
    pub(crate) fn slice_sym(&self, alloc_id: rustc_middle::mir::interpret::AllocId) -> String {
        format!("{}::slice[{:?}]", self.fn_name, alloc_id)
    }
    pub(crate) fn lower_statement(
        &mut self,
        stmt: &rustc_middle::mir::Statement<'tcx>,
    ) -> Result<(), ForgeError> {
        match &stmt.kind {
            StatementKind::Assign(box (place, rvalue)) => {
                if crate::trace::trace_enabled("STMT") {
                    eprintln!("[forge] stmt: {:?} = {:?}", place, rvalue);
                }
                match rvalue {
                    Rvalue::Aggregate(kind, fields) => {
                        // 枚举 variant 构造：写判别（offset 0）+ variant 字段
                        if let mir::AggregateKind::Adt(_def, variant_idx, _, _, _) = **kind {
                            let ty = place.ty(&self.body.local_decls, self.tcx).ty;
                            if let ty::TyKind::Adt(enum_def, _) = ty.kind()
                                && enum_def.is_enum()
                            {
                                let base = self.place_addr(place);
                                // variant 字段：按 for_variant 布局写
                                let layout = self
                                    .tcx
                                    .layout_of(ty::PseudoCanonicalInput {
                                        typing_env: ty::TypingEnv::fully_monomorphized(),
                                        value: ty,
                                    })
                                    .map_err(|e| ForgeError::Message(format!("layout_of: {e}")))?;
                                // niche 枚举（判别嵌入 payload）：只写 payload 值（即判别），
                                // 不写独立判别（否则判别 1 覆盖值 8 且 payload 写槽外）。
                                // 判断依据是 tag_encoding（Niche）而非 backend_repr——
                                // fieldless 枚举（如 enum E{A,B,C}）backend_repr 也是
                                // Scalar 但判别是 Direct 独立存储，必须写判别值。
                                let is_niche = matches!(
                                    layout.layout.variants(),
                                    rustc_abi::Variants::Multiple {
                                        tag_encoding: rustc_abi::TagEncoding::Niche { .. },
                                        ..
                                    }
                                );
                                // Niche 变体（≠ untagged）：写 niche_value 到 tag 位置
                                // （rustc codegen_tag_value 的精确公式：
                                // niche_value = niche_start + (variant_index - niche_variants.start)，
                                // wrapping + 按 tag 宽度掩码）。untagged 变体不写（payload 值
                                // 自然≠niche——判别隐式）。cf5 的 CF::Break(Err(5u8)) 此前未写
                                // niche_value（is_niche 跳过）→ 判别区读到 0 → 误判 untagged。
                                if !is_niche {
                                    // 判别值（无 niche 枚举的判别在 offset 0）
                                    let discr = enum_def
                                        .discriminant_for_variant(self.tcx, variant_idx)
                                        .val;
                                    let dval = self.builder.iconst(discr as i64, TypeId::I32);
                                    self.builder.store(dval, base);
                                } else if let rustc_abi::Variants::Multiple {
                                    tag_encoding:
                                        rustc_abi::TagEncoding::Niche {
                                            untagged_variant,
                                            niche_variants,
                                            niche_start,
                                            ..
                                        },
                                    tag_field,
                                    ..
                                } = layout.layout.variants()
                                    && variant_idx != *untagged_variant
                                {
                                    let k = variant_idx.as_u32() - niche_variants.start.as_u32();
                                    let niche_value = (k as u128).wrapping_add(*niche_start);
                                    if crate::trace::trace_enabled("DISCR") {
                                        eprintln!(
                                            "[forge] Niche CONSTRUCT {ty} variant_idx={} untagged={} niche_variants={:?} start={} niche_value={niche_value}",
                                            variant_idx.index(),
                                            untagged_variant.index(),
                                            niche_variants
                                                .clone()
                                                .into_iter()
                                                .map(|v| v.index())
                                                .collect::<Vec<_>>(),
                                            niche_start,
                                        );
                                    }
                                    // 注：rustc 按 tag 宽度掩码（niche_layout.size.
                                    // unsigned_int_max）；forge 简化——niche_value
                                    // 通常为小值（u8 tag 场景 ≤255），I32 写入低
                                    // 字节即正确（高字节为 0）。
                                    let nv = self.builder.iconst(niche_value as i64, TypeId::I32);
                                    // tag 位置（通常 offset 0；通用按 tag_field 偏移）
                                    let tag_off =
                                        layout.layout.fields().offset(tag_field.index()).bytes()
                                            as i64;
                                    if tag_off == 0 {
                                        self.builder.store(nv, base);
                                    } else {
                                        let off_v = self.builder.iconst(tag_off, TypeId::I64);
                                        let tag_addr = self.builder.iadd(base, off_v);
                                        self.builder.store(nv, tag_addr);
                                    }
                                }
                                // 字段偏移 = variants[v].fields()（rustc 已含 tag 后位置）
                                let data_fields = self.enum_data_fields(&layout);
                                for (i, f) in fields.iter().enumerate() {
                                    let off = data_fields.get(i).copied().unwrap_or(0);
                                    let f_ty = f.ty(&self.body.local_decls, self.tcx);
                                    let f_sz = layout_bytes(self.tcx, f_ty);
                                    let faddr = if off == 0 {
                                        base
                                    } else {
                                        let off_v = self.builder.iconst(off, TypeId::I64);
                                        self.builder.iadd(base, off_v)
                                    };
                                    // 聚合 payload（如 Result::Ok(Layout) 的 16 字节
                                    // Layout）：按 size 整值复制（lower_operand 对
                                    // 聚合只 load 8 字节，payload 后半丢失 → 字段读 0）
                                    if f_sz > 8
                                        && matches!(
                                            f_ty.kind(),
                                            ty::TyKind::Adt(..) | ty::TyKind::Tuple(..)
                                        )
                                        && let Operand::Move(p) | Operand::Copy(p) = f
                                    {
                                        let src = self.place_addr(p);
                                        self.copy_agg(faddr, src, f_sz);
                                    } else {
                                        let fval = self.lower_operand(f)?;
                                        self.builder.store(fval, faddr);
                                    }
                                }
                                return Ok(());
                            }
                        }
                        // 结构体/元组/数组构造：逐字段写目标 place
                        let base = self.place_addr(place);
                        let ty = place.ty(&self.body.local_decls, self.tcx).ty;
                        for (i, f) in fields.iter().enumerate() {
                            let off = self.field_offset(ty, i);
                            if crate::trace::trace_enabled("AGG") {
                                let bt = self.builder.value_type(base);
                                eprintln!("[forge] agg {ty} i={i} off={off} base_ty={bt:?}");
                            }
                            let faddr = if off == 0 {
                                base
                            } else {
                                let off_v = self.builder.iconst(off, TypeId::I64);
                                // base 统一为 I64（PTR 的 iadd 在特定上下文 lowering 丢——绕开）
                                let base_i = self.builder.ireduce(base, TypeId::I64);
                                self.builder.iadd(base_i, off_v)
                            };
                            let f_ty = f.ty(&self.body.local_decls, self.tcx);
                            let f_sz = layout_bytes(self.tcx, f_ty);
                            // 嵌套聚合字段（如 Vec 的 buf/RawVec——16 字节、结构体
                            // 字段数组 [i32; 3]——12 字节）：按 size 整值复制，
                            // 否则只写 8 字节（ptr）→ cap/len 槽垃圾；数组字段
                            // 曾漏判（仅 Adt/Tuple）→ 第 3 元素丢失（struct_array
                            // field_sum 读 xs[2]=0 回归实证）。
                            if f_sz > 8
                                && matches!(
                                    f_ty.kind(),
                                    ty::TyKind::Adt(..)
                                        | ty::TyKind::Tuple(..)
                                        | ty::TyKind::Array(..)
                                )
                                && let Operand::Move(p) | Operand::Copy(p) = f
                            {
                                let src = self.place_addr(p);
                                self.copy_agg(faddr, src, f_sz);
                            } else {
                                let fval = self.lower_operand(f)?;
                                if crate::trace::trace_enabled("AGG") {
                                    eprintln!("[forge] agg store val={fval:?} addr={faddr:?}");
                                }
                                // B2：浮点/向量字段必须走 fstore/store_ty（XMM/向量
                                // 语义）——builder.store 是 GPR 语义，f32 字段会
                                // 把地址寄存器低 32 位写进槽（SIMD3 数组构造
                                // `[const 1f32,..]` 反汇编实证 movl %r15d,(%r15)）。
                                let f_codegen = map_type(f_ty, self.tcx).unwrap_or(TypeId::I32);
                                self.store_ty(fval, faddr, f_codegen);
                            }
                        }
                    }
                    Rvalue::Repeat(op, len) => {
                        // 数组 repeat 构造：同一值写 len 次
                        let base = self.place_addr(place);
                        let ty = place.ty(&self.body.local_decls, self.tcx).ty;
                        let elem_size = layout_bytes(self.tcx, self.elem_ty(ty));
                        let v = self.lower_operand(op)?;
                        let len = len.try_to_target_usize(self.tcx).unwrap_or(0);
                        for k in 0..len {
                            let off = (k * elem_size as u64) as i64;
                            let addr = if off == 0 {
                                base
                            } else {
                                let off_v = self.builder.iconst(off, TypeId::I64);
                                self.builder.iadd(base, off_v)
                            };
                            self.builder.store(v, addr);
                        }
                    }
                    _ => {
                        let ty = place.ty(&self.body.local_decls, self.tcx).ty;
                        let size = layout_bytes(self.tcx, ty);
                        if size > 8
                            && let Rvalue::Use(op, _) = rvalue
                            && let Operand::Move(src) | Operand::Copy(src) = op
                        {
                            // 聚合整值复制（memcpy）：按 size 从 src 逐 8 字节复制到 dst
                            let dst = self.place_addr(place);
                            let src = self.place_addr(src);
                            self.copy_agg(dst, src, size);
                        } else if size > 8
                            && let Rvalue::Cast(kind, op, _to_ty) = rvalue
                            && let Operand::Move(src) | Operand::Copy(src) = op
                        {
                            // unsize cast（&T → &dyn Trait / &T → &[T] 等）：
                            // 目标是 fat pointer（16 字节 ScalarPair：data + vtable/len）。
                            // 当源 place 也是 ScalarPair（16 字节，复制已有 fat pointer）
                            // 时按 size 整值复制（lo/hi 都复制）；源是瘦指针（&T，8 字节）
                            // 时生成 vtable 数据段并写 fat pointer（lo=源指针, hi=vtable
                            // 地址）——间接调用读 vtable 槽不再崩溃。
                            let src_ty = src.ty(&self.body.local_decls, self.tcx).ty;
                            if layout_bytes(self.tcx, src_ty) == size {
                                let dst = self.place_addr(place);
                                let src = self.place_addr(src);
                                self.copy_agg(dst, src, size);
                            } else if let mir::CastKind::PointerCoercion(
                                ty::adjustment::PointerCoercion::Unsize,
                                _,
                            ) = *kind
                            {
                                let to_ty = *_to_ty;
                                // 只对 dyn Trait（fat pointer 需 vtable）生成；&[T] 等
                                // 切片（len 元数据）由 lower_rvalue 的 Cast 分支处理
                                let dyn_ty = match to_ty.kind() {
                                    ty::TyKind::Ref(_, pointee, _) => *pointee,
                                    _ => to_ty,
                                };
                                if matches!(dyn_ty.kind(), ty::TyKind::Dynamic(..)) {
                                    let lo = self.load_place(&src.clone())?;
                                    // vtable 的 self 类型是具体类型（非引用）：
                                    // &Dog → Dog，否则 vtable 方法实例解析失败
                                    let src_concrete = match src_ty.kind() {
                                        ty::TyKind::Ref(_, t, _) => *t,
                                        _ => src_ty,
                                    };
                                    let g = self.gen_vtable(src_concrete, dyn_ty)?;
                                    let hi = self.builder.global_addr(GlobalId(g));
                                    let dst = self.place_addr(place);
                                    self.builder.store(lo, dst);
                                    let eight = self.builder.iconst(8, TypeId::I64);
                                    let hi_addr = self.builder.iadd(dst, eight);
                                    self.builder.store(hi, hi_addr);
                                } else if matches!(dyn_ty.kind(), ty::TyKind::Slice(_)) {
                                    // &[T; N] → &[T]：构造 fat pointer。
                                    // lo = 源数组指针、hi = N（数组长度，编译期常量）。
                                    // 否则 len 槽不写 → 收参方 ScalarPair 拆 hi 读到
                                    // 垃圾/0（vecfrom：Vec::from(&[1,2,3][..]) len=0）。
                                    let lo = self.load_place(&src.clone())?;
                                    let src_pointee = match src_ty.kind() {
                                        ty::TyKind::Ref(_, t, _) => *t,
                                        _ => src_ty,
                                    };
                                    let n = match src_pointee.kind() {
                                        rustc_middle::ty::TyKind::Array(_, len) => len
                                            .try_to_target_usize(self.tcx)
                                            .unwrap_or(0),
                                        _ => 0,
                                    };
                                    if crate::trace::trace_enabled("CAST") {
                                        eprintln!(
                                            "[forge] unsize array->slice n={n} src={src_ty} dst={to_ty}"
                                        );
                                    }
                                    let dst = self.place_addr(place);
                                    self.builder.store(lo, dst);
                                    let eight = self.builder.iconst(8, TypeId::I64);
                                    let hi_addr = self.builder.iadd(dst, eight);
                                    let hi = self.builder.iconst(n as i64, TypeId::I64);
                                    self.builder.store(hi, hi_addr);
                                } else {
                                    let val = self.lower_rvalue(rvalue)?;
                                    self.store_place(place, val);
                                }
                            } else {
                                let val = self.lower_rvalue(rvalue)?;
                                self.store_place(place, val);
                            }
                        } else {
                            // checked 算术（AddWithOverflow 等）：结果是 (value, bool)
                            // 二元组——用 forge-ir overflow API 拆写 place 的 0/8 偏移
                            if let Rvalue::BinaryOp(bin_op, box (op1, op2)) = rvalue
                                && matches!(
                                    *bin_op,
                                    mir::BinOp::AddWithOverflow
                                        | mir::BinOp::SubWithOverflow
                                        | mir::BinOp::MulWithOverflow
                                )
                            {
                                let place_ty = place.ty(&self.body.local_decls, self.tcx).ty;
                                let place_sz = layout_bytes(self.tcx, place_ty);
                                if place_sz > 4 {
                                    let lhs = self.lower_operand(op1)?;
                                    let rhs = self.lower_operand(op2)?;
                                    let op1_ty = op1.ty(&self.body.local_decls, self.tcx);
                                    let signed = op1_ty.is_signed();
                                    let (v, f) =
                                        self.lower_checked_op(*bin_op, lhs, rhs, signed)?;
                                    let base = self.place_addr(place);
                                    self.builder.store(v, base);
                                    // 标志半区写到字段 1 的真实 layout 偏移（如
                                    // (i32, bool) 的 bool 在偏移 4）——硬编码 8 会
                                    // 写错位置，解构读字段 1 时读到垃圾。
                                    let f_off = self.field_offset(
                                        place.ty(&self.body.local_decls, self.tcx).ty,
                                        1,
                                    );
                                    let f_addr = if f_off != 0 {
                                        let off_v = self.builder.iconst(f_off, TypeId::I64);
                                        self.builder.iadd(base, off_v)
                                    } else {
                                        base
                                    };
                                    self.builder.store(f, f_addr);
                                    return Ok(());
                                }
                            }
                            // 聚合常量（如 <i32 as SizedTypeProperties>::LAYOUT）按
                            // 字节展开写槽（lower_rvalue 的 const 分支只处理标量，
                            // 聚合常量会退化为 0 → 字段读全错）。
                            if let Rvalue::Use(Operand::Constant(ct), _) = rvalue {
                                let p_ty = place.ty(&self.body.local_decls, self.tcx).ty;
                                let p_sz = layout_bytes(self.tcx, p_ty);
                                // &str/&[T] 字面量：ConstValue::Slice{alloc, meta}。
                                // ptr 指向 rodata（global_addr + intern_promoted 落盘）、
                                // meta = len——写槽 ptr@0 + len@8（ScalarPair）。
                                if let rustc_middle::mir::Const::Val(
                                    rustc_middle::mir::ConstValue::Slice { alloc_id, meta },
                                    _,
                                ) = ct.const_
                                {
                                    if crate::trace::trace_enabled("CONST") {
                                        eprintln!(
                                            "[forge] stmt const Slice alloc={alloc_id:?} meta={meta}"
                                        );
                                    }
                                    // 登记 slice 字节（intern_promoted 幂等，返回唯一
                                    // G 索引——与 global_addr 引用一致）
                                    let g =
                                        if let rustc_middle::mir::interpret::GlobalAlloc::Memory(
                                            alloc,
                                        ) = self.tcx.global_alloc(alloc_id)
                                        {
                                            let inner = &*alloc.0;
                                            let size = inner.size().bytes_usize();
                                            let bytes = inner
                                                .inspect_with_uninit_and_ptr_outside_interpreter(
                                                    0..size,
                                                )
                                                .to_vec();
                                            let align = inner.align.bytes();
                                            let sym = self.slice_sym(alloc_id);
                                            self.func_refs
                                                .intern_promoted(alloc_id, &sym, bytes, align)
                                        } else {
                                            let sym = self.slice_sym(alloc_id);
                                            self.func_refs.intern_global(alloc_id, &sym)
                                        };
                                    let base = self.place_addr(place);
                                    let ptr = self.builder.global_addr(GlobalId(g));
                                    self.builder.store(ptr, base);
                                    let lenv = self.builder.iconst(meta as i64, TypeId::I64);
                                    let eight = self.builder.iconst(8, TypeId::I64);
                                    let hi = self.builder.iadd(base, eight);
                                    self.builder.store(lenv, hi);
                                    return Ok(());
                                }
                                // Unevaluated 引用（如 promoted 数组
                                // `&[1,2,3]` = const promoted[0]，&[T;N] thin 引用）：
                                // eval 后 Ptr → global_addr 写 8B 槽（eval_const_bytes
                                // 对 Ptr 返回 0）。类型守卫：仅引用/指针类型。
                                use rustc_middle::ty::TyKind;
                                let is_ref_ty = matches!(
                                    p_ty.kind(),
                                    TyKind::Ref(..) | TyKind::RawPtr(..)
                                );
                                if is_ref_ty
                                    && let rustc_middle::mir::Const::Unevaluated(..) = ct.const_
                                    && let Ok(rustc_middle::mir::ConstValue::Scalar(
                                        rustc_middle::mir::interpret::Scalar::Ptr(ptr, _),
                                    )) = ct.const_.eval(
                                        self.tcx,
                                        ty::TypingEnv::fully_monomorphized(),
                                        rustc_span::DUMMY_SP,
                                    )
                                {
                                    if crate::trace::trace_enabled("CONST") {
                                        eprintln!(
                                            "[forge] stmt Unevaluated ref alloc={:?}",
                                            ptr.provenance.alloc_id()
                                        );
                                    }
                                    let alloc_id = ptr.provenance.alloc_id();
                                    let g =
                                        if let rustc_middle::mir::interpret::GlobalAlloc::Memory(
                                            alloc,
                                        ) = self.tcx.global_alloc(alloc_id)
                                        {
                                            let inner = &*alloc.0;
                                            let size = inner.size().bytes_usize();
                                            let bytes = inner
                                                .inspect_with_uninit_and_ptr_outside_interpreter(
                                                    0..size,
                                                )
                                                .to_vec();
                                            let align = inner.align.bytes();
                                            let sym = self.slice_sym(alloc_id);
                                            self.func_refs
                                                .intern_promoted(alloc_id, &sym, bytes, align)
                                        } else {
                                            let sym = self.slice_sym(alloc_id);
                                            self.func_refs.intern_global(alloc_id, &sym)
                                        };
                                    let base = self.place_addr(place);
                                    let ptrv = self.builder.global_addr(GlobalId(g));
                                    self.builder.store(ptrv, base);
                                    return Ok(());
                                }
                                if p_sz > 8
                                    && let Some(bytes) = self.eval_const_bytes(ct.const_)
                                {
                                    let base = self.place_addr(place);
                                    self.agg_const_bytes(base, &bytes);
                                    return Ok(());
                                }
                            }
                            let val = self.lower_rvalue(rvalue)?;
                            self.store_place(place, val);
                        }
                    }
                }
            }
            StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
            // rustc 把 intrinsics::copy_nonoverlapping 在 monomorphize 后展开
            // 成 MIR 专用语句 `Intrinsic(NonDivergingIntrinsic::CopyNonOverlapping
            // { src, dst, count })`（wrapper 的 bb3）——此前未处理落入 `_ => {}`
            // 忽略 → 复制不执行（buf[4] 恒 0）。内联逐 8 字节复制循环。
            StatementKind::Intrinsic(box rustc_middle::mir::NonDivergingIntrinsic::CopyNonOverlapping(
                rustc_middle::mir::CopyNonOverlapping { src, dst, count },
            )) => {
                if crate::trace::trace_enabled("STMT") {
                    eprintln!(
                        "[forge] stmt intrinsic CopyNonOverlapping src={src:?} dst={dst:?} count={count:?}"
                    );
                }
                let src_v = self.lower_operand(src)?;
                let dst_v = self.lower_operand(dst)?;
                let count_v = self.lower_operand(count)?;
                // 元素大小从 src 操作数的类型取（pointee）
                let src_ty = src.ty(&self.body.local_decls, self.tcx);
                let elem = self.pointee_ty(src_ty);
                let sz = layout_bytes(self.tcx, elem) as i64;                let cur = self.builder.current_block();
                let (loop_blk, lp) =
                    self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                let (body_blk, bp) =
                    self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                let done = self.builder.create_block();
                self.builder.switch_to_block(cur);
                let zero = self.builder.iconst(0, TypeId::I64);
                let one = self.builder.iconst(1, TypeId::I64);
                self.builder.jump(loop_blk, &[zero]);
                self.builder.switch_to_block(loop_blk);
                let i = lp[0];
                let cond = self.builder.icmp(IntCC::UnsignedLessThan, i, count_v);
                self.builder.branch(cond, body_blk, &[i], done, &[]);
                self.builder.switch_to_block(body_blk);
                let bi = bp[0];
                let off = if sz == 1 {
                    bi
                } else {
                    let szv = self.builder.iconst(sz, TypeId::I64);
                    self.builder.imul(bi, szv)
                };
                let saddr = self.builder.iadd(src_v, off);
                let v = self.builder.load(saddr, TypeId::I64);
                let daddr = self.builder.iadd(dst_v, off);
                self.builder.store(v, daddr);
                let i2 = self.builder.iadd(bi, one);
                self.builder.jump(loop_blk, &[i2]);
                self.builder.switch_to_block(done);
            }
            StatementKind::Intrinsic(kind) => {
                if crate::trace::trace_enabled("STMT") {
                    eprintln!("[forge] stmt intrinsic (unhandled): {kind:?}");
                }
            }
            _ => {
                if crate::trace::trace_enabled("STMT") {
                    eprintln!("[forge] stmt unhandled kind: {:?}", stmt.kind);
                }
            }
        }
        Ok(())
    }
}
