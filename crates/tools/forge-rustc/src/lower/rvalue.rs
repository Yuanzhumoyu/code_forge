//! Rvalue / Operand / 二元一元运算 lowering。
use super::*;

impl<'tcx, 'f> LowerCtxt<'tcx, 'f> {
    pub(crate) fn lower_rvalue(&mut self, rvalue: &Rvalue<'tcx>) -> Result<Value, ForgeError> {
        match rvalue {
            Rvalue::Use(op, _) => self.lower_operand(op),
            Rvalue::BinaryOp(bin_op, box (op1, op2)) => {
                let lhs = self.lower_operand(op1)?;
                let rhs = self.lower_operand(op2)?;
                // 符号性决定 sdiv/udiv、srem/urem、sshr/ushr 的选择
                // （forge-ir 的 Int 类型不带符号位，必须从 MIR 操作数类型推导）
                let op1_ty = op1.ty(&self.body.local_decls, self.tcx);
                let signed = op1_ty.is_signed();
                self.lower_binary_op(*bin_op, lhs, rhs, signed, op1_ty)
            }
            Rvalue::UnaryOp(un_op, op) => {
                let val = self.lower_operand(op)?;
                let is_bool = op.ty(&self.body.local_decls, self.tcx).is_bool();
                self.lower_unary_op(*un_op, val, is_bool)
            }
            Rvalue::Cast(cast_kind, op, to_ty) => {
                let mut val = self.lower_operand(op)?;
                let to_type = map_type(*to_ty, self.tcx)?;
                match cast_kind {
                    rustc_middle::mir::CastKind::IntToInt
                    | rustc_middle::mir::CastKind::FloatToInt
                    | rustc_middle::mir::CastKind::IntToFloat => {
                        // 窄类型零扩展 mask（u8/u16/bool → 更宽）：
                        // 主库 load/算术按 32 位"寄存器安全"宽度（opsize 折中），
                        // u8 值的高 24 位是栈槽残留垃圾（write_bytes_loop 曾把
                        // buf[0] 读成 0xABABABAB）。无符号源在参与 64 位运算前
                        // mask 低 8/16 位清零高位；有符号源的符号扩展依赖
                        // ireduce 后的使用（e2e 暂无 i8 直接扩展用例）。
                        let src_ty = op.ty(&self.body.local_decls, self.tcx);
                        if matches!(
                            src_ty.kind(),
                            rustc_middle::ty::TyKind::Int(_)
                                | rustc_middle::ty::TyKind::Uint(_)
                                | rustc_middle::ty::TyKind::Bool
                        ) && !src_ty.is_signed()
                        {
                            let sz = src_ty.primitive_size(self.tcx);
                            if sz.bytes() < 4 {
                                let mask = (1u64 << (sz.bytes() * 8)) - 1;
                                let m = self.builder.iconst(mask as i64, TypeId::I64);
                                val = self.builder.band(val, m);
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
                        Ok(self.builder.load(addr, TypeId::I32))
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
                        let raw = self.builder.load(addr, TypeId::I64);
                        // 常见单 niche（count==1 且 niche 值 0）：判别 = untagged - is_eq*untagged
                        //（算术避免 select 两臂常量的 XReg 重叠分配——regalloc 已知问题）
                        let niche_count = niche_variants.clone().into_iter().count();
                        if niche_count == 1 && *niche_start == 0 {
                            let z = self.builder.iconst(0, TypeId::I64);
                            let is_eq = self.builder.icmp(IntCC::Equal, raw, z);
                            let ut = self
                                .builder
                                .iconst(untagged_variant.index() as i64, TypeId::I32);
                            let scaled = self.builder.imul(is_eq, ut);
                            let r = self.builder.isub(ut, scaled);
                            Ok(r)
                        } else {
                            // rustc codegen_get_discr 的算术公式（规避主库
                            // [lower.Select]="mov rd,rs2" 无条件返回 true 臂的
                            // bug——niche 通用循环的 select 在 cf5 全错）：
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
                            // 2026-08：尝试恢复原生 Select（builder.select）验证主库
                            // [lower.Select] 修复（test+cmovne）——nested_enum_break
                            // 仍 SEGV（cond 位宽/cmovne 路径未达预期），回滚算术公式
                            // 规避；Select 修复保留在 isa TOML（框架正确性），待 Phase 1
                            // 继续调查 cmovne 路径后再恢复。
                            let one = self.builder.iconst(1, TypeId::I32);
                            let not_niche = self.builder.isub(one, is_niche);
                            let t1 = self.builder.imul(tagged, is_niche);
                            let t2 = self.builder.imul(untagged_v, not_niche);
                            Ok(self.builder.iadd(t1, t2))
                        }
                    }
                    _ => Ok(self.builder.load(addr, TypeId::I32)),
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
                let alloc_id = match constant.const_ {
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
                };
                if let Some(alloc_id) = alloc_id {
                    // alloc_id → static 符号名
                    if let rustc_middle::mir::interpret::GlobalAlloc::Static(def_id) =
                        self.tcx.global_alloc(alloc_id)
                    {
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
                        let g = self.func_refs.intern_global(alloc_id, &sym);
                        if crate::trace::trace_enabled("GLOBAL") {
                            eprintln!("[forge] global_addr alloc={alloc_id:?} -> G{g} sym={sym}");
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
                        match s.size().bytes() {
                            1 => (bits as u8 as i8) as i64,
                            2 => (bits as u16 as i16) as i64,
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
