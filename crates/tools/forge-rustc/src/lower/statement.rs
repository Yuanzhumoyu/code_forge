//! Statement lowering（含 terminator 分发）。
use super::*;

impl<'tcx, 'f> LowerCtxt<'tcx, 'f> {
    /// slice 常量（&str/&[T] 字面量）的 rodata 符号名：**内容稳定键**。
    ///
    /// M3/M4（并行 CGU Stage A）曾用 `__slice_{alloc_id:?}`（alloc_id 纯函数
    /// 命名）——但 WA-38 残余（M5 关闭）：真并行（-Z threads +
    /// FORGE_CODEGEN_THREADS>1）下 rustc 全局 AllocId 由 `AtomicU64` 按
    /// **首次请求序**分配（mir/interpret/mod.rs `AllocMap::next_id`；内容
    /// dedup 只保证同内容同 id，不保证同内容跨运行同**数值**）——各函数
    /// 降级任务在池上调度序不定 → 同一内容跨运行符号名数值漂移。M5 改为
    /// **（bytes, align）64 位 FNV-1a 内容哈希**：跨运行与调度序无关，
    /// 同名必同内容（intern/merge 按内容去重 + debug_assert 碰撞防护）。
    /// 命名仍是任务可独立计算的纯函数（无全局状态、不依赖 alloc_id）。
    pub(crate) fn slice_sym(bytes: &[u8], align: u64) -> String {
        let mut canonical = Vec::with_capacity(bytes.len() + 8);
        canonical.extend_from_slice(bytes);
        canonical.extend_from_slice(&align.to_le_bytes());
        format!("__slice_{:016x}", super::fnv1a64(&canonical))
    }

