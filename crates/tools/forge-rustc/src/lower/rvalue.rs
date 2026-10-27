//! Rvalue / Operand / 二元一元运算 lowering。
use super::*;

impl<'tcx, 'f> LowerCtxt<'tcx, 'f> {
    pub(crate) fn lower_rvalue(&mut self, rvalue: &Rvalue<'tcx>) -> Result<Value, ForgeError> {
        match rvalue {
            Rvalue::Use(op, _) => self.lower_operand(op),
            Rvalue::BinaryOp(bin_op, (op1, op2)) => {
                // **宽整数 ÷ 2 的幂常量 ⇒ 直接发移位/掩码**（v27 M6-70）。
                //
                // 128 位的 `Div`/`Rem` 在 isel 里没有规则（`wide_split` 明确 fail-closed），
                // 只有"除数是 2 的幂"能**无关 ABI**地改写成算术：
                //   `x / 2^k` ⇒ `x >> k`；`x % 2^k` ⇒ `x & (2^k - 1)`
                // 为什么放在降级层而不是 legalize 的 pass：pass 看到的已经是 SSA 值，
                // `-O2` 下 rustc 会把除数常量提升/先 load 进值 ⇒ 认不出"常量"；
                // 这里 `op2` 还拿着 `Operand::Constant`，是唯一能稳定识别的层次。
                // 有符号的除法舍入（向零取整）需要额外修正，本轮只做**无符号**；
                // 其余（非 2 的幂、有符号）继续 fail-closed。
                let bop_ty = op1.ty(&self.body.local_decls, self.tcx);
                let wide = matches!(
                    bop_ty.kind(),
                    rustc_middle::ty::TyKind::Int(ty::IntTy::I128)
                        | rustc_middle::ty::TyKind::Uint(ty::UintTy::U128)
                );
                if wide
                    && !bop_ty.is_signed()
                    && matches!(bin_op, mir::BinOp::Div | mir::BinOp::Rem)
                    && let Some(pow2) = self.mir_const_pow2(op2)
                {
                    let x = self.lower_operand(op1)?;
                    let k = pow2.trailing_zeros();
                    if k == 0 {
                        // ÷1 / %1
                        return Ok(if matches!(bin_op, mir::BinOp::Rem) {
                            self.builder.const_int(
                                0i128,
                                map_type(bop_ty, self.tcx).unwrap_or(TypeId::I128),
                            )
                        } else {
                            x
                        });
                    }
                    let ty = map_type(bop_ty, self.tcx).unwrap_or(TypeId::I128);
                    return Ok(if matches!(bin_op, mir::BinOp::Rem) {
                        let mask = self.builder.const_int((pow2 - 1) as i128, ty);
                        self.builder.band(x, mask)
                    } else {
                        // 移位量必须与 `x` **同类型**（`Ushr` 两侧位宽一致，否则 IR 校验不过）：
                        // 宽值就是 i128 常量，别写成 I64（实测 `TypeMismatch: Ushr 期望 t10`）。
                        let sh = self.builder.const_int(i128::from(k), ty);
                        self.builder.ushr(x, sh)
                    });
                }
                let lhs = self.lower_operand(op1)?;
                let rhs = self.lower_operand(op2)?;
                // 符号性决定 sdiv/udiv、srem/urem、sshr/ushr 的选择
                // （forge-ir 的 Int 类型不带符号位，必须从 MIR 操作数类型推导）
                let op1_ty = op1.ty(&self.body.local_decls, self.tcx);
                let op2_ty = op2.ty(&self.body.local_decls, self.tcx);
                let signed = op1_ty.is_signed();
                // **两侧 MIR 类型不同就先对齐 IR 宽度**：常量操作数的 IR 类型由
                // `map_type(常量类型)` 决定，可能与另一侧不同（实测：`b[0] == 7`
                // 一边是 `load i8`、另一边是 i32 ⇒ `icmp eq i8 %x, i32 7` 是非法 IR，
                // `-O` 下 verifier 直接拒绝）。按 MIR 类型判宽，把窄的一侧
                // zxt/sext 到宽的一侧（语义同 C 的整型提升）。
                let (lhs, rhs) = self.coerce_binop_operands(lhs, rhs, op1_ty, op2_ty, signed)?;
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
                let op_ty = op.ty(&self.body.local_decls, self.tcx);
                let wide_src = matches!(
                    op_ty.kind(),
                    rustc_middle::ty::TyKind::Int(rustc_middle::ty::IntTy::I128)
                        | rustc_middle::ty::TyKind::Uint(rustc_middle::ty::UintTy::U128)
                );
                let wide_dst = matches!(
                    to_ty.kind(),
                    rustc_middle::ty::TyKind::Int(rustc_middle::ty::IntTy::I128)
                        | rustc_middle::ty::TyKind::Uint(rustc_middle::ty::UintTy::U128)
                );
                if wide_src && !wide_dst {
                    if let rustc_middle::mir::Operand::Copy(pl)
                    | rustc_middle::mir::Operand::Move(pl) = op
                    {
                        let addr = self.place_addr(pl);
                        let to_type = map_type(*to_ty, self.tcx)?;
                        let lo = self.builder.load(addr, TypeId::I64);
                        let dst_bytes = crate::layout::layout_bytes(self.tcx, *to_ty);
                        return Ok(if dst_bytes < 8 {
                            self.builder.ireduce(lo, to_type)
                        } else {
                            lo
                        });
                    }
                }
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
                                // **掩码要用源类型的 IR 类型**：此前恒用 `I64` 常量 ⇒
                                // `Band(i8, i64)` 操作数类型不符（verifier `TypeMismatch`），
                                // 随后那条 `ireduce` 跟着报 `ConversionBitWidthMismatch
                                // t5 → t4`。v27 M6 实测：`buf[1] as i32`（源 u8）在 `-O` 下
                                // 直接编译失败。
                                let mask = (1u64 << (sz.bytes() * 8)) - 1;
                                let src_ir = map_type(src_ty, self.tcx)?;
                                let m = self.builder.const_int(mask as i64, src_ir);
                                val = self.builder.band(val, m);
                            }
                        }
                        // **按 IR 位宽选指令**：`ireduce` 只能**变窄**（sext/zext 只能变宽）。
                        // 原实现无条件 `ireduce` ⇒ 加宽转换（`bool`/`u8`/`i8`/`i32` → 更宽）
                        // 产出非法 IR（verifier: ConversionBitWidthMismatch `t1 → t4`），
                        // 只在优化管线真跑起来（v27 M5 接线）后才暴露。
                        // 位宽口径用 rustc 侧的布局字节数（`bool` 是 1 字节、`i32` 是 4 字节）：
                        // 它和 `map_type` 出来的 IR 类宽度同序，且不需要 IR 侧的宽度查询。
                        let s_bits = layout_bytes(self.tcx, src_ty);
                        let d_bits = layout_bytes(self.tcx, *to_ty);
                        Ok(if d_bits < s_bits {
                            self.builder.ireduce(val, to_type)
                        } else if d_bits > s_bits {
                            if is_int && src_ty.is_signed() {
                                self.builder.sextend(val, to_type)
                            } else {
                                self.builder.uextend(val, to_type)
                            }
                        } else {
                            // 同宽：值不变（窄类型的"高垃圾位"已在上面的 mask 分支清掉）
                            val
                        })
                    }
                    rustc_middle::mir::CastKind::PtrToPtr
                    | rustc_middle::mir::CastKind::FnPtrToPtr => Ok(val),
                    _ => Ok(val),
                }
            }
            // v27 TLS：`&TLS` / 读写 TLS 静态用的是**专门的 rvalue**（不是 const 路径）。
            // 以前它落到 `unsupported rvalue`（fail-closed，不是静默错值）；现在走动态 TLS。
            Rvalue::ThreadLocalRef(def_id) => self.tls_static_addr(*def_id),
            Rvalue::Ref(_, _, place) => {
                // &x / &mut x → 该 place 的栈地址
                Ok(self.place_addr(place))
            }
            Rvalue::RawPtr(_, place) => {
                // &raw const v / &v as *const → 该 place 的栈地址
                Ok(self.place_addr(place))
            }
            Rvalue::Discriminant(place) => {
                // 判别观测（DIscr）：入口处打印布局事实，避免被分支内的打印挡住
                if crate::trace::trace_enabled("DISCR") {
                    let dty = place.ty(&self.body.local_decls, self.tcx).ty;
                    if let Ok(dl) = self.tcx.layout_of(ty::PseudoCanonicalInput {
                        typing_env: ty::TypingEnv::fully_monomorphized(),
                        value: dty,
                    }) {
                        eprintln!(
                            "[forge] DISCR-ENTRY ty={dty} size={} repr={:?} variants={:?}",
                            dl.layout.size().bytes(),
                            dl.layout.backend_repr,
                            dl.layout.variants()
                        );
                    }
                }
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
                        tag_field,
                        variants,
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
                                        return Ok(self.builder.const_int(0, TypeId::I64));
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
                        // v27 M5 缺陷（`Range::contains` → ud2）：tag 宽度不能从
                        // `BackendRepr::ScalarPair { a, .. }` 猜——`Bound<&u32>` 的 `a` 是
                        // payload 指针（8B）、tag 是另一个标量 ⇒ 读 8 字节载荷去比 case 0
                        // ⇒ payload≠0 就落 otherwise（unreachable）⇒ ud2。
                        // 正确做法：tag 宽度按**变体数**（rustc 对 repr(Rust) 枚举的规则）。
                        let n_var = variants.len();
                        let tag_bits = if n_var <= 256 {
                            8
                        } else if n_var <= 65536 {
                            16
                        } else {
                            32
                        };
                        let tag_t = match tag_bits {
                            8 => TypeId::I8,
                            16 => TypeId::I16,
                            32 => TypeId::I32,
                            _ => TypeId::I64,
                        };
                        let tag_off = layout.layout.fields().offset((*tag_field).index()).bytes();
                        let tag_addr = if tag_off == 0 {
                            addr
                        } else {
                            let off = self.builder.const_int(tag_off as i64, TypeId::I64);
                            self.builder.gep(addr, &[off], TypeId::I8)
                        };
                        let tag = self.builder.load(tag_addr, tag_t);
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
                        // WA-26/WA-44：niche 判别读的字段偏移——与写侧（statement.rs）
                        // 共用 `niche_tag_offset`：ScalarPair 取与 tag 标量同类的那一半
                        // 的偏移（`Option<(usize,&i32)>` 的 &i32 在 offset 8；恒读
                        // offset 0 会读到 usize=0 → 误判 None，mn2 实证）；聚合 payload
                        // 用 `tag_field` 的字段偏移（niche 在非 0 偏移时不再读错）。
                        let niche_off = self.niche_tag_offset(&layout)?;
                        // v27 M5 缺陷（`char::from_u32` 错值）：niche 值必须按**它自己的宽度**读，
                        // 不能恒读 I64——`Result<char, CharTryFromError>` 只有 4 字节，
                        // 读 8 字节会把栈上 4 字节垃圾掺进 `relative`（`raw - niche_start`），
                        // 于是 `is_niche` 判错 ⇒ 无效码点被当成 `Ok`。
                        let niche_bits = match &layout.layout.backend_repr {
                            rustc_abi::BackendRepr::Scalar(s) => {
                                s.primitive().size(&self.tcx).bits()
                            }
                            rustc_abi::BackendRepr::ScalarPair { a, b, .. } => {
                                if niche_off == 0 {
                                    a.primitive().size(&self.tcx).bits()
                                } else {
                                    b.primitive().size(&self.tcx).bits()
                                }
                            }
                            _ => 64,
                        };
                        let niche_t = match niche_bits {
                            8 => TypeId::I8,
                            16 => TypeId::I16,
                            32 => TypeId::I32,
                            _ => TypeId::I64,
                        };
                        let raw_addr = if niche_off == 0 {
                            addr
                        } else {
                            let off_v = self.builder.const_int(niche_off, TypeId::I64);
                            self.builder.iadd(addr, off_v)
                        };
                        let raw_v = self.builder.load(raw_addr, niche_t);
                        let raw = if niche_bits >= 64 {
                            raw_v
                        } else {
                            self.builder.uextend(raw_v, TypeId::I64)
                        };
                        // 常见单 niche（count==1 且 niche 值 0）：判别 = is_eq ? 0 : untagged
                        //（原生 Select：主库 [lower.Select] test+mov+cmovcc 已由
                        // test_jit_select_strict_matrix 严格矩阵验证无 bug——
                        // 变量臂/任意 cond/链式/多 XReg 压力全过，2026-08-31 nightly）
                        let niche_count = niche_variants.clone().into_iter().count();
                        if niche_count == 1 && *niche_start == 0 {
                            let z = self.builder.const_int(0, TypeId::I64);
                            let is_eq = self.builder.icmp(IntCC::Eq, raw, z);
                            let ut = self
                                .builder
                                .const_int(untagged_variant.index() as i64, TypeId::I32);
                            let z32 = self.builder.const_int(0, TypeId::I32);
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
                            let start_v = self.builder.const_int(*niche_start as i64, TypeId::I64);
                            let relative = self.builder.isub(raw, start_v);
                            let max_v = self.builder.const_int(relative_max as i64, TypeId::I64);
                            let is_niche = self.builder.icmp(IntCC::Ule, relative, max_v);
                            let tagged = {
                                let rel32 = self.builder.ireduce(relative, TypeId::I32);
                                let start_idx = self
                                    .builder
                                    .const_int(niche_variants.start.as_u32() as i64, TypeId::I32);
                                self.builder.iadd(rel32, start_idx)
                            };
                            let untagged_v = self
                                .builder
                                .const_int(untagged_variant.index() as i64, TypeId::I32);
                            // 原生 Select（2026-08-31 恢复）：主库 [lower.Select]
                            // 严格矩阵验证通过（变量臂/任意 cond/链式/多 XReg）。
                            let disc = self.builder.select(is_niche, tagged, untagged_v);
                            Ok(self.builder.uextend(disc, TypeId::I64))
                        }
                    }
                    rustc_abi::Variants::Single { index } => {
                        // v27 M5 缺陷（`RangeInclusive::fold` ud2）：`Variants::Single` 表示
                        // **没有独立 tag**（如 `ControlFlow<NeverShortCircuitResidual, i32>`
                        // 被优化成 4 字节 `Scalar(i32)`）⇒ 判别值就是该唯一变体的索引（常量）。
                        // 此前这里按内存读 4 字节 ⇒ 把 payload 当判别 ⇒ `isize` 判别值不是 0/1
                        // ⇒ 落 `otherwise: unreachable` ⇒ ud2。
                        let idx = self.builder.const_int(index.index() as i64, TypeId::I64);
                        Ok(idx)
                    }
                    _ => {
                        // 其余形态（理论上不可达）：保持旧的"整 4 字节 @0"行为。
                        let tag = self.builder.load(addr, TypeId::I32);
                        Ok(self.builder.uextend(tag, TypeId::I64))
                    }
                }
            }
            Rvalue::Repeat(op, _len) => self.lower_operand(op),
            Rvalue::Aggregate(_kind, _fields) => {
                // 结构体/元组构造 → 返回 0 占位
                Ok(self.builder.constant(0i32))
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
                    // alloc_id → 数据段符号（M5 稳定键统一入口 intern_const_data）：
                    // promoted/slice 字面量是 GlobalAlloc::Memory（内容稳定名
                    // `__slice_{hash}`，intern_promoted 落 .rodata）、static 是
                    // GlobalAlloc::Static（真实符号名，intern_global 落 .data）。
                    // v27 TLS：线程局部静态在这里被拦成动态 TLS 调用（`const_data_addr`）。
                    if let Some(addr) = self.const_data_addr(alloc_id)? {
                        if crate::trace::trace_enabled("GLOBAL") {
                            eprintln!("[forge] global_addr alloc={alloc_id:?}（TLS 走运行时）");
                        }
                        return Ok(addr);
                    }
                    // **函数指针常量**（v27 M6-89）：`take as fn(..)` 的分配是
                    // `GlobalAlloc::Function`——不是数据段，`const_data_addr` 返回 `None`。
                    // 取该实例的**符号地址**：借隐藏全局符号表（`G{N}` → 真实符号名）
                    // 走 `GlobalAddr` 的 `abs64` 重定位，链接器把函数符号填进来。
                    // 此前这里落到标量求值 ⇒ 指针是 `null`（fn 指针一律 AV）。
                    if let rustc_middle::mir::interpret::GlobalAlloc::Function { instance } =
                        self.tcx.global_alloc(alloc_id)
                    {
                        if crate::trace::trace_enabled("GLOBAL") {
                            eprintln!("[forge] func_addr alloc={alloc_id:?} inst={instance:?}");
                        }
                        return Ok(self.fn_addr_value(&instance));
                    }
                    if crate::trace::trace_enabled("GLOBAL") {
                        eprintln!("[forge] const alloc={alloc_id:?} non-data, skip");
                    }
                }
                // `FnDef` 直接作为值（`take as fn(..)` 的另一形态：MIR 的常量类型就是
                // `FnDef`）：解析实例 → 符号地址（与上面 `GlobalAlloc::Function` 同路）。
                if let TyKind::FnDef(def_id, substs) = constant.const_.ty().kind()
                    && let Some(inst) = ty::Instance::try_resolve(
                        self.tcx,
                        ty::TypingEnv::fully_monomorphized(),
                        *def_id,
                        substs.skip_binder(),
                    )
                    .ok()
                    .flatten()
                {
                    return Ok(self.fn_addr_value(&inst));
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
                    return Ok(self.builder.const_float(bits as u64, ty));
                }
                // **128 位整数常量（v27 M5 · M-E）**：16 字节标量**不能**截成 `i64`
                // （下面那组 1/2/4/8 分支的 `_ => bits as i64` 会丢掉高 64 位）。
                // 走 `Big` 通道，由 `iconst_big` 生成真正的 i128 常量。
                if let Some(s) = scalar
                    && s.size().bytes() == 16
                {
                    let bits = s.to_bits(s.size());
                    let v = bits as i128;
                    if crate::trace::trace_enabled("CONST") {
                        eprintln!("[forge] i128const ty={ty} val={v} (0x{bits:032x})");
                    }
                    return Ok(self.builder.const_int(v, ty));
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
                Ok(self.builder.const_int(val, ty))
            }
            Operand::RuntimeChecks(_) => Ok(self.builder.constant(0i32)),
        }
    }
    /// checked 算术（`*WithOverflow`）：返回值与溢出标志。
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
    /// 二元运算前按 **MIR 类型**对齐两侧的 IR 宽度。
    ///
    /// 为什么不能只靠值自身的 IR 类型：常量操作数的 IR 类型来自
    /// `map_type(常量类型)`，可能与另一侧（`load` 出来的窄值）不同，
    /// 于是产出 `icmp eq i8 %x, i32 7` 这种非法 IR（v27 M6 实测：
    /// `let mut b = [0u8; 4]; b[0] = 7; if b[0] == 7 { … }`，`-O` 下 verifier 拒绝）。
    /// 这里按 MIR 类型的字节宽判宽，把窄的一侧 zxt/sext 到宽的那侧；宽度相同
    /// 或任一侧非整数（指针/浮点/聚合）就原样返回。
    fn coerce_binop_operands(
        &mut self,
        lhs: Value,
        rhs: Value,
        lhs_ty: Ty<'tcx>,
        rhs_ty: Ty<'tcx>,
        signed: bool,
    ) -> Result<(Value, Value), ForgeError> {
        use rustc_middle::ty::TyKind;
        let is_int = |t: Ty<'tcx>| matches!(t.kind(), TyKind::Int(_) | TyKind::Uint(_));
        if !is_int(lhs_ty) || !is_int(rhs_ty) {
            return Ok((lhs, rhs));
        }
        let lb = crate::layout::layout_bytes(self.tcx, lhs_ty);
        let rb = crate::layout::layout_bytes(self.tcx, rhs_ty);
        if crate::trace::trace_enabled("CONST") {
            eprintln!(
                "[forge] coerce lhs={lhs_ty} ({lb}B) rhs={rhs_ty} ({rb}B) \
                 ir=({:?},{:?})",
                self.builder.value_type(lhs),
                self.builder.value_type(rhs)
            );
        }
        if lb == rb {
            return Ok((lhs, rhs));
        }
        let (narrow, wide_ty) = if lb < rb {
            (lhs, map_type(rhs_ty, self.tcx)?)
        } else {
            (rhs, map_type(lhs_ty, self.tcx)?)
        };
        let ext = if signed {
            self.builder.sextend(narrow, wide_ty)
        } else {
            self.builder.uextend(narrow, wide_ty)
        };
        Ok(if lb < rb { (ext, rhs) } else { (lhs, ext) })
    }

    /// 把两个整数操作数对齐到**同一个 IR 类型**（比较类指令的前提）。
    ///
    /// 为什么需要：`icmp` 的类型规则是 `binop_same`（两侧同类型、非聚合）。而本后端的
    /// 常量可能以**更宽的规范宽度**出现（`const_int` 走的是类型的 `scalar_bits`，但
    /// rustc 给的 `op1_ty` 有时是提升后的宽类型），于是一条真实比较会变成
    /// `icmp eq i8 %load, i32 7`——非法 IR，`-O` 下 verifier 立刻拒绝
    /// （v27 M6 实测：`let mut b = [0u8; 4]; b[0] = 7; if b[0] == 7 { … }`）。
    ///
    /// 规则：按**值的真实类型**取位宽，把窄的一侧按 `signed` 扩展（zxt/sext）到宽的一侧；
    /// 类型相同或拿不到位宽（动态位宽）时原样返回——不猜。
    fn align_int_operands(&mut self, a: Value, b: Value, signed: bool) -> (Value, Value) {
        let (Some(at), Some(bt)) = (self.builder.value_type(a), self.builder.value_type(b)) else {
            return (a, b);
        };
        if at == bt {
            return (a, b);
        }
        let (Some(ab), Some(bb)) = (at.builtin_scalar_bits(), bt.builtin_scalar_bits()) else {
            return (a, b);
        };
        if ab == bb {
            // 位宽相同、编码不同（如 i8 与 u8）：按目标类型的宽度转一次即可让类型一致。
            let wide = if ab >= bb { at } else { bt };
            let (x, y) = if ab >= bb { (a, b) } else { (b, a) };
            let y2 = if signed {
                self.builder.sextend(y, wide)
            } else {
                self.builder.uextend(y, wide)
            };
            return if ab >= bb { (x, y2) } else { (y2, x) };
        }
        if ab < bb {
            let e = if signed {
                self.builder.sextend(a, bt)
            } else {
                self.builder.uextend(a, bt)
            };
            (e, b)
        } else {
            let e = if signed {
                self.builder.sextend(b, at)
            } else {
                self.builder.uextend(b, at)
            };
            (a, e)
        }
    }

    /// 窄（i8/u8/i16/u16）移位的结果**截回原宽度**。
    ///
    /// 移位的操作数为了符号正确被扩展到 64 位（见 `lower_binary_op`），所以结果必须先
    /// 截回原类型，否则高阶位会漏出去（例如 `i8: -1 << 1` 会留下 64 位的 -2）。
    fn reduce_narrow_shift(&mut self, v: Value, ty: Ty<'tcx>) -> Result<Value, ForgeError> {
        if !matches!(
            ty.kind(),
            rustc_middle::ty::TyKind::Int(_) | rustc_middle::ty::TyKind::Uint(_)
        ) {
            return Ok(v);
        }
        if crate::layout::layout_bytes(self.tcx, ty) < 4 {
            return Ok(self.builder.ireduce(v, map_type(ty, self.tcx)?));
        }
        Ok(v)
    }

    /// 若 MIR 操作数是**整数常量**且其值为 2 的幂，返回该值（宽值除法的改写用）。
    fn mir_const_pow2(&self, op: &mir::Operand<'tcx>) -> Option<u128> {
        let mir::Operand::Constant(c) = op else {
            return None;
        };
        let s = c.const_.try_to_scalar_int().or_else(|| {
            c.const_
                .try_eval_scalar_int(self.tcx, ty::TypingEnv::fully_monomorphized())
        })?;
        let v = s.to_bits(s.size());
        if v != 0 && v.is_power_of_two() {
            Some(v)
        } else {
            None
        }
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
        // WA-36 + v27 M5 缺陷①：窄域（rustc i8/u8/i16/u16）的**比较与移位**都要先扩展——
        // x86 的 cmp/sar 走 32/64 位域，未扩展的窄位模式会算错（i8 -50 被当 +206、
        // `i8: -128 >> 7` 得 1）。下面按 `narrow_w < 4` 统一 sext/zext 到 64，
        // 结果再按需要截回原宽度。
        let narrow_w = match op1_ty.kind() {
            rustc_middle::ty::TyKind::Int(_) | rustc_middle::ty::TyKind::Uint(_) => {
                layout_bytes(self.tcx, op1_ty)
            }
            _ => 4,
        };
        let (lhs, rhs) = if !is_fp && narrow_w < 4 {
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
        // 兜底对齐：**比较两侧必须同类型**（`icmp` 的 `binop_same` 规则）。
        // 上面那条按 `op1_ty` 判窄的路径覆盖不到的情况（实测：`op1_ty` 给了宽类型，
        // 而实际值是窄的 `load i8` + 一个被提升成 i32 的常量）会产出
        // `icmp eq i8 %x, i32 7` 这种非法 IR——`-O` 下 verifier 立刻炸
        //（v27 M6 实测：`let mut b = [0u8; 4]; b[0] = 7; if b[0] == 7 { … }`）。
        // 这里按**值的真实类型**把窄的一侧扩到宽的一侧（语义同 C 的整型提升）。
        let (lhs, rhs) = if is_fp {
            (lhs, rhs)
        } else {
            self.align_int_operands(lhs, rhs, signed)
        };
        match op {
            mir::BinOp::Add | mir::BinOp::AddWithOverflow | mir::BinOp::AddUnchecked => {
                if is_fp {
                    Ok(self.builder.fadd(lhs, rhs))
                } else {
                    let r = self.builder.iadd(lhs, rhs);
                    self.reduce_narrow_shift(r, op1_ty)
                }
            }
            mir::BinOp::Sub | mir::BinOp::SubWithOverflow | mir::BinOp::SubUnchecked => {
                if is_fp {
                    Ok(self.builder.fsub(lhs, rhs))
                } else {
                    let r = self.builder.isub(lhs, rhs);
                    self.reduce_narrow_shift(r, op1_ty)
                }
            }
            mir::BinOp::Mul | mir::BinOp::MulWithOverflow | mir::BinOp::MulUnchecked => {
                if is_fp {
                    Ok(self.builder.fmul(lhs, rhs))
                } else {
                    let r = self.builder.imul(lhs, rhs);
                    self.reduce_narrow_shift(r, op1_ty)
                }
            }
            mir::BinOp::Div => {
                if is_fp {
                    Ok(self.builder.fdiv(lhs, rhs))
                } else {
                    let r = if signed {
                        self.builder.sdiv(lhs, rhs)
                    } else {
                        self.builder.udiv(lhs, rhs)
                    };
                    self.reduce_narrow_shift(r, op1_ty)
                }
            }
            mir::BinOp::Rem => {
                let r = if signed {
                    self.builder.srem(lhs, rhs)
                } else {
                    self.builder.urem(lhs, rhs)
                };
                self.reduce_narrow_shift(r, op1_ty)
            }
            mir::BinOp::BitAnd => Ok(self.builder.band(lhs, rhs)),
            mir::BinOp::BitOr => Ok(self.builder.bor(lhs, rhs)),
            mir::BinOp::BitXor => Ok(self.builder.bxor(lhs, rhs)),
            mir::BinOp::Shl | mir::BinOp::ShlUnchecked => {
                let r = self.builder.ishl(lhs, rhs);
                self.reduce_narrow_shift(r, op1_ty)
            }
            mir::BinOp::Shr | mir::BinOp::ShrUnchecked => {
                // 有符号 >> 是算术右移，无符号 >> 是逻辑右移
                let r = if signed {
                    self.builder.sshr(lhs, rhs)
                } else {
                    self.builder.ushr(lhs, rhs)
                };
                self.reduce_narrow_shift(r, op1_ty)
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
                    let sz = self.builder.const_int(elem_size, TypeId::I64);
                    let scaled = self.builder.imul(rhs, sz);
                    Ok(self.builder.iadd(lhs, scaled))
                }
            }
            mir::BinOp::Eq => {
                // 浮点相等必须走 fcmp（ucomisd），icmp 只支持整数
                if is_fp {
                    Ok(self.builder.fcmp(FloatCC::Oeq, lhs, rhs))
                } else {
                    Ok(self.builder.icmp(IntCC::Eq, lhs, rhs))
                }
            }
            mir::BinOp::Ne => {
                if is_fp {
                    // v27 M5 缺陷（NaN 比较）：Rust 的 `!=` 是**无序不等**（`Une`）——
                    // 任一侧是 NaN 都必须返回 **true**。原来映射成 `One`（有序不等）⇒
                    // `NaN != 1.0` 返回 false。实测（位掩码探针）：其余五种比较都对，
                    // 只有 `!=` 在 NaN 上错，正是这一处。
                    Ok(self.builder.fcmp(FloatCC::Une, lhs, rhs))
                } else {
                    Ok(self.builder.icmp(IntCC::Ne, lhs, rhs))
                }
            }
            mir::BinOp::Lt => {
                if is_fp {
                    Ok(self.builder.fcmp(FloatCC::Olt, lhs, rhs))
                } else if signed {
                    Ok(self.builder.icmp(IntCC::Slt, lhs, rhs))
                } else {
                    Ok(self.builder.icmp(IntCC::Ult, lhs, rhs))
                }
            }
            mir::BinOp::Le => {
                if is_fp {
                    Ok(self.builder.fcmp(FloatCC::Ole, lhs, rhs))
                } else if signed {
                    Ok(self.builder.icmp(IntCC::Sle, lhs, rhs))
                } else {
                    Ok(self.builder.icmp(IntCC::Ule, lhs, rhs))
                }
            }
            mir::BinOp::Gt => {
                if is_fp {
                    Ok(self.builder.fcmp(FloatCC::Ogt, lhs, rhs))
                } else if signed {
                    Ok(self.builder.icmp(IntCC::Sgt, lhs, rhs))
                } else {
                    Ok(self.builder.icmp(IntCC::Ugt, lhs, rhs))
                }
            }
            mir::BinOp::Ge => {
                if is_fp {
                    Ok(self.builder.fcmp(FloatCC::Oge, lhs, rhs))
                } else if signed {
                    Ok(self.builder.icmp(IntCC::Sge, lhs, rhs))
                } else {
                    Ok(self.builder.icmp(IntCC::Uge, lhs, rhs))
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
        let eight = self.builder.const_int(8, TypeId::I64);
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
                    let zero = self.builder.const_int(0, TypeId::BOOL);
                    Ok(self.builder.icmp(IntCC::Eq, val, zero))
                } else {
                    Ok(self.builder.bnot(val))
                }
            }
            mir::UnOp::Neg => {
                let zero = self.builder.constant(0i32);
                Ok(self.builder.isub(zero, val))
            }
            mir::UnOp::PtrMetadata => {
                // 指针元数据：thin 指针的 metadata 为空（值无关紧要，恒 0）
                Ok(self.builder.const_int(0, TypeId::I64))
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
        // **零尺寸类型（ZST）不产生 Store**（v27 M6-116）：`()` 等类型 `map_type` 给
        // VOID，`lower_operand` 会造出 `Iconst ty=VOID`，随后 Store 拿着一个 bits=0
        // 的值找规则 ⇒ `Store no matching rule`。ZST 没有内存表示，写它也没有语义。
        if ty == TypeId::VOID {
            return;
        }
        if ty.is_float() {
            self.builder.fstore(val, addr)
        } else {
            self.builder.store(val, addr)
        }
    }
}
