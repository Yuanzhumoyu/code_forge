//! Rvalue / Operand / 二元一元运算 lowering。
use super::*;

impl<'tcx, 'f> LowerCtxt<'tcx, 'f> {
    pub(crate) fn lower_rvalue(&mut self, rvalue: &Rvalue<'tcx>) -> Result<Value, ForgeError> {
        match rvalue {
            Rvalue::Use(op, _) => self.lower_operand(op),
            Rvalue::BinaryOp(bin_op, (op1, op2)) => {
                let lhs = self.lower_operand(op1)?;
                let rhs = self.lower_operand(op2)?;
                // 符号性决定 sdiv/udiv、srem/urem、sshr/ushr 的选择
                // （forge-ir 的 Int 类型不带符号位，必须从 MIR 操作数类型推导）
                let op1_ty = op1.ty(&self.body.local_decls, self.tcx);
                let signed = op1_ty.is_signed();
                self.lower_binary_op(*bin_op, lhs, rhs, signed, op1_ty)
            }
            Rvalue::UnaryOp(un_op, op) => {
                // PtrMetadata（fat pointer 元数据）：&str/&[T] 的 len。
                // 单独处理——lower_operand 返回 ptr 单值，metadata 需从
                // place 槽读（fat ptr = ptr@0 + metadata@8）。
                if *un_op == mir::UnOp::PtrMetadata {
                    if let Operand::Copy(p) | Operand::Move(p) = op {
                        if let Some(meta) = self.fat_ptr_metadata(p) {
                            return Ok(meta);
                        }
                    }
                }
                let val = self.lower_operand(op)?;
                let is_bool = op.ty(&self.body.local_decls, self.tcx).is_bool();
                self.lower_unary_op(*un_op, val, is_bool)
            }
            Rvalue::Cast(cast_kind, op, to_ty) => {
                let mut val = self.lower_operand(op)?;
                let to_type = map_type(*to_ty, self.tcx)?;
                match cast_kind {
                    // FloatToInt（f32/f64 → i32/i64）：必须走 Fptosi（XMM →
                    // GPR，cvttss2si/cvttsd2si）——曾与 IntToInt 一起走
                    // ireduce（GPR 语义），f32 值在 XMM 却被整数截断 →
                    // 垃圾（float_array_construct 反汇编实证 movl %r15d）。
                    rustc_middle::mir::CastKind::FloatToInt => {
                        Ok(self.builder.fptosi(val, to_type))
                    }
                    // IntToFloat（i32/i64 → f32/f64）：Sitofp（GPR → XMM）。
                    rustc_middle::mir::CastKind::IntToFloat => {
                        Ok(self.builder.sitofp(val, to_type))
                    }
                    rustc_middle::mir::CastKind::IntToInt => {
                        // 窄类型零扩展 mask（u8/u16/bool → 更宽）：
                        // 主库 load/算术按 32 位"寄存器安全"宽度（opsize 折中），
                        // u8 值的高 24 位是栈槽残留垃圾（write_bytes_loop 曾把
                        // buf[0] 读成 0xABABABAB）。无符号源在参与 64 位运算前
                        // mask 低 8/16 位清零高位。
                        // WA-36：有符号窄源（i8/i16 → 更宽）必须**符号扩展**
                        //（sextend，movsx 链）——原实现 ireduce 截断（i8 -50
                        // 位 0xCE 被 movzx 表示 206 → cast 后仍 +206）；原注释
                        // "符号扩展依赖 ireduce 后的使用"未实现（i8 max 实证）。
                        let src_ty = op.ty(&self.body.local_decls, self.tcx);
                        let is_int = matches!(
                            src_ty.kind(),
                            rustc_middle::ty::TyKind::Int(_)
                                | rustc_middle::ty::TyKind::Uint(_)
                                | rustc_middle::ty::TyKind::Bool
                        );
                        if is_int && !src_ty.is_signed() {
                            let sz = src_ty.primitive_size(self.tcx);
                            if sz.bytes() < 4 {
                                let mask = (1u64 << (sz.bytes() * 8)) - 1;
                                let m = self.builder.iconst(mask as i64, TypeId::I64);
                                val = self.builder.band(val, m);
                            }
                        }
                        if is_int && src_ty.is_signed() {
                            let src_w = src_ty.primitive_size(self.tcx).bytes() as u32;
                            let to_w = layout_bytes(self.tcx, *to_ty);
                            if src_w < to_w {
                                return Ok(self.builder.sextend(val, to_type));
                            }
                        }
                        Ok(self.builder.ireduce(val, to_type))
                    }
                    rustc_middle::mir::CastKind::PtrToPtr
                    | rustc_middle::mir::CastKind::FnPtrToPtr => Ok(val),
                    _ => Ok(val),
                }
            }
            Rvalue::Ref(_, _, place) => {
                // &x / &mut x → 该 place 的栈地址
                Ok(self.place_addr(place))
            }
            Rvalue::RawPtr(_, place) => {
                // &raw const v / &v as *const → 该 place 的栈地址
                Ok(self.place_addr(place))
            }
            Rvalue::Discriminant(place) => {
                // 枚举判别读取：
                // - Direct tag（非 niche）：判别字段在 tag_field（通常 offset 0），读 I32
                // - Niche（Option/Result）：判别嵌入 payload，读 8 字节原始值后按 niche
                //   表规范化（niche 值 → 对应 variant 索引；否则 untagged_variant 索引），
                //   使 switchInt 的 case（variant 索引 0/1）正确匹配
                let addr = self.place_addr(place);
                let ty = place.ty(&self.body.local_decls, self.tcx).ty;
                let layout = self
                    .tcx
                    .layout_of(ty::PseudoCanonicalInput {
                        typing_env: ty::TypingEnv::fully_monomorphized(),
                        value: ty,
                    })
                    .map_err(|e| ForgeError::Message(format!("layout_of: {e}")))?;
                match &layout.layout.variants() {
                    rustc_abi::Variants::Multiple {
                        tag_encoding: rustc_abi::TagEncoding::Direct,
                        ..
                    } => {
                        if crate::trace::trace_enabled("DISCR") {
                            let lf = layout.layout.fields();
                            let mut fo = Vec::new();
                            for i in 0..lf.count() {
                                fo.push(lf.offset(i).bytes());
                            }
                            let tf = match &layout.layout.variants() {
                                rustc_abi::Variants::Multiple {
                                    tag_field,
                                    tag_encoding,
                                    variants,
                                    ..
                                } => {
                                    // 调试输出路径：Multiple variants 保证 ≥1，
                                    // 但防御性处理避免极端情况下 ICE
                                    let Some(v0) = variants.iter().next() else {
                                        eprintln!("[forge] warn: enum with zero variants");
                                        return Ok(self.builder.iconst(0, TypeId::I64));
                                    };
                                    let mut vf = Vec::new();
                                    for i in 0..v0.field_offsets.len() {
                                        vf.push(v0.field_offsets[FieldIdx::new(i)].bytes());
                                    }
                                    let te = match tag_encoding {
                                        rustc_abi::TagEncoding::Direct => "Direct".to_string(),
                                        rustc_abi::TagEncoding::Niche {
                                            untagged_variant,
                                            niche_start,
                                            ..
                                        } => format!(
                                            "Niche(untagged={}, start={})",
                                            untagged_variant.index(),
                                            niche_start
                                        ),
                                    };
                                    eprintln!(
                                        "[forge]   tag_encoding={te} variants[0].fields={vf:?} tag_field={}",
                                        tag_field.index()
                                    );
                                    Some(tag_field.index())
                                }
                                _ => None,
                            };
                            eprintln!(
                                "[forge] Discriminant DIRECT ty={ty} tag_field={tf:?} fields={fo:?} size={}",
                                layout.layout.size().bytes()
                            );
                        }
                        // tag 宽度 = ScalarPair 第 1 标量（tag）的真实位宽
                        //（u8/u16/u32）：discriminant_ty 返回"判别值类型"（无
                        // repr 枚举可能是 isize 8 字节），load 8 字节会读 tag 槽
                        // 后相邻内存（垃圾高位 → case 判错 → unreachable/ud2，
                        // enum_payload 实证）；固定 I32 读会带 ScalarPair 的
                        // pad 字节（Option<i32> tag u8@0 + pad@1..4，pad 是
                        // 返回寄存器残留垃圾 → nested_loop_break_outer 挂起
                        // 实证）。非 ScalarPair（普通 enum tag 紧凑无 pad）
                        // fallback I32（保持既有行为）。
                        let tag_bits = match &layout.layout.backend_repr {
                            rustc_abi::BackendRepr::ScalarPair { a, .. } => {
                                a.primitive().size(&self.tcx).bits()
                            }
                            _ => 32,
                        };
                        let tag_t = match tag_bits {
                            8 => TypeId::I8,
                            16 => TypeId::I16,
                            32 => TypeId::I32,
                            _ => TypeId::I64,
                        };
                        let tag = self.builder.load(addr, tag_t);
                        Ok(self.builder.uextend(tag, TypeId::I64))
                    }
                    rustc_abi::Variants::Multiple {
                        tag_encoding:
                            rustc_abi::TagEncoding::Niche {
                                untagged_variant,
                                niche_variants,
                                niche_start,
                            },
                        ..
                    } => {
                        if crate::trace::trace_enabled("DISCR") {
                            eprintln!(
                                "[forge] Discriminant NICHE ty={ty} untagged={} start={} niche_variants={:?}",
                                untagged_variant.index(),
                                niche_start,
                                niche_variants
                                    .clone()
                                    .into_iter()
                                    .map(|v| v.index())
                                    .collect::<Vec<_>>()
                            );
                        }
                        // WA-26：niche 判别读的字段偏移——niche 值在 payload 的
                        // 标量中（通常第 2 标量，如 Option<(usize,&i32)> 的 &i32
                        // 在 offset 8）。恒读 offset 0 会读到非 niche 标量
                        // （usize=0 → 误判 None，mn2 实证：判别恒 0 → 走 None
                        // 分支）。仅当 ScalarPair 的 b 是指针类（niche 在 b，
                        // 如 &i32/&T/ptr 用 null 做判别）时用 b_offset；
                        // 其他 niche（单标量或 b 非指针）读 offset 0——否则
                        // Result/ControlFlow 等非指针 niche 被错读（vl3 实证：
                        // grow 链 Result<_, TryReserveError> 的 niche 不在
                        // b_offset，读错 → 解引用垃圾 SEGV）。
                        let niche_off = match &layout.layout.backend_repr {
                            rustc_abi::BackendRepr::ScalarPair { b, b_offset, .. } => {
                                let is_ptr =
                                    matches!(b.primitive(), rustc_abi::Primitive::Pointer(_));
                                if is_ptr { b_offset.bytes() as i64 } else { 0 }
                            }
                            _ => 0,
                        };
                        let raw = if niche_off == 0 {
                            self.builder.load(addr, TypeId::I64)
                        } else {
                            let off_v = self.builder.iconst(niche_off, TypeId::I64);
                            let naddr = self.builder.iadd(addr, off_v);
                            self.builder.load(naddr, TypeId::I64)
                        };
                        // 常见单 niche（count==1 且 niche 值 0）：判别 = is_eq ? 0 : untagged
                        //（原生 Select：主库 [lower.Select] test+mov+cmovcc 已由
                        // test_jit_select_strict_matrix 严格矩阵验证无 bug——
                        // 变量臂/任意 cond/链式/多 XReg 压力全过，2026-08-31 nightly）
                        let niche_count = niche_variants.clone().into_iter().count();
                        if niche_count == 1 && *niche_start == 0 {
                            let z = self.builder.iconst(0, TypeId::I64);
                            let is_eq = self.builder.icmp(IntCC::Equal, raw, z);
                            let ut = self
                                .builder
                                .iconst(untagged_variant.index() as i64, TypeId::I32);
                            let z32 = self.builder.iconst(0, TypeId::I32);
                            let disc = self.builder.select(is_eq, z32, ut);
                            // 同 Direct：扩展 I32 → I64（isize 语义，防 switchInt
                            // 8 字节比较读到槽高 4 字节垃圾）
                            Ok(self.builder.uextend(disc, TypeId::I64))
                        } else {
                            // rustc codegen_get_discr 的公式：
                            //   relative = raw - niche_start（wrapping）
                            //   is_niche = relative ule relative_max
                            //   discr = is_niche ? (relative + variants.start) : untagged
                            let relative_max =
                                niche_variants.last.as_u32() - niche_variants.start.as_u32();
                            let start_v = self.builder.iconst(*niche_start as i64, TypeId::I64);
                            let relative = self.builder.isub(raw, start_v);
                            let max_v = self.builder.iconst(relative_max as i64, TypeId::I64);
                            let is_niche =
                                self.builder
                                    .icmp(IntCC::UnsignedLessThanOrEqual, relative, max_v);
                            let tagged = {
                                let rel32 = self.builder.ireduce(relative, TypeId::I32);
                                let start_idx = self
                                    .builder
                                    .iconst(niche_variants.start.as_u32() as i64, TypeId::I32);
                                self.builder.iadd(rel32, start_idx)
                            };
                            let untagged_v = self
                                .builder
                                .iconst(untagged_variant.index() as i64, TypeId::I32);
                            // 原生 Select（2026-08-31 恢复）：主库 [lower.Select]
                            // 严格矩阵验证通过（变量臂/任意 cond/链式/多 XReg）。
                            let disc = self.builder.select(is_niche, tagged, untagged_v);
                            Ok(self.builder.uextend(disc, TypeId::I64))
                        }
                    }
                    _ => {
                        let tag_bits = match &layout.layout.backend_repr {
                            rustc_abi::BackendRepr::ScalarPair { a, .. } => {
                                a.primitive().size(&self.tcx).bits()
                            }
                            _ => 32,
                        };
                        let tag_t = match tag_bits {
                            8 => TypeId::I8,
                            16 => TypeId::I16,
                            32 => TypeId::I32,
                            _ => TypeId::I64,
                        };
                        let tag = self.builder.load(addr, tag_t);
                        Ok(self.builder.uextend(tag, TypeId::I64))
                    }
                }
            }
            Rvalue::Repeat(op, _len) => self.lower_operand(op),
            Rvalue::Aggregate(_kind, _fields) => {
                // 结构体/元组构造 → 返回 0 占位
                Ok(self.builder.iconst_i32(0))
            }
            _ => Err(ForgeError::Message(format!(
                "unsupported rvalue: {:?}",
                rvalue
            ))),
        }
    }
    pub(crate) fn lower_operand(&mut self, operand: &Operand<'tcx>) -> Result<Value, ForgeError> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => {
                // 标量 place 读取：计算地址后 load（聚合整体移动暂不支持，
                // 由 move/copy 的 place 投影场景逐步处理）
                self.load_place(place)
            }
            Operand::Constant(constant) => {
                let ty = map_type(constant.const_.ty(), self.tcx)?;
                // 静态数据引用（const {allocN}：&static 或 &"str"）：值是
                // 指向数据段的指针——生成 global_addr（对象文件 .data/.rodata
                // 符号经 "G{N}" 重定位解析）。
                // 静态数据引用（const {allocN}：&static 或 &"str"）：值是
                // 指向数据段的指针——生成 global_addr（对象文件 .data/.rodata
                // 符号经 "G{N}" 重定位解析）。
                // 两种形态：ConstValue::Indirect{..}（旧）或
                // ConstValue::Scalar(Scalar::Ptr(..))（新 nightly）
                // 类型守卫：仅引用/指针类型走 static/promoted global_addr——
                // Layout 等聚合值（Indirect）必须保持字节求值（否则 vec_push
                // 的 LAYOUT 被当 promoted 引用 → 值错回归）。
                use rustc_middle::ty::TyKind;
                let is_ref_ty = matches!(
                    constant.const_.ty().kind(),
                    TyKind::Ref(..) | TyKind::RawPtr(..)
                );
                // Unevaluated（如 promoted 数组 `&[1,2,3]` = const promoted[0]）：
                // 先求值再按形态提取（引用类型的 promoted 是 Ptr/Indirect）
                let eval_cv = if is_ref_ty {
                    match constant.const_ {
                        rustc_middle::mir::Const::Unevaluated(..) => {
                            let r = constant.const_.eval(
                                self.tcx,
                                ty::TypingEnv::fully_monomorphized(),
                                rustc_span::DUMMY_SP,
                            );
                            if crate::trace::trace_enabled("CONST") {
                                eprintln!(
                                    "[forge] operand Unevaluated eval -> {:?}",
                                    r.as_ref().map(|v| format!("{v:?}"))
                                );
                            }
                            r.ok()
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                let alloc_id = if is_ref_ty {
                    match eval_cv {
                        Some(rustc_middle::mir::ConstValue::Indirect { alloc_id, .. }) => {
                            if crate::trace::trace_enabled("GLOBAL") {
                                eprintln!("[forge] const Indirect alloc={alloc_id:?}");
                            }
                            Some(alloc_id)
                        }
                        Some(rustc_middle::mir::ConstValue::Scalar(
                            rustc_middle::mir::interpret::Scalar::Ptr(ptr, _),
                        )) => {
                            if crate::trace::trace_enabled("GLOBAL") {
                                eprintln!("[forge] const Ptr prov={:?}", ptr.provenance);
                            }
                            Some(ptr.provenance.alloc_id())
                        }
                        _ => match constant.const_ {
                            rustc_middle::mir::Const::Val(
                                rustc_middle::mir::ConstValue::Indirect { alloc_id, .. },
                                _,
                            ) => {
                                if crate::trace::trace_enabled("GLOBAL") {
                                    eprintln!("[forge] const Indirect alloc={alloc_id:?}");
                                }
                                Some(alloc_id)
                            }
                            rustc_middle::mir::Const::Val(
                                rustc_middle::mir::ConstValue::Scalar(
                                    rustc_middle::mir::interpret::Scalar::Ptr(ptr, _),
                                ),
                                _,
                            ) => {
                                if crate::trace::trace_enabled("GLOBAL") {
                                    eprintln!("[forge] const Ptr prov={:?}", ptr.provenance);
                                }
                                Some(ptr.provenance.alloc_id())
                            }
                            _ => None,
                        },
                    }
                } else {
                    None
                };
                if let Some(alloc_id) = alloc_id {
                    // alloc_id → 数据段符号：promoted/slice 字面量是
                    // GlobalAlloc::Memory（intern_promoted 落盘 .rodata）、
                    // static 是 GlobalAlloc::Static（intern_global 落盘 .data）。
                    // 其余（Function 等）不在此路径，落到下方标量求值。
                    let g = match self.tcx.global_alloc(alloc_id) {
                        rustc_middle::mir::interpret::GlobalAlloc::Memory(alloc) => {
                            let inner = &*alloc.0;
                            let size = inner.size().bytes_usize();
                            let bytes = inner
                                .inspect_with_uninit_and_ptr_outside_interpreter(0..size)
                                .to_vec();
                            let align = inner.align.bytes();
                            let sym = self.slice_sym(alloc_id);
                            if crate::trace::trace_enabled("GLOBAL") {
                                eprintln!(
                                    "[forge] promoted alloc={alloc_id:?} size={size} -> sym={sym}"
                                );
                            }
                            Some(self.func_refs.intern_promoted(alloc_id, &sym, bytes, align))
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
                            Some(self.func_refs.intern_global(alloc_id, &sym))
                        }
                        _ => {
                            if crate::trace::trace_enabled("GLOBAL") {
                                eprintln!("[forge] const alloc={alloc_id:?} non-data, skip");
                            }
                            None
                        }
                    };
                    if let Some(g) = g {
                        if crate::trace::trace_enabled("GLOBAL") {
                            eprintln!("[forge] global_addr alloc={alloc_id:?} -> G{g}");
                        }
                        return Ok(self.builder.global_addr(GlobalId(g)));
                    }
                }
                // 求值标量常量：用 ScalarInt 自身位宽取位（s.size()），再按
                // 类型位宽分派——不能对 size 1/2/4 的标量调 to_i64()（固定 8
                // 字节，size<8 时 rustc 断言 ICE：expected int of size 8）。
                // 先试已求值标量；未求值常量（如
                // `<i32 as SizedTypeProperties>::ALIGN/SIZE` trait 关联常量）
                // 用 tcx 求值（TypingEnv::fully_monomorphized 适合单态化
                // codegen），否则 unwrap_or(0) 会把 ALIGN/SIZE 错误地当 0，
                // 导致 misaligned/null 检查链全部算错。
                let scalar = constant.const_.try_to_scalar_int().or_else(|| {
                    constant
                        .const_
                        .try_eval_scalar_int(self.tcx, ty::TypingEnv::fully_monomorphized())
                });
                if ty.is_float() {
                    // 浮点常量：位模式经 Fconst 加载到 FPR——iconst 走 GPR
                    // mov_imm，fadd/fcmp 等 FPR 运算会读到 GPR 里的垃圾值
                    let bits = scalar.map(|s| s.to_bits(s.size())).unwrap_or(0);
                    if crate::trace::trace_enabled("CONST") {
                        eprintln!("[forge] fconst ty={ty} bits={bits:#x} scalar={scalar:?}");
                    }
                    return Ok(self.builder.fconst(bits as u64, ty));
                }
                let val = scalar
                    .map(|s| {
                        let bits = s.to_bits(s.size());
                        // WA-36：按 rustc 类型符号性扩展窄常量——u8/u16 常量
                        // 零扩展（位模式：0xFC→252），i8/i16 符号扩展（0xFC→-4）。
                        // 原实现恒 signed sext：u8 常量 0xFC 变 -4（全 F），与
                        // load（movzx 零扩展 252）icmp 不等（u8 高字节常量
                        // 比较误判实证）。等宽（i32/i64）两侧扩展一致无差。
                        let signed = constant.const_.ty().is_signed();
                        match s.size().bytes() {
                            1 if signed => (bits as u8 as i8) as i64,
                            1 => bits as u8 as i64,
                            2 if signed => (bits as u16 as i16) as i64,
                            2 => bits as u16 as i64,
                            4 => (bits as u32 as i32) as i64,
                            _ => bits as i64,
                        }
                    })
                    .unwrap_or(0);
                if crate::trace::trace_enabled("CONST") {
                    eprintln!("[forge] iconst ty={ty} val={val} scalar={scalar:?}");
                }
                Ok(self.builder.iconst(val, ty))
            }
            Operand::RuntimeChecks(_) => Ok(self.builder.iconst_i32(0)),
        }
    }
    pub(crate) fn lower_checked_op(
        &mut self,
        op: mir::BinOp,
        lhs: Value,
        rhs: Value,
        signed: bool,
    ) -> Result<(Value, Value), ForgeError> {
        let (v, f) = match op {
            mir::BinOp::AddWithOverflow => {
                if signed {
                    self.builder.sadd_overflow(lhs, rhs)
                } else {
                    self.builder.uadd_overflow(lhs, rhs)
                }
            }
            mir::BinOp::SubWithOverflow => {
                if signed {
                    self.builder.ssub_overflow(lhs, rhs)
                } else {
                    self.builder.usub_overflow(lhs, rhs)
                }
            }
            mir::BinOp::MulWithOverflow => {
                if signed {
                    self.builder.smul_overflow(lhs, rhs)
                } else {
                    self.builder.umul_overflow(lhs, rhs)
                }
            }
            _ => {
                return Err(ForgeError::Message(format!(
                    "unsupported checked op: {:?}",
                    op
                )));
            }
        };
        Ok((v, f))
    }
    pub(crate) fn lower_binary_op(
        &mut self,
        op: mir::BinOp,
        lhs: Value,
        rhs: Value,
        signed: bool,
        op1_ty: Ty<'tcx>,
    ) -> Result<Value, ForgeError> {
        let is_fp = op1_ty.is_floating_point();
        // WA-36：整数 icmp 窄域（rustc i8/u8/i16/u16）统一扩展——x86
        // cmp 走 32/64 位域，未扩展窄位模式无法比较（i8 -50(0xCE) movzx
        // 后 +206 被当正数、signed < 错；常量侧已按类型扩展，值侧 load
        // 是 movzx 零扩展——两侧扩展方式不一致则 Eq/比较错）。比较前把
        // 窄操作数按 signed 语义 sext/zext 到 64（movsx/movzx 链）。
        let cmp_op = matches!(
            op,
            mir::BinOp::Eq
                | mir::BinOp::Ne
                | mir::BinOp::Lt
                | mir::BinOp::Le
                | mir::BinOp::Gt
                | mir::BinOp::Ge
        );
        let (lhs, rhs) = if cmp_op && !is_fp {
            let narrow_w = match op1_ty.kind() {
                rustc_middle::ty::TyKind::Int(_) | rustc_middle::ty::TyKind::Uint(_) => {
                    layout_bytes(self.tcx, op1_ty)
                }
                _ => 4,
            };
            if narrow_w < 4 {
                let mut ext = |v: Value| -> Value {
                    if signed {
                        self.builder.sextend(v, TypeId::I64)
                    } else {
                        self.builder.uextend(v, TypeId::I64)
                    }
                };
                let e1 = ext(lhs);
                let e2 = ext(rhs);
                (e1, e2)
            } else {
                (lhs, rhs)
            }
        } else {
            (lhs, rhs)
        };
        match op {
            mir::BinOp::Add | mir::BinOp::AddWithOverflow | mir::BinOp::AddUnchecked => {
                if is_fp {
                    Ok(self.builder.fadd(lhs, rhs))
                } else {
                    Ok(self.builder.iadd(lhs, rhs))
                }
            }
            mir::BinOp::Sub | mir::BinOp::SubWithOverflow | mir::BinOp::SubUnchecked => {
                if is_fp {
                    Ok(self.builder.fsub(lhs, rhs))
                } else {
                    Ok(self.builder.isub(lhs, rhs))
                }
            }
            mir::BinOp::Mul | mir::BinOp::MulWithOverflow | mir::BinOp::MulUnchecked => {
                if is_fp {
                    Ok(self.builder.fmul(lhs, rhs))
                } else {
                    Ok(self.builder.imul(lhs, rhs))
                }
            }
            mir::BinOp::Div => {
                if is_fp {
                    Ok(self.builder.fdiv(lhs, rhs))
                } else if signed {
                    Ok(self.builder.sdiv(lhs, rhs))
                } else {
                    Ok(self.builder.udiv(lhs, rhs))
                }
            }
            mir::BinOp::Rem => {
                if signed {
                    Ok(self.builder.srem(lhs, rhs))
                } else {
                    Ok(self.builder.urem(lhs, rhs))
                }
            }
            mir::BinOp::BitAnd => Ok(self.builder.band(lhs, rhs)),
            mir::BinOp::BitOr => Ok(self.builder.bor(lhs, rhs)),
            mir::BinOp::BitXor => Ok(self.builder.bxor(lhs, rhs)),
            mir::BinOp::Shl | mir::BinOp::ShlUnchecked => Ok(self.builder.ishl(lhs, rhs)),
            mir::BinOp::Shr | mir::BinOp::ShrUnchecked => {
                // 有符号 >> 是算术右移，无符号 >> 是逻辑右移
                if signed {
                    Ok(self.builder.sshr(lhs, rhs))
                } else {
                    Ok(self.builder.ushr(lhs, rhs))
                }
            }
            mir::BinOp::Offset => {
                // ptr.offset(count)：字节偏移 = count * size_of::<T>()（ZST noop）
                let pointee = op1_ty.builtin_deref(true).unwrap_or(op1_ty);
                let elem_size = layout_bytes(self.tcx, pointee) as i64;
                if elem_size == 0 {
                    Ok(lhs)
                } else if elem_size == 1 {
                    Ok(self.builder.iadd(lhs, rhs))
                } else {
                    let sz = self.builder.iconst(elem_size, TypeId::I64);
                    let scaled = self.builder.imul(rhs, sz);
                    Ok(self.builder.iadd(lhs, scaled))
                }
            }
            mir::BinOp::Eq => {
                // 浮点相等必须走 fcmp（ucomisd），icmp 只支持整数
                if is_fp {
                    Ok(self.builder.fcmp(FloatCC::Equal, lhs, rhs))
                } else {
                    Ok(self.builder.icmp(IntCC::Equal, lhs, rhs))
                }
            }
            mir::BinOp::Ne => {
                if is_fp {
                    Ok(self.builder.fcmp(FloatCC::NotEqual, lhs, rhs))
                } else {
                    Ok(self.builder.icmp(IntCC::NotEqual, lhs, rhs))
                }
            }
            mir::BinOp::Lt => {
                if is_fp {
                    Ok(self.builder.fcmp(FloatCC::LessThan, lhs, rhs))
                } else if signed {
                    Ok(self.builder.icmp(IntCC::SignedLessThan, lhs, rhs))
                } else {
                    Ok(self.builder.icmp(IntCC::UnsignedLessThan, lhs, rhs))
                }
            }
            mir::BinOp::Le => {
                if is_fp {
                    Ok(self.builder.fcmp(FloatCC::LessThanOrEqual, lhs, rhs))
                } else if signed {
                    Ok(self.builder.icmp(IntCC::SignedLessThanOrEqual, lhs, rhs))
                } else {
                    Ok(self.builder.icmp(IntCC::UnsignedLessThanOrEqual, lhs, rhs))
                }
            }
            mir::BinOp::Gt => {
                if is_fp {
                    Ok(self.builder.fcmp(FloatCC::GreaterThan, lhs, rhs))
                } else if signed {
                    Ok(self.builder.icmp(IntCC::SignedGreaterThan, lhs, rhs))
                } else {
                    Ok(self.builder.icmp(IntCC::UnsignedGreaterThan, lhs, rhs))
                }
            }
            mir::BinOp::Ge => {
                if is_fp {
                    Ok(self.builder.fcmp(FloatCC::GreaterThanOrEqual, lhs, rhs))
                } else if signed {
                    Ok(self.builder.icmp(IntCC::SignedGreaterThanOrEqual, lhs, rhs))
                } else {
                    Ok(self
                        .builder
                        .icmp(IntCC::UnsignedGreaterThanOrEqual, lhs, rhs))
                }
            }
            _ => Err(ForgeError::Message(format!(
                "unsupported binary op: {:?}",
                op
            ))),
        }
    }
    /// fat pointer 的 metadata（len）：读 place 槽 [base+8]（fat ptr 布局
    /// ptr@0 + metadata@8）。仅当 place 类型是 fat pointer（&str/&[T]/
    /// dyn Trait）时调用；thin 指针返回 None（PtrMetadata 恒 0）。
    pub(crate) fn fat_ptr_metadata(&mut self, place: &mir::Place<'tcx>) -> Option<Value> {
        use rustc_middle::ty::TyKind;
        let ty = place.ty(&self.body.local_decls, self.tcx).ty;
        let is_fat = match ty.kind() {
            TyKind::Ref(_, t, _) | TyKind::RawPtr(t, _) => matches!(
                t.kind(),
                TyKind::Slice(..) | TyKind::Dynamic(..) | TyKind::Str
            ),
            _ => false,
        };
        if !is_fat {
            if crate::trace::trace_enabled("META") {
                eprintln!("[forge] fat_ptr_metadata NOT-FAT ty={ty} place={place:?}");
            }
            return None;
        }
        if crate::trace::trace_enabled("META") {
            eprintln!("[forge] fat_ptr_metadata ty={ty} place={place:?}");
        }
        let base = self.place_addr(place);
        let eight = self.builder.iconst(8, TypeId::I64);
        let hi = self.builder.iadd(base, eight);
        Some(self.builder.load(hi, TypeId::I64))
    }

    pub(crate) fn lower_unary_op(
        &mut self,
        op: mir::UnOp,
        val: Value,
        is_bool: bool,
    ) -> Result<Value, ForgeError> {
        match op {
            mir::UnOp::Not => {
                // bool 的 ! 是翻转 0/1（bnot 会产生 -1，作为真值判定会出错）
                if is_bool {
                    let zero = self.builder.iconst(0, TypeId::BOOL);
                    Ok(self.builder.icmp(IntCC::Equal, val, zero))
                } else {
                    Ok(self.builder.bnot(val))
                }
            }
            mir::UnOp::Neg => {
                let zero = self.builder.iconst_i32(0);
                Ok(self.builder.isub(zero, val))
            }
            mir::UnOp::PtrMetadata => {
                // 指针元数据：thin 指针的 metadata 为空（值无关紧要，恒 0）
                Ok(self.builder.iconst(0, TypeId::I64))
            }
        }
    }
    pub(crate) fn load_ty(&mut self, addr: Value, ty: TypeId) -> Value {
        if ty == TypeId::BOOL {
            // WA-22 读侧对称：主库对 BOOL 的 Load 无窄规则（TOML 仅 32/64
            // 两档，BOOL bits=1 走默认 64 位 mov_mem）——聚合内字段（如
            // (i32,bool) 的 flag@+4）8 字节读越界到相邻槽（bump1 实证：
            // 读 -0x54..-0x4d 混入 _1 参数槽的 p → testq 非零误判溢出 →
            // 走 panic 路径 call panic_handler 死循环）。显式 32 位读
            // （写侧已 ireduce 到 I32 + mov_sto32，槽内 4 字节恒 0/1）
            // 再 ireduce 回 BOOL，语义与窄读等价且不越界。
            let v32 = self.builder.load(addr, TypeId::I32);
            return self.builder.ireduce(v32, TypeId::BOOL);
        }
        if ty.is_float() {
            self.builder.fload(addr, ty)
        } else {
            self.builder.load(addr, ty)
        }
    }
    pub(crate) fn store_ty(&mut self, val: Value, addr: Value, ty: TypeId) {
        if ty.is_float() {
            self.builder.fstore(val, addr)
        } else {
            self.builder.store(val, addr)
        }
    }
}