    /// Slice/const 数据引用的数据段 GlobalId（M5 稳定键重构统一入口，
    /// 替代各处内联的 global_alloc 分派）：
    /// - `GlobalAlloc::Memory` → `intern_promoted`（rodata，内容稳定名）；
    /// - `GlobalAlloc::Static(def_id)` → `intern_global`（真实 static 符号名，
    ///   与 mono_symbol 一致——static 项符号不参与 alloc_id 编号）；
    /// - 其他种类（Function/VTable/TypeId）→ `None`（调用方跳过；slice 常量
    ///   引用路径上不应出现）。
    pub(crate) fn intern_const_data(
        &mut self,
        alloc_id: rustc_middle::mir::interpret::AllocId,
    ) -> Option<u32> {
        let g = match self.tcx.global_alloc(alloc_id) {
            rustc_middle::mir::interpret::GlobalAlloc::Memory(alloc) => {
                let inner = &*alloc.0;
                let size = inner.size().bytes_usize();
                let bytes = inner
                    .inspect_with_uninit_and_ptr_outside_interpreter(0..size)
                    .to_vec();
                let align = inner.align.bytes();
                let sym = Self::slice_sym(&bytes, align);
                self.func_refs.intern_promoted(alloc_id, &sym, bytes, align)
            }
            rustc_middle::mir::interpret::GlobalAlloc::Static(def_id) => {
                let instantiating_crate = if def_id.is_local() {
                    rustc_hir::def_id::LOCAL_CRATE
                } else {
                    def_id.krate
                };
                let sym = rustc_symbol_mangling::symbol_name_for_instance_in_crate(
                    self.tcx,
                    rustc_middle::ty::Instance::mono(self.tcx, def_id),
                    instantiating_crate,
                );
                self.func_refs.intern_global(alloc_id, &sym)
            }
            _ => return None,
        };
        Some(g)
    }
    pub(crate) fn lower_statement(
        &mut self,
        stmt: &rustc_middle::mir::Statement<'tcx>,
    ) -> Result<(), ForgeError> {
        match &stmt.kind {
            StatementKind::Assign((place, rvalue)) => {
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
                                // 枚举构造诊断：无条件打印 variant/布局（WA-28 根因排查）
                                if crate::trace::trace_enabled("DISCR") {
                                    let lv = self.tcx.layout_of(ty::PseudoCanonicalInput {
                                        typing_env: ty::TypingEnv::fully_monomorphized(),
                                        value: ty,
                                    });
                                    let vnames = enum_def
                                        .variants()
                                        .iter()
                                        .map(|v| v.name.to_string())
                                        .collect::<Vec<_>>();
                                    eprintln!(
                                        "[forge] ENUM CONSTRUCT {ty} variant_idx={} variant_names={vnames:?} backend_repr={:?} variants={:?}",
                                        variant_idx.index(),
                                        lv.as_ref().map(|l| l.layout.backend_repr.clone()),
                                        lv.as_ref().map(|l| l.layout.variants().clone()),
                                    );
                                }
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
                                    tag,
                                    tag_encoding:
                                        rustc_abi::TagEncoding::Niche {
                                            untagged_variant,
                                            niche_variants,
                                            niche_start,
                                            ..
                                        },
                                    ..
                                } = layout.layout.variants()
                                    && variant_idx != *untagged_variant
                                {
                                    let k = variant_idx.as_u32() - niche_variants.start.as_u32();
                                    let niche_value = (k as u128).wrapping_add(*niche_start);
                                    if crate::trace::trace_enabled("DISCR") {
                                        // WA-28 诊断：None 构造 tag 写入偏移全貌
                                        eprintln!(
                                            "[forge] Niche CONSTRUCT-DETAIL variant_idx_raw={} place_ty={ty} backend_repr={:?} variants={:?}",
                                            variant_idx.index(),
                                            layout.layout.backend_repr,
                                            layout.layout.variants(),
                                        );
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
                                    // WA-29：写入宽度必须匹配 tag 标量宽度——
                                    // 判别读（rvalue.rs）按 I64（8 字节）load +
                                    // uextend（指针 tag 的 niche 值 = null），若
                                    // 这里恒 I32（movl 32 位）写，8 字节指针 tag
                                    // 的高 4 字节残留栈上旧值 → 读出的"null"
                                    // ≠ 0 → 判别误判 untagged（enumerate None
                                    // 路径 SEGV 根因：Enumerate::next 的 None
                                    // 构造 movl 写 _0+8，判别读 8 字节含残留
                                    // 高位 → main 误判 Some → **x 解引用 null）。
                                    //
                                    // WA-42：宽度取**枚举 tag 标量自身**
                                    //（`Variants::Multiple { tag, .. }`——Niche 编码下
                                    // rustc 给的 tag 就是 niche 字段的标量）。此前按
                                    // `backend_repr` 两分支推导 + `_ => 4` 兜底：宽聚合
                                    // payload（`Memory` repr，如 `Option<(NonNull<u8>,
                                    // Layout)>` 24 字节、niche = offset 0 的 8 字节指针）
                                    // 落进兜底 → `store i32 0` 只写低 4 字节，高 4 字节
                                    // 残留（本机实测 0x00007ffd00000000）→ 调用方按 8
                                    // 字节判空失败 → `finish_grow` 误取 `Some(野指针)`
                                    // → `grow_impl_runtime` 的 copy 解引用 → AV
                                    //（WA-41 本机确定性复现的根因）。
                                    let tag_w = tag.primitive().size(&self.tcx).bytes() as u32;
                                    let nv = if tag_w >= 8 {
                                        self.builder.iconst(niche_value as i64, TypeId::I64)
                                    } else {
                                        self.builder.iconst(niche_value as i64, TypeId::I32)
                                    };
                                    // tag 位置：**按 rustc 布局派生**（WA-44）——
                                    // `niche_tag_offset` 统一处理 ScalarPair 的标量
                                    // 偏移与聚合 payload 的 tag_field 偏移；旧实现
                                    // 恒给 offset 0（niche 在 offset 16 的 24 字节
                                    // 枚举被判读到字段 0 → Some 误判 None）。
                                    let tag_off = self.niche_tag_offset(&layout)?;
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
                            if crate::trace::trace_enabled("AGG") {
                                eprintln!(
                                    "[forge] agg field ty={ty} i={i} f_ty={f_ty} f_sz={f_sz} kind={:?}",
                                    f_ty.kind()
                                );
                            }
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
                        // WA-25：fat pointer 的 Ref（`&*fat_ptr`）——目标类型
                        // 是 &[T]/&str/&dyn（16 字节 ScalarPair），语义是复制
                        // fat pointer 值本身（ptr@0 + len/vtable@8），而非槽地址。
                        // deref 链实证：`_0 = &(*_2)`（_2: *const [i32]）若走
                        // lower_rvalue 的 Rvalue::Ref→place_addr 只写 8 字节
                        // ptr，len 半区不写 → PtrMetadata 读 len=0（Vec deref
                        // 后 slice.len()=0）。按 size 整值复制（与 Use/Cast 的
                        // 聚合复制一致）。
                        if size > 8
                            && let Rvalue::Ref(_, _, p) = rvalue
                            && (matches!(ty.kind(), rustc_middle::ty::TyKind::Ref(_, _, _)))
                        {
                            if crate::trace::trace_enabled("CAST") {
                                eprintln!(
                                    "[forge] WA-25 fat Ref copy dst={place:?} src={p:?} size={size} ty={ty}"
                                );
                            }
                            let dst = self.place_addr(place);
                            // WA-25 修正：复制源必须跳过 Deref 投影——`&(*_5)`
                            // 的语义是复制 fat pointer 值（ptr@0 + len@8），
                            // place_addr((*_5)) 会解引用读 8 字节 ptr（HEAP
                            // 数据地址）→ 从 HEAP 复制 16 字节 → len 半区是
                            // 堆数据垃圾（frp 实证：from_raw_parts 后
                            // sl.len()=0）。源 = 纯 local 槽（去掉 Deref）。
                            let src = self.place_addr(&mir::Place {
                                local: p.local,
                                projection: ty::List::empty(),
                            });
                            self.copy_agg(dst, src, size);
                        } else if size > 8
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
                                        rustc_middle::ty::TyKind::Array(_, len) => {
                                            len.try_to_target_usize(self.tcx).unwrap_or(0)
                                        }
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
                            if let Rvalue::BinaryOp(bin_op, (op1, op2)) = rvalue
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
                                    // WA-22：flag（bool）store 宽度必须窄——主库对
                                    // BOOL 值的 store 在 (i32,bool) 聚合邻槽场景写 8 字节
                                    //（movq），覆盖相邻局部槽（bump1 实证：flag@+4 的
                                    // movq 覆盖 _1 参数槽 -0x50 的 p → 写回 store [0]
                                    // SEGV）。显式 ireduce 到 I32（4 字节，槽内不越界），
                                    // 解构读 bool 只取低字节不受影响。
                                    let f32 = self.builder.ireduce(f, TypeId::I32);
                                    self.builder.store(f32, f_addr);
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
                                    // 登记 slice 字节（intern_const_data 内容去重幂等，
                                    // 返回唯一 G 索引——与 global_addr 引用一致）
                                    let Some(g) = self.intern_const_data(alloc_id) else {
                                        // 不可达防御：Slice 字面量恒为 Memory/Static，
                                        // 其余 GlobalAlloc 种类不应出现在此（见
                                        // intern_const_data）。
                                        return Ok(());
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
                                let is_ref_ty =
                                    matches!(p_ty.kind(), TyKind::Ref(..) | TyKind::RawPtr(..));
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
                                    let Some(g) = self.intern_const_data(alloc_id) else {
                                        // 不可达防御（见上）。
                                        return Ok(());
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
            // 忽略 → 复制不执行（buf[4] 恒 0）。内联逐元素复制（每个元素按
            // 元素大小 sz 展开 8 字节块；sz>8 时全宽——WA-37 D2，见下）。
            StatementKind::Intrinsic(
                rustc_middle::mir::NonDivergingIntrinsic::CopyNonOverlapping(
                    rustc_middle::mir::CopyNonOverlapping { src, dst, count },
                ),
            ) => {
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
                let sz = layout_bytes(self.tcx, elem) as i64;
                let cur = self.builder.current_block();
                let (loop_blk, lp) = self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                let (body_blk, bp) = self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
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
                // WA-37 D2：逐元素复制必须是**全宽**——元素 >8B（如
                // portable_simd Simd::load 的 [f32; 8] 32B 元素，count=1）
                // 时旧实现每元素只搬 8 字节（末 24 字节不写 → 槽残留垃圾/
                // 零 → lane2.. 读 0，V256 探针错值根因）。按元素 sz 展开
                // 8 字节块（sz/8 块，sz 编译期已知 → 静态展开；与 copy_agg
                // 的整值复制约定一致）。sz≤8 时退化为单块（语义与旧行为
                // 相同：u8 元素在 i*1 处 8 字节重叠复制收敛 memcpy）。
                // 当前块 = body_blk（create_block_with_params 已切回，
                // 下文 iadd/load/store 均留在本块，块末尾 jump 回 loop）。
                for chunk in (0..sz).step_by(8) {
                    let chunk_off = if chunk == 0 {
                        off
                    } else {
                        let co = self.builder.iconst(chunk, TypeId::I64);
                        self.builder.iadd(off, co)
                    };
                    let saddr = self.builder.iadd(src_v, chunk_off);
                    let v = self.builder.load(saddr, TypeId::I64);
                    let daddr = self.builder.iadd(dst_v, chunk_off);
                    self.builder.store(v, daddr);
                }
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
