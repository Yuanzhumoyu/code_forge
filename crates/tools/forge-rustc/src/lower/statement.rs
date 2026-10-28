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
    ///
    /// 常量/静态的**地址值**（v27 TLS：线程局部静态在这里被拦截成动态 TLS 调用）。
    ///
    /// 四个调用点（`rvalue.rs` 的 const 地址、`mod.rs` 的 slice 实参、本文件的
    /// slice 字面量与 Unevaluated 引用）都走这一个入口 —— 隐藏符号/运行时命名只在这里
    /// 出现一次。返回 `None` 表示"不是可取地址的数据分配"。
    /// 线程局部静态的**本线程块指针**（动态 TLS 运行时；v27）。
    ///
    /// 两个调用者：`Rvalue::ThreadLocalRef`（rustc 对 `&TLS`/`TLS` 的专门 rvalue）
    /// 与 `const_data_addr` 的静态分支。
    pub(crate) fn tls_static_addr(
        &mut self,
        def_id: rustc_hir::def_id::DefId,
    ) -> Result<Value, ForgeError> {
        let ty = self.tcx.type_of(def_id).skip_binder();
        let layout = self
            .tcx
            .layout_of(ty::PseudoCanonicalInput {
                typing_env: ty::TypingEnv::fully_monomorphized(),
                value: ty,
            })
            .map_err(|e| {
                // 用构造器而不是裸字符串：缺口守卫认的是构造器调用这个形态
                //（台账行 ↔ 源码站点双向相等）。**别在注释里原样写它的字面形态**
                //——守卫是文本扫描，注释里的示例也会被算成第二个站点（实测踩过）。
                ForgeError::from(IrError::capability(
                    "TLS",
                    format!(
                        "取不到线程局部静态 `{}` 的布局：{e:?}",
                        self.tcx.def_path_str(def_id)
                    ),
                ))
            })?;
        let size = layout.size.bytes() as i64;
        let align = layout.align.abi.bytes() as i64;
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
        let slot = self
            .func_refs
            .intern_global_named(&crate::tls_runtime::tls_slot_sym(&sym));
        let init = self
            .func_refs
            .intern_global_named(&crate::tls_runtime::tls_init_sym(&sym));
        let slot_addr = self.builder.global_addr(GlobalId::new(slot));
        let init_addr = self.builder.global_addr(GlobalId::new(init));
        let size_v = self.builder.const_int(size, TypeId::I64);
        let align_v = self.builder.const_int(align, TypeId::I64);
        let fr = self.func_refs.intern(crate::tls_runtime::TLS_RUNTIME);
        let p = self.builder.call(
            FuncRef::new(fr),
            &[slot_addr, size_v, align_v, init_addr],
            &[TypeId::I64],
        )[0];
        Ok(p)
    }

    pub(crate) fn const_data_addr(
        &mut self,
        alloc_id: rustc_middle::mir::interpret::AllocId,
    ) -> Result<Option<Value>, ForgeError> {
        use rustc_middle::mir::interpret::GlobalAlloc;
        if let GlobalAlloc::Static(def_id) = self.tcx.global_alloc(alloc_id)
            && self.tcx.is_thread_local_static(def_id)
        {
            // 线程局部静态：不取 .data 符号地址（那会丢线程私有语义），走动态 TLS。
            return Ok(Some(self.tls_static_addr(def_id)?));
        }
        Ok(self
            .intern_const_data(alloc_id)
            .map(|g| self.builder.global_addr(GlobalId::new(g))))
    }

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
                                    let dval = self.builder.const_int(discr as i64, TypeId::I32);
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
                                        self.builder.const_int(niche_value as i64, TypeId::I64)
                                    } else {
                                        self.builder.const_int(niche_value as i64, TypeId::I32)
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
                                        let off_v = self.builder.const_int(tag_off, TypeId::I64);
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
                                    // **ZST 字段没有内存表示**（v27 M6-116）：`()` 等
                                    // 字段的 `map_type` 是 VOID，lower_operand 会造
                                    // `Iconst ty=VOID`，随后 Store 拿着 bits=0 的值
                                    // 找不到规则（`Store no matching rule`）。
                                    if f_sz == 0 {
                                        continue;
                                    }
                                    let faddr = if off == 0 {
                                        base
                                    } else {
                                        let off_v = self.builder.const_int(off, TypeId::I64);
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
                                let off_v = self.builder.const_int(off, TypeId::I64);
                                // base 统一为 I64（PTR 的 iadd 在特定上下文 lowering 丢——绕开）
                                //
                                // **必须是 `ptrtoint`/`inttoptr` 往返，不能 `ireduce`**：
                                // base 的类型是 PTR，`ireduce` 只接受整数源（verifier 报
                                // `ConversionBitWidthMismatch ... 类别不匹配`，实测
                                // `opt_bounds_panic`/`opt_slice_sum` 编译失败）；而
                                // `store`/`load` 的地址**必须**是指针类型（否则
                                // `StoreAddrNotPointer`），所以算完偏移要转回 PTR。
                                let base_i = self.builder.ptrtoint(base, TypeId::I64);
                                let fa_i = self.builder.iadd(base_i, off_v);
                                self.builder.inttoptr(fa_i, TypeId::PTR)
                            };
                            let f_ty = f.ty(&self.body.local_decls, self.tcx);
                            let f_sz = layout_bytes(self.tcx, f_ty);
                            // **ZST 字段没有内存表示**（v27 M6-116 补漏）：`Iter` 的
                            // `_marker: PhantomData` 是 0 字节；写它会越过槽尾写到
                            // `fp-16`（= 被调方 x29 保存槽）⇒ 调用者 fp 变 0 ⇒ 下一条
                            // fp 相对 store 直接 Data Abort。枚举分支已跳过，这里同样跳过。
                            if f_sz == 0 {
                                continue;
                            }
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
                                let off_v = self.builder.const_int(off, TypeId::I64);
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
                        // **`&slice[from..to]`（Subslice 投影）**：语义是构造**新的**
                        // fat pointer（ptr + from*elem，len - from - to）；WA-25 的
                        // “整值复制”会原样复制旧 ptr/len ⇒ `from_ascii_bytes_radix_impl`
                        // 的 `s = &s[1..]` 循环永不缩短 ⇒ `from_str` 在 -O0/-O1/-O2
                        // 全部死循环（KD-04 根因，MIR bb13/bb18）。这里显式重算 len。
                        if size > 8
                            && let Rvalue::Ref(_, _, p) = rvalue
                            && matches!(ty.kind(), rustc_middle::ty::TyKind::Ref(_, _, _))
                            && let Some((from, to)) = ref_subslice_bounds(p)
                        {
                            let src = self.place_addr(&mir::Place {
                                local: p.local,
                                projection: ty::List::empty(),
                            });
                            let lo = self.builder.load(src, TypeId::I64);
                            let eight = self.builder.const_int(8, TypeId::I64);
                            let hi_addr = self.builder.iadd(src, eight);
                            let len = self.builder.load(hi_addr, TypeId::I64);
                            let pointee = ty.builtin_deref(true).unwrap_or(ty);
                            let elem_size = match pointee.kind() {
                                rustc_middle::ty::TyKind::Slice(e) => {
                                    layout_bytes(self.tcx, *e) as i64
                                }
                                _ => 1,
                            };
                            let from_v =
                                self.builder.const_int(from as i64 * elem_size, TypeId::I64);
                            let new_lo = self.builder.iadd(lo, from_v);
                            let from_len = self.builder.const_int(from as i64, TypeId::I64);
                            let mut new_len = self.builder.isub(len, from_len);
                            if to != 0 {
                                let to_v = self.builder.const_int(to as i64, TypeId::I64);
                                new_len = self.builder.isub(new_len, to_v);
                            }
                            let dst = self.place_addr(place);
                            self.builder.store(new_lo, dst);
                            let dst_hi = self.builder.iadd(dst, eight);
                            self.builder.store(new_len, dst_hi);
                        } else if size > 8
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
                            && let Rvalue::Cast(_kind, op, _to_ty) = rvalue
                            && let Operand::Constant(ct) = op
                            && let rustc_middle::mir::Const::Val(
                                rustc_middle::mir::ConstValue::Slice { alloc_id, meta },
                                _,
                            ) = ct.const_
                        {
                            // **胖指针常量的 transmute**（`s.as_bytes()` 在 -O2 被
                            // 内联成 `&str → &[u8]` 的 `Transmute`）：常量本身不在
                            // 栈槽里，不能走下面的 place 复制；直接按 ScalarPair 写
                            // dst（ptr@0 = rodata/data 地址、len@8 = meta）。
                            // 旧实现落到 `lower_rvalue` 的标量兜底 ⇒ 写 `null`，
                            // `PtrMetadata` 读 len=0（`parse_u32` 在 -O2 恒返回 0）。
                            if let Some(ptr) = self.const_data_addr(alloc_id)? {
                                let dst = self.place_addr(place);
                                self.builder.store(ptr, dst);
                                let eight = self.builder.const_int(8, TypeId::I64);
                                let hi = self.builder.iadd(dst, eight);
                                let lenv = self.builder.const_int(meta as i64, TypeId::I64);
                                self.builder.store(lenv, hi);
                            }
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
                                    let hi = self.builder.global_addr(GlobalId::new(g));
                                    let dst = self.place_addr(place);
                                    self.builder.store(lo, dst);
                                    let eight = self.builder.const_int(8, TypeId::I64);
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
                                    let eight = self.builder.const_int(8, TypeId::I64);
                                    let hi_addr = self.builder.iadd(dst, eight);
                                    let hi = self.builder.const_int(n as i64, TypeId::I64);
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
                                // **`place_sz <= 4` 也走这里**（2026-10-19 修）：以前只在
                                // `> 4` 时拆写，小聚合（如 `(u16, bool)` 4 字节、
                                // `(u8, bool)` 2 字节）落到下面"当普通 rvalue"⇒ 标志位
                                // **被丢掉** ⇒ `-Coverflow-checks=on` 静默回绕。
                                // 小聚合**不能**沿用"I32 存标志"（2 字节槽写 4 字节会
                                // 覆盖相邻局部，WA-22 的坑）⇒ 按标志字段的**真实字节宽**
                                // 落槽。
                                if place_sz <= 4 {
                                    let lhs = self.lower_operand(op1)?;
                                    let rhs = self.lower_operand(op2)?;
                                    let op1_ty = op1.ty(&self.body.local_decls, self.tcx);
                                    let signed = op1_ty.is_signed();
                                    let (v, f) =
                                        self.lower_checked_op(*bin_op, lhs, rhs, signed)?;
                                    let flag_ty = self.field_ty(place_ty, 1);
                                    let flag_sz = layout_bytes(self.tcx, flag_ty);
                                    let base = self.place_addr(place);
                                    // 值半区：宽度由值自身类型决定（u8 → 1 字节）
                                    self.builder.store(v, base);
                                    let target = match flag_sz {
                                        1 => Some(TypeId::I8),
                                        2 => Some(TypeId::I16),
                                        4 => Some(TypeId::I32),
                                        8 => Some(TypeId::I64),
                                        _ => None,
                                    };
                                    let Some(target) = target else {
                                        return Err(ForgeError::Message(format!(
                                            "checked 算术的标志字段宽度不支持：{flag_sz} 字节（{place_ty}）"
                                        )));
                                    };
                                    // MIR 里 checked 算术的标志**恒为 `bool`** ⇒ 先
                                    // uextend 到 **I32**（既有路径实证可用），再按标志
                                    // 字段真实宽度 ireduce。**不能**直接
                                    // `uextend(BOOL → I8)`。
                                    let f32v = if self.builder.value_type(f) == Some(TypeId::BOOL) {
                                        self.builder.uextend(f, TypeId::I32)
                                    } else {
                                        f
                                    };
                                    let f_val = if target == TypeId::I32 {
                                        f32v
                                    } else {
                                        self.builder.ireduce(f32v, target)
                                    };
                                    let f_off = self.field_offset(place_ty, 1);
                                    let f_addr = if f_off != 0 {
                                        let off_v = self.builder.const_int(f_off, TypeId::I64);
                                        self.builder.iadd(base, off_v)
                                    } else {
                                        base
                                    };
                                    self.builder.store(f_val, f_addr);
                                    return Ok(());
                                }
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
                                        let off_v = self.builder.const_int(f_off, TypeId::I64);
                                        self.builder.iadd(base, off_v)
                                    } else {
                                        base
                                    };
                                    // WA-22：flag（bool）store 宽度必须窄——主库对
                                    // BOOL 值的 store 在 (i32,bool) 聚合邻槽场景写 8 字节
                                    //（movq），覆盖相邻局部槽（bump1 实证：flag@+4 的
                                    // movq 覆盖 _1 参数槽 -0x50 的 p → 写回 store [0]
                                    // SEGV）。显式收窄到 I32（4 字节，槽内不越界），
                                    // 解构读 bool 只取低字节不受影响。
                                    //
                                    // **`bool`(i1) → i32 是加宽**：`ireduce` 只能变窄，用它
                                    // 会产出非法 IR（verifier: ConversionBitWidthMismatch
                                    // `t1 → t4`）。按值类型分派。
                                    let f32 = if self.builder.value_type(f) == Some(TypeId::BOOL) {
                                        self.builder.uextend(f, TypeId::I32)
                                    } else {
                                        self.builder.ireduce(f, TypeId::I32)
                                    };
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
                                    let Some(ptr) = self.const_data_addr(alloc_id)? else {
                                        // 不可达防御：Slice 字面量恒为 Memory/Static，
                                        // 其余 GlobalAlloc 种类不应出现在此（见
                                        // intern_const_data）。
                                        return Ok(());
                                    };
                                    let base = self.place_addr(place);
                                    self.builder.store(ptr, base);
                                    let lenv = self.builder.const_int(meta as i64, TypeId::I64);
                                    let eight = self.builder.const_int(8, TypeId::I64);
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
                                    let Some(ptrv) = self.const_data_addr(alloc_id)? else {
                                        // 不可达防御（见上）。
                                        return Ok(());
                                    };
                                    let base = self.place_addr(place);
                                    self.builder.store(ptrv, base);
                                    return Ok(());
                                }
                                // **任意非标量聚合常量**都按字节写槽（不再只限 >8）：
                                // `Option<i32>`/`Result<i32,i32>` 这类 8 字节聚合
                                // 在 -O2 会被常量传播成 `Operand::Constant`；若落
                                // `lower_rvalue` 的标量兜底会变成 0 值（实测
                                // `Ok(0x12345678)` → 全零 ⇒ match 得 0）。bool/
                                // 整数/浮点/char 仍走标量路径（槽宽语义由
                                // store_place 决定）。
                                let scalar_like = matches!(
                                    p_ty.kind(),
                                    rustc_middle::ty::TyKind::Bool
                                        | rustc_middle::ty::TyKind::Char
                                        | rustc_middle::ty::TyKind::Int(_)
                                        | rustc_middle::ty::TyKind::Uint(_)
                                        | rustc_middle::ty::TyKind::Float(_)
                                );
                                // **含指针的聚合常量不能走字节**：`eval_const_bytes`
                                // 对 `Scalar::Ptr` 只取比特，丢重定位（`Some(&x)` 会
                                // 变成数字地址）。只放行“纯数据”聚合（如
                                // `Result<i32,i32>`）。
                                let has_ptr = p_ty.walk().any(|arg| {
                                    arg.as_type().is_some_and(|t| {
                                        matches!(
                                            t.kind(),
                                            rustc_middle::ty::TyKind::Ref(..)
                                                | rustc_middle::ty::TyKind::RawPtr(..)
                                                | rustc_middle::ty::TyKind::FnPtr(..)
                                        )
                                    })
                                });
                                if !scalar_like
                                    && p_sz >= 8
                                    && !has_ptr
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
                // 地址统一成 PTR（ABI 指针可能以 i64 槽传入）：否则 load/store
                // 的地址是整数，-O1 verifier 报 LoadAddrNotPointer。
                let src_v = if self.builder.value_type(src_v) == Some(TypeId::PTR) {
                    src_v
                } else {
                    self.builder.inttoptr(src_v, TypeId::PTR)
                };
                let dst_v = if self.builder.value_type(dst_v) == Some(TypeId::PTR) {
                    dst_v
                } else {
                    self.builder.inttoptr(dst_v, TypeId::PTR)
                };
                // 元素大小/对齐从 src 操作数的类型取（pointee）
                let src_ty = src.ty(&self.body.local_decls, self.tcx);
                let elem = self.pointee_ty(src_ty);
                let sz = layout_bytes(self.tcx, elem) as i64;
                let elem_align = crate::layout::layout_align(self.tcx, elem).max(1);
                let cur = self.builder.current_block();
                let (loop_blk, lp) = self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                let (body_blk, bp) = self.builder.create_block_with_params(&[(TypeId::I64, "i")]);
                let done = self.builder.create_block();
                self.builder.switch_to_block(cur);
                let zero = self.builder.const_int(0, TypeId::I64);
                let one = self.builder.const_int(1, TypeId::I64);
                self.builder.jump(loop_blk, &[zero]);
                self.builder.switch_to_block(loop_blk);
                let i = lp[0];
                let cond = self.builder.icmp(IntCC::Ult, i, count_v);
                self.builder.branch(cond, body_blk, &[i], done, &[]);
                self.builder.switch_to_block(body_blk);
                let bi = bp[0];
                let off = if sz == 1 {
                    bi
                } else {
                    let szv = self.builder.const_int(sz, TypeId::I64);
                    self.builder.imul(bi, szv)
                };
                // **精确按元素字节数复制**（v27 M6-122）：旧实现每元素固定搬
                // `(0..sz).step_by(8)` 个 8 字节块——`i32` 元素因此写 8 字节
                // （越界 4 字节 + 4 对齐地址上的 `str x` ⇒ arm64 Data Abort，
                // `core::slice::sort` 的 insert_tail 必现）；`sz>8` 时余数同样
                // 越界。现在按 `elem_align` 选块宽、8/4/2/1 阶梯精确覆盖。
                if sz > 0 {
                    let saddr = self.builder.iadd(src_v, off);
                    let daddr = self.builder.iadd(dst_v, off);
                    self.copy_bytes(daddr, saddr, sz as u32, elem_align);
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
/// `&place` 的投影里若含 `Subslice { from, to }`（其余投影只允许 Deref/OpaqueCast），
/// 返回 `(from, to)`；`to` 按 rustc 语义是**从尾部**排除的长度（`[from .. len-to]`）。
fn ref_subslice_bounds<'tcx>(p: &mir::Place<'tcx>) -> Option<(u64, u64)> {
    let mut found = None;
    for pe in p.projection.iter() {
        match pe {
            mir::ProjectionElem::Deref | mir::ProjectionElem::OpaqueCast(_) => {}
            mir::ProjectionElem::Subslice { from, to, .. } => {
                if found.is_some() {
                    return None;
                }
                found = Some((from, to));
            }
            _ => return None,
        }
    }
    found
}
