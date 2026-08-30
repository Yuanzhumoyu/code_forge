//! TargetLowering 生成：Opcode → 指令序列（谓词/占位符/临时寄存器）。
//!
//! 从 `integration.rs` 拆分（原 2060-3524 行）。依赖 integration 的工具
//! 函数（fids/inst_fids/inst_move_role/parse_i64_lit/compile_pred_guard/
//! gen_lowering_attrs/strip_placeholder_decls/lowering_token_kind/
//! parse_mem_template/collect_phys_clobbers/inst_reg_imm_fids）与
//! codegen 的 pascal_ident、model 类型。
//!
//! ## x86 浮点 Call 路径硬编码（保留原因）
//! Return/Call/arg_move_loop 的浮点路径直接引用 `Inst::Movss`/`Inst::Movsd`
//! 变体（f32/f64 移动）与 by-ref 向量 `VMOVUPS*`。这些指令名与字段角色
//! 已硬编码在生成代码中（非配置字符串）——riscv 无浮点 Call（浮点参数/
//! 返回经 GPR 位模式或降级 Unsupported），仅 x86 触发。完全模型化需把
//! "浮点移动指令 + 字段角色"抽象为 ABI 键 + 运行时查表，收益 < 风险，
//! 故保留并在此集中标注（新增浮点 Call 的 ISA 需按此路径扩展）。

use super::super::model::*;
use super::super::pred::{self, CmpOp, Pred};
use super::integration::{
    collect_phys_clobbers, compile_pred_guard, gen_lowering_attrs, inst_exists, inst_fids,
    inst_move_role, lowering_token_kind, parse_i64_lit, parse_mem_template,
    strip_placeholder_decls,
};
use super::{InstInfo, field_ctor_expr};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

// ─────────────────────── TargetLowering ───────────────────────

/// 入口：`pub(crate)` 由 `integration::gen_integration()` 调用。
pub(crate) fn gen_lowering(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    // 助记符 → 候选指令（v12.1：lowering 模板用汇编助记符引用 = asm 首词）。
    // 同助记符多形状（如 add reg,reg 与 add reg,imm）→ 由 gen_lowering_insts
    // 按模板操作数 token 类型消歧（reg/imm/mem/cond）。
    let mut mnemonic_to_infos: std::collections::HashMap<&str, Vec<&InstInfo>> =
        std::collections::HashMap::new();
    for i in infos {
        mnemonic_to_infos
            .entry(i.mnemonic.as_str())
            .or_default()
            .push(i);
    }
    let name_to_vn = mnemonic_to_infos;

    let mut arms: Vec<TokenStream> = Vec::new();
    let lowering_attrs = gen_lowering_attrs();
    // Call/CallIndirect：专用 lowering（参数→ABI 寄存器、函数符号 reloc、
    // 返回值移动）——动态参数数/类型分派无法用静态模板表达；无 TOML 规则。
    arms.push(gen_call_lowering("Call", infos, model)?);
    arms.push(gen_call_lowering("CallIndirect", infos, model)?);
    // 按 op 分组（保持声明序）——同 op 多规则用 when 谓词 if-else-if 链分派。
    let mut by_op: Vec<(&str, Vec<&Lowering>)> = Vec::new();
    for rule in &model.lowering {
        match by_op.iter_mut().find(|(op, _)| *op == rule.op.as_str()) {
            Some((_, rules)) => rules.push(rule),
            None => by_op.push((rule.op.as_str(), vec![rule])),
        }
    }
    for (op_name, rules) in by_op {
        let op_ident = format_ident!("{op_name}");
        // 从最后一条规则向前构建 if-else 链（无 when 的规则 = 无条件兜底）
        let mut chain: TokenStream = quote! {
            Err(crate::prelude::IrError::Unsupported(concat!("v12 lowering: ", #op_name, " no matching rule").into()))
        };
        for rule in rules.iter().rev() {
            // 展开 insts 模板 → 指令构造序列（符号化操作数 {out}/{0}/{1}）
            let inst_toks =
                gen_lowering_insts(&rule.insts, infos, &name_to_vn, lowering_width_hint(rule))?;
            // 模板临时寄存器预声明：从占位符注册表收集（{g}/{gN} → GPR、
            // {f}/{fN} → FPR）。新增临时占位符只改 placeholder.rs。
            let mut t_binds: Vec<TokenStream> = Vec::new();
            let mut tf_binds: Vec<TokenStream> = Vec::new();
            for (var, cls) in super::placeholder::collect_temps(&rule.insts) {
                let vid = format_ident!("{var}");
                match cls {
                    super::placeholder::PhTemp::Gpr => {
                        t_binds.push(quote! { let #vid = ctx.alloc_xreg(__DEFAULT_GPR_CLASS); });
                    }
                    super::placeholder::PhTemp::Fpr => {
                        tf_binds.push(quote! { let #vid = ctx.alloc_xreg(__DEFAULT_FPR_CLASS); });
                    }
                }
            }
            let t_bind: TokenStream = if t_binds.is_empty() {
                quote! {}
            } else {
                quote! { #(#t_binds)* }
            };
            let tf_bind: TokenStream = if tf_binds.is_empty() {
                quote! {}
            } else {
                quote! { #(#tf_binds)* }
            };
            // clobber：模板中写死的物理寄存器（RAX/RDX 等）——regalloc 本点避开
            let clobbers = collect_phys_clobbers(&rule.insts, infos, model)?;
            let clobber_set: TokenStream = if clobbers.is_empty() {
                quote! {}
            } else {
                quote! { ctx.current_clobbers = vec![#(#clobbers),*]; }
            };
            // Icmp 特殊：__cc = 条件码（setcc 用；其他 op 恒 0）
            let cc_bind: TokenStream = if rule.op == "Icmp" {
                quote! {
                    let __cc: u8 = match op {
                        crate::prelude::Opcode::Icmp { cond } => {
                            use crate::prelude::IntCC::*;
                            match cond {
                                Equal => 4, NotEqual => 5,
                                SignedLessThan => 12, SignedLessThanOrEqual => 14,
                                SignedGreaterThan => 15, SignedGreaterThanOrEqual => 13,
                                UnsignedLessThan => 2, UnsignedLessThanOrEqual => 6,
                                UnsignedGreaterThan => 7, UnsignedGreaterThanOrEqual => 3,
                                _ => 4,
                            }
                        }
                        _ => 0,
                    };
                }
            } else {
                quote! { let __cc: u8 = 0; }
            };
            let body = quote! {
                #cc_bind
                #t_bind
                #tf_bind
                #clobber_set
                #(#inst_toks)*
                Ok(__pack)
            };
            match &rule.when {
                Some(v) => {
                    let pred = pred::parse(v)
                        .map_err(|e| format!("[[lowering.{}]] when: {e}", rule.op))?;
                    let guard = compile_pred_guard(&pred, &format_ident!("__attr"));
                    chain = quote! { if #guard { #body } else { #chain } };
                }
                None => {
                    chain = quote! { #body };
                }
            }
        }
        arms.push(quote! {
            crate::prelude::Opcode::#op_ident { .. } => {
                #lowering_attrs
                let mut __pack = crate::prelude::InstPacket::new();
                #chain
            }
        });
    }

    // terminator 指令引用：`[abi]` 键驱动（ret_inst/jump_inst/branch_inst/
    // test_inst）。**缺省值是固定默认名**（按 ISA 形态：变长 x86 → 相对
    // 跳转、定宽 riscv → JAL/BEQ），**不做"按名字存在性"猜测**——指令
    // 不存在时由 inst_fids 查找报错（用户用其他名字必须显式声明键）。
    let abi_cfg = model.abi.as_ref();
    let ret_inst = abi_cfg
        .and_then(|a| a.ret_inst.clone())
        .unwrap_or_else(|| "RET".to_string());
    let jump_inst = abi_cfg
        .and_then(|a| a.jump_inst.clone())
        .unwrap_or_else(|| {
            if model.meta.variable_length {
                "JMP_REL32".to_string()
            } else {
                "JAL".to_string()
            }
        });
    let branch_inst = abi_cfg
        .and_then(|a| a.branch_inst.clone())
        .unwrap_or_else(|| {
            if model.meta.variable_length {
                "JCC_REL32".to_string()
            } else {
                "BEQ".to_string()
            }
        });
    let test_inst = abi_cfg
        .and_then(|a| a.test_inst.clone())
        .unwrap_or_else(|| "TEST_RM_R".to_string());
    // 指令存在性：终结符指令必须声明（缺失 → 该终结符 Unsupported，
    // 生成代码引用不存在的变体是编译错误；用查找失败兜底报错信息）。
    // 存在性 = `inst_exists`（与操作数无关——RET/NOP 无操作数，不能
    // 用 `inst_fids` 空 vec 判断）。
    let has_ret = inst_exists(infos, &ret_inst);
    let has_jmp = inst_exists(infos, &jump_inst);
    let has_jcc = inst_exists(infos, &branch_inst);
    let has_test = inst_exists(infos, &test_inst);
    // 定宽跳转（JAL 语义）：jump_inst 指令存在即启用（非按 ISA 形态猜——
    // demo 等无跳转指令的定宽 ISA 自然降级，不生成不存在的变体引用）。
    let has_jal = has_jmp && !model.meta.variable_length;
    // 定宽（riscv）跳转：JAL x0（rd=index0、label 槽 = 块号 → encoder 定宽
    // fixup Relative(4,0)）。JAL 字段 = [dest(gpr out), target(label)]。
    let jal_f = inst_fids(infos, &jump_inst);
    let (jal_dest, jal_target) = if jal_f.len() >= 2 {
        (jal_f[0].clone(), jal_f[1].clone())
    } else {
        (format_ident!("dest"), format_ident!("target"))
    };
    // 定宽条件分支（riscv）：BEQ 字段 = [src, src2, target(label)]。
    let beq_f = inst_fids(infos, &branch_inst);
    let (b_src, b_src2, b_target) = if beq_f.len() >= 3 {
        (beq_f[0].clone(), beq_f[1].clone(), beq_f[2].clone())
    } else {
        (
            format_ident!("src"),
            format_ident!("src2"),
            format_ident!("target"),
        )
    };
    // 返回移动指令名可配置（[abi].ret_mov_inst，缺省 "MOV_RM8_R64"）。
    let ret_mov_inst = abi_cfg
        .and_then(|a| a.ret_mov_inst.clone())
        .unwrap_or_else(|| "MOV_RM8_R64".to_string());
    let has_mov_rax = inst_exists(infos, &ret_mov_inst);
    let ret_vn = crate::v12::codegen::pascal_ident(&ret_mov_inst);
    let ret_move = inst_move_role(infos, &ret_mov_inst);
    // 类型化字段名（按指令名查，缺省 op0/op1/op2 兜底——集成层构造用）
    let fids =
        |name: &str| -> Vec<syn::Ident> { inst_fids(infos, name).into_iter().cloned().collect() };
    let (mov_src, mov_src_idx, mov_dest, _mov_dest_idx) = match &ret_move {
        Some((s, si, d, _di)) => (s.clone(), *si, d.clone(), 0u8),
        None => (format_ident!("src"), 0u8, format_ident!("dest"), 0u8),
    };
    let jmp_f = fids(&jump_inst);
    let jmp_rel = jmp_f
        .first()
        .cloned()
        .unwrap_or_else(|| format_ident!("target"));
    let test_f = fids(&test_inst);
    let (t_a, t_b) = if test_f.len() == 2 {
        (test_f[0].clone(), test_f[1].clone())
    } else {
        (format_ident!("src"), format_ident!("src2"))
    };
    let jcc_f = fids(&branch_inst);
    let (jcc_cond, jcc_rel) = if jcc_f.len() == 2 {
        (jcc_f[0].clone(), jcc_f[1].clone())
    } else {
        (format_ident!("cond"), format_ident!("target"))
    };

    // Return：整数值 → RAX（MOV_RM8_R64）、浮点值 → XMM0（MOVSD/MOVSS）。
    // 与 v11 一致：**不生成 RET**——return block 经 emit_epilogue_jump 跳到
    // epilogue 统一恢复 callee-saved 后 ret（否则栈不平衡崩溃）。
    // 浮点移动指令由 `[abi].fpr_mov_inst`/`fpr_mov_inst32` 键指定
    //（缺省 "MOVSD"/"MOVSS"），变体名与字段均按键派生——不做名判断。
    let fpr_mov64 = abi_cfg
        .and_then(|a| a.fpr_mov_inst.clone())
        .unwrap_or_else(|| "MOVSD".to_string());
    let fpr_mov32 = abi_cfg
        .and_then(|a| a.fpr_mov_inst32.clone())
        .unwrap_or_else(|| "MOVSS".to_string());
    let fpr_mov64_vn = crate::v12::codegen::pascal_ident(&fpr_mov64);
    let fpr_mov32_vn = crate::v12::codegen::pascal_ident(&fpr_mov32);
    let fpr_mov_fids = inst_fids(infos, &fpr_mov64);
    let has_fpr_mov = fpr_mov_fids.len() >= 2 && inst_fids(infos, &fpr_mov32).len() >= 2;
    let fpr_mov_src: syn::Ident = match fpr_mov_fids.get(1) {
        Some(f) => (*f).clone(),
        None => format_ident!("src"),
    };
    let fpr_mov_dest: syn::Ident = match fpr_mov_fids.first() {
        Some(f) => (*f).clone(),
        None => format_ident!("dest"),
    };
    let fpr_return_body: TokenStream = if has_fpr_mov {
        quote! {
            // 浮点返回值 → XMM0（f32 → fpr_mov32 / f64 → fpr_mov64）
            let __fidx = __pack.push_inst(if ctx.xreg_types.get(&val).map(|t| t.bits()).unwrap_or(64) == 32 {
                Inst::#fpr_mov32_vn {
                    #fpr_mov_dest: Reg::from_index(0, __DEFAULT_FPR_CLASS),
                    #fpr_mov_src: Reg::from_index(0, __DEFAULT_FPR_CLASS),
                }
            } else {
                Inst::#fpr_mov64_vn {
                    #fpr_mov_dest: Reg::from_index(0, __DEFAULT_FPR_CLASS),
                    #fpr_mov_src: Reg::from_index(0, __DEFAULT_FPR_CLASS),
                }
            });
            __pack.map_reg_field(val, __fidx, 1u8, false);
        }
    } else {
        quote! {
            let _ = __pack;
            return Err(crate::prelude::IrError::Unsupported("v12 float return (fpr_mov_inst missing)".into()));
        }
    };
    let return_body: TokenStream = if has_mov_rax {
        let mov_src_idx_lit = mov_src_idx as usize;
        // 返回寄存器：ret_regs 首项（riscv X10=a0）或 index 0（x86 RAX）。
        let ret_dest: TokenStream = model
            .abi
            .as_ref()
            .and_then(|a| a.ret_regs.first())
            .map(|r| {
                let ident = format_ident!("{r}");
                quote! { Reg::#ident }
            })
            .unwrap_or_else(|| quote! { Reg::from_index(0, forge_ir::RegClass::GPR64) });
        quote! {
            let val = values.first().copied()
                .and_then(|x| value_to_xreg.get(&x)).copied()
                .unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
            if ctx.xreg_types.get(&val).is_some_and(|t| t.is_float()) {
                #fpr_return_body
            } else {
                let __idx = __pack.push_inst(Inst::#ret_vn {
                    #mov_src: Reg::from_index(0, forge_ir::RegClass::GPR64),
                    #mov_dest: #ret_dest,

                });
                __pack.map_reg_field(val, __idx, #mov_src_idx_lit as u8, false);
            }
            Ok(__pack)
        }
    } else {
        quote! {
            let _ = __pack;
            Err(crate::prelude::IrError::Unsupported("v12 return lowering (ret mov inst missing)".into()))
        }
    };
    // Jump：跳转指令（[abi].jump_inst 键；缺省 JMP_REL32/JAL 存在性回退）
    // 变体名按键派生，避免硬编码 Inst::JmpRel32/Inst::Jal。
    let jump_vn = crate::v12::codegen::pascal_ident(&jump_inst);
    let jump_body: TokenStream = if has_jal {
        quote! {
            __pack.push_inst(Inst::#jump_vn {
                #jal_dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
                #jal_target: target.0 as i64,
            });
            Ok(__pack)
        }
    } else if has_jmp {
        quote! {
            __pack.push_inst(Inst::#jump_vn { #jmp_rel: target.0 as i64 });
            Ok(__pack)
        }
    } else {
        quote! {
            let _ = __pack;
            Err(crate::prelude::IrError::Unsupported("v12 jump lowering (JMP_REL32/JAL missing)".into()))
        }
    };
    // Branch：x86 = test cond,cond → je false → jmp true（v11 语义）；
    // 定宽（riscv）= beq cond, X0, false → jal x0, true（X0 恒零 → cond==0
    // 走 false；label 槽块号 → 定宽 fixup）。变体名按键派生。
    let test_vn = crate::v12::codegen::pascal_ident(&test_inst);
    let branch_vn = crate::v12::codegen::pascal_ident(&branch_inst);
    let beq_vn = crate::v12::codegen::pascal_ident(&branch_inst);
    let branch_body: TokenStream = if has_test && has_jcc && has_jmp {
        quote! {
            let cond = value_to_xreg.get(cond_val).copied()
                .unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
            let true_block = then_block.0 as i64;
            let false_block = else_block.0 as i64;
            // TEST cond, cond（85 /r：reg=op0、rm=op1）
            let __idx = __pack.push_inst(Inst::#test_vn {
                #t_a: Reg::from_index(0, forge_ir::RegClass::GPR64),
                #t_b: Reg::from_index(0, forge_ir::RegClass::GPR64),

            });
            __pack.map_reg_field(cond, __idx, 0u8, false);
            __pack.map_reg_field(cond, __idx, 1u8, false);
            // je false_block（cond=4=e）
            __pack.push_inst(Inst::#branch_vn { #jcc_cond: 4u8, #jcc_rel: false_block });
            // jmp true_block
            __pack.push_inst(Inst::#jump_vn { #jmp_rel: true_block });
            Ok(__pack)
        }
    } else if has_jal && !beq_f.is_empty() {
        quote! {
            let cond = value_to_xreg.get(cond_val).copied()
                .unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
            let true_block = then_block.0 as i64;
            let false_block = else_block.0 as i64;
            // beq cond, X0, false_block（cond==0 → false）
            let __idx = __pack.push_inst(Inst::#beq_vn {
                #b_src: Reg::from_index(0, forge_ir::RegClass::GPR64),
                #b_src2: Reg::from_index(0, forge_ir::RegClass::GPR64),
                #b_target: false_block,
            });
            __pack.map_reg_field(cond, __idx, 0u8, false);
            // jal x0, true_block
            __pack.push_inst(Inst::#jump_vn {
                #jal_dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
                #jal_target: true_block,
            });
            Ok(__pack)
        }
    } else {
        quote! {
            let _ = __pack;
            Err(crate::prelude::IrError::Unsupported("v12 branch lowering (TEST/JCC/JMP or BEQ/JAL missing)".into()))
        }
    };

    // 无任何 terminator 指令（riscv 等）→ 直接 Err 版本（避免 __pack 未使用/不可达）
    let term_impl: TokenStream = if has_ret || has_jmp || has_jcc {
        quote! {
            #[allow(unreachable_code)]
            fn lower_terminator(
                &self,
                term: &crate::prelude::Terminator,
                value_to_xreg: &std::collections::HashMap<crate::prelude::Value, crate::prelude::XReg>,
                _block_to_vblock: &std::collections::HashMap<crate::prelude::Block, crate::prelude::VBlockId>,
                ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::IrError> {
                let mut __pack: crate::prelude::InstPacket<Self::Inst> =
                    crate::prelude::InstPacket::new();
                match term {
                    crate::prelude::Terminator::Return { values, .. } => {
                        #return_body
                    }
                    crate::prelude::Terminator::Jump { target, .. } => {
                        #jump_body
                    }
                    crate::prelude::Terminator::Branch {
                        cond: cond_val,
                        then_block,
                        else_block,
                        ..
                    } => {
                        #branch_body
                    }
                    _ => Err(crate::prelude::IrError::Unsupported("v12 terminator lowering".into())),
                }
            }
        }
    } else {
        quote! {
            fn lower_terminator(
                &self,
                _term: &crate::prelude::Terminator,
                _value_to_xreg: &std::collections::HashMap<crate::prelude::Value, crate::prelude::XReg>,
                _block_to_vblock: &std::collections::HashMap<crate::prelude::Block, crate::prelude::VBlockId>,
                _ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::IrError> {
                Err(crate::prelude::IrError::Unsupported("v12 terminator lowering".into()))
            }
        }
    };

    // 操作数符号预绑定：收集所有模板用到的 `{N}` 最大编号，生成
    // `let rs1..=rsN = args.get(..)…`（任意多操作数指令；不再硬编码 3 个）。
    let max_op_idx: usize = model
        .lowering
        .iter()
        .flat_map(|r| &r.insts)
        .flat_map(|t| {
            t.split(['{', '}'])
                .skip(1)
                .step_by(2)
                .filter_map(|tok| tok.parse::<usize>().ok())
        })
        .max()
        .unwrap_or(0);
    let op_binds: TokenStream = (0..=max_op_idx)
        .map(|i| {
            let rsn = format_ident!("rs{}", i + 1);
            quote! {
                let #rsn = args.get(#i).copied().unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
            }
        })
        .collect();

    Ok(quote! {
        /// 谓词属性 `elem` 的数值映射（标量类型；向量元素在 Phase 6 扩展）。
        fn elem_id_of(t: crate::prelude::TypeId) -> i64 {
            match t {
                crate::prelude::TypeId::F32 => 1,
                crate::prelude::TypeId::F64 => 2,
                crate::prelude::TypeId::I32 => 3,
                crate::prelude::TypeId::I64 => 4,
                crate::prelude::TypeId::I8 => 5,
                crate::prelude::TypeId::I16 => 6,
                _ => 0,
            }
        }

        /// 谓词属性 `cond` 的数值映射（Fcmp 16 条件；与 TOML when 谓词对齐）。
        fn fcmp_id(c: &crate::prelude::FloatCC) -> i64 {
            use crate::prelude::FloatCC::*;
            match c {
                Ordered => 1, Unordered => 2, Equal => 3, NotEqual => 4,
                LessThan => 5, LessThanOrEqual => 6, GreaterThan => 7, GreaterThanOrEqual => 8,
                False => 9, True => 10, Ueq => 11, Ugt => 12, Uge => 13, Ult => 14, Ule => 15, Une => 16,
            }
        }

        /// 谓词属性 `cond` 的数值映射（Icmp 10 条件）。
        fn icmp_id(c: &crate::prelude::IntCC) -> i64 {
            use crate::prelude::IntCC::*;
            match c {
                Equal => 1, NotEqual => 2,
                SignedLessThan => 3, SignedLessThanOrEqual => 4, SignedGreaterThan => 5, SignedGreaterThanOrEqual => 6,
                UnsignedLessThan => 7, UnsignedLessThanOrEqual => 8, UnsignedGreaterThan => 9, UnsignedGreaterThanOrEqual => 10,
            }
        }

        /// ShuffleVector 掩码 → SHUFPS imm8（每 2 位一个 lane 索引 0-3；
        /// mask 元素 0-3=a、4-7=b、8-11=a.hi、12-15=b.hi → `m & 3` 通用）。
        fn __shufps_imm8(mask: &[u64], base: usize) -> i64 {
            let mut imm: u64 = 0;
            for i in 0..4 {
                imm |= (mask.get(base + i).copied().unwrap_or(0) & 3) << (i * 2);
            }
            imm as i64
        }

        /// 向量结果的元素位宽（Vconst 的 lane 恢复分派用）。
        fn __vconst_elem_bits(
            ctx: &crate::prelude::LowerCtx,
            results: &[crate::prelude::XReg],
        ) -> u64 {
            let t = results.first().and_then(|x| ctx.xreg_types.get(x));
            t.and_then(|&t| {
                ctx.type_ctx
                    .as_ref()
                    .and_then(|tc| tc.element_type(t))
            })
            .map(|e| e.bits() as u64)
            .unwrap_or(32)
        }

        /// 向量常量字节 → 64 位半（half=0/1 低/高 128 位；hi=低/高 64 位）。
        /// elem64=直取 64 位 lane；否则 32 位元素交错（lane0|lane2<<32 等）。
        fn __vconst_half(
            pool: Option<&forge_ir::ConstantPool>,
            cid: u32,
            half: usize,
            hi: bool,
            elem64: bool,
        ) -> i64 {
            let Some(p) = pool else { return 0 };
            let Some(bytes) = p.get_vector(crate::prelude::ConstId(cid)) else { return 0 };
            let base = half * 16;
            let le = matches!(
                p.get_vector_endian(crate::prelude::ConstId(cid)),
                None | Some(crate::prelude::Endianness::Little)
            );
            if elem64 {
                let o = base + if hi { 8 } else { 0 };
                if bytes.len() < o + 8 { return 0; }
                let mut a = [0u8; 8];
                a.copy_from_slice(&bytes[o..o + 8]);
                (if le { u64::from_le_bytes(a) } else { u64::from_be_bytes(a) }) as i64
            } else {
                let o0 = base + if hi { 4 } else { 0 };
                let o1 = base + if hi { 12 } else { 8 };
                if bytes.len() < o1 + 4 { return 0; }
                let mut l = [0u8; 4];
                l.copy_from_slice(&bytes[o0..o0 + 4]);
                let mut h = [0u8; 4];
                h.copy_from_slice(&bytes[o1..o1 + 4]);
                let lv = if le { u32::from_le_bytes(l) } else { u32::from_be_bytes(l) } as u64;
                let hv = if le { u32::from_le_bytes(h) } else { u32::from_be_bytes(h) } as u64;
                (lv | (hv << 32)) as i64
            }
        }

        /// 向量常量低 8 字节直取（V64 的 2×32 位 lane）。
        fn __vconst_raw64(
            pool: Option<&forge_ir::ConstantPool>,
            cid: u32,
        ) -> i64 {
            let Some(p) = pool else { return 0 };
            let Some(bytes) = p.get_vector(crate::prelude::ConstId(cid)) else { return 0 };
            if bytes.len() < 8 { return 0; }
            let mut a = [0u8; 8];
            a.copy_from_slice(&bytes[0..8]);
            u64::from_le_bytes(a) as i64
        }

        pub struct Lowering;

        impl crate::machine::lowering::TargetLowering for Lowering {
            type Inst = Inst;

            fn lower_inst(
                &self,
                op: &crate::prelude::Opcode,
                args: &[crate::prelude::XReg],
                results: &[crate::prelude::XReg],
                ctx: &mut crate::prelude::LowerCtx,
            ) -> Result<crate::prelude::InstPacket<Self::Inst>, crate::prelude::IrError> {
                let rd = results.first().copied().unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
                let rd2 = results.get(1).copied().unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
                #op_binds
                match op { #(#arms,)* _ => Err(crate::prelude::IrError::Unsupported("v12 lowering".into())) }
            }

            #term_impl
        }
    })
}

/// Call/CallIndirect 专用 lowering（Phase 5）：
/// 参数 → ABI 寄存器（整数 RCX/RDX/R8/R9、浮点 XMM0-3）→ CALL → 返回值
/// RAX/XMM0 → 结果 XReg。Call 的 FuncRef 编码为 `-(f+1)`（负 rel 触发
/// gen_encoder 的函数符号 reloc "@N"）。clobbers 声明 ABI 易失寄存器。
fn gen_call_lowering(
    op_name: &str,
    infos: &[InstInfo],
    model: &V12Model,
) -> Result<TokenStream, String> {
    let vn = |n: &str| crate::v12::codegen::pascal_ident(n);
    let fids = |n: &str| inst_fids(infos, n);
    // ABI 参数/返回寄存器类（缺失 → Call 降级 Unsupported，如定宽试点 ISA）
    let abi = match model.abi.as_ref() {
        Some(a) => a,
        None => {
            let op_ident = format_ident!("{op_name}");
            return Ok(quote! {
                crate::prelude::Opcode::#op_ident { .. } => {
                    Err(crate::prelude::IrError::Unsupported("v12 call lowering ([abi] missing)".into()))
                }
            });
        }
    };
    // 整数移动指令：按 [abi].ret_mov_inst（缺省 "MOV_RM8_R64"），字段按角色
    // 解析（x86 op0=In=src/op1=InOut=dest；demo op0=Out=dest/op1=In=src）。
    let mov_inst = abi
        .ret_mov_inst
        .clone()
        .unwrap_or_else(|| "MOV_RM8_R64".to_string());
    let mov_vn = vn(&mov_inst);
    let (m_src, m_src_idx, m_dest, m_dest_idx) =
        inst_move_role(infos, &mov_inst).ok_or_else(|| {
            format!("Call lowering: [{mov_inst}] must have In(src)/Out|InOut(dest) reg operands")
        })?;
    // 浮点移动指令：`[abi].fpr_mov_inst`/`fpr_mov_inst32` 键（缺省
    // "MOVSD"/"MOVSS"）——缺失 → 浮点路径 Unsupported（防生成代码引用
    // 不存在的 Inst 变体；demo 等无浮点 ISA 的 Call 整体降级）。
    let fpr_mov64 = model
        .abi
        .as_ref()
        .and_then(|a| a.fpr_mov_inst.clone())
        .unwrap_or_else(|| "MOVSD".to_string());
    let fpr_mov32 = model
        .abi
        .as_ref()
        .and_then(|a| a.fpr_mov_inst32.clone())
        .unwrap_or_else(|| "MOVSS".to_string());
    let fpr_mov64_vn = crate::v12::codegen::pascal_ident(&fpr_mov64);
    let fpr_mov32_vn = crate::v12::codegen::pascal_ident(&fpr_mov32);
    let sd_f = fids(&fpr_mov64);
    let (f_dest, f_src) = if sd_f.len() >= 2 {
        (sd_f[0].clone(), sd_f[1].clone())
    } else {
        (format_ident!("dest"), format_ident!("src"))
    };
    let has_ss = fids(&fpr_mov32).len() >= 2;
    let has_fpr_mov = sd_f.len() >= 2 && has_ss;
    // 返回寄存器：ret_regs 首项（riscv X10=a0）或 index 0（x86 RAX）。
    let ret_src_expr: TokenStream = abi
        .ret_regs
        .first()
        .map(|r| {
            let ident = format_ident!("{r}");
            quote! { Reg::#ident }
        })
        .unwrap_or_else(|| {
            quote! { <Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR64) }
        });
    let int_regs: Vec<syn::Ident> = abi
        .arg_class
        .iter()
        .find(|ac| ac.class == "int")
        .map(|ac| ac.regs.iter().map(|r| format_ident!("{r}")).collect())
        .unwrap_or_default();
    let float_regs: Vec<syn::Ident> = abi
        .arg_class
        .iter()
        .find(|ac| ac.class == "float")
        .map(|ac| ac.regs.iter().map(|r| format_ident!("{r}")).collect())
        .unwrap_or_default();
    let n = int_regs.len();
    let fn_ = float_regs.len();
    // clobbers：缺省 = 整数参数寄存器 + 返回寄存器（ret_regs 首项或
    // index 0）。[abi].call_clobbers 可显式覆盖（riscv：无 callee-saved
    // 保存序列 → 全部 caller-saved 临时寄存器都列上，防跨调用存活值留在
    // 寄存器被 callee 破坏——实测递归 fib 死循环）。
    let clobber_gprs: Vec<syn::Ident> = match &abi.call_clobbers {
        Some(names) => names.iter().map(|r| format_ident!("{r}")).collect(),
        None => int_regs.clone(),
    };
    let ret_gpr_clobber: TokenStream = abi
        .ret_regs
        .first()
        .map(|r| {
            let ident = format_ident!("{r}");
            quote! { (Reg::#ident.to_index(), forge_ir::RegClass::GPR64) }
        })
        .unwrap_or_else(|| {
            quote! {
                (<Reg as forge_ir::PhysReg>::from_index(0, forge_ir::RegClass::GPR64).to_index(),
                 forge_ir::RegClass::GPR64)
            }
        });
    let gpr_clobbers: Vec<TokenStream> = clobber_gprs
        .iter()
        .map(|r| quote! { (Reg::#r.to_index(), forge_ir::RegClass::GPR64) })
        .chain(std::iter::once(ret_gpr_clobber))
        .collect();
    let fpr_clobbers: Vec<TokenStream> = float_regs
        .iter()
        .map(|r| quote! { (Reg::#r.to_index(), __DEFAULT_FPR_CLASS) })
        .chain(std::iter::once(quote! {
            (<Reg as forge_ir::PhysReg>::from_index(0, __DEFAULT_FPR_CLASS).to_index(),
             __DEFAULT_FPR_CLASS)
        }))
        .collect();
    let fpr_ret_stmt: TokenStream = if has_fpr_mov {
        quote! {
            let __idx = __pack.push_inst(if ctx.xreg_types.get(&__r).map(|t| t.bits()).unwrap_or(64) == 32 && #has_ss {
                Inst::#fpr_mov32_vn { #f_dest: Reg::from_index(0, __DEFAULT_FPR_CLASS), #f_src: <Reg as forge_ir::PhysReg>::from_index(0, __DEFAULT_FPR_CLASS) }
            } else {
                Inst::#fpr_mov64_vn { #f_dest: Reg::from_index(0, __DEFAULT_FPR_CLASS), #f_src: <Reg as forge_ir::PhysReg>::from_index(0, __DEFAULT_FPR_CLASS) }
            });
            __pack.map_reg_field(__r, __idx, 0u8, true);
        }
    } else {
        quote! {
            return Err(crate::prelude::IrError::Unsupported(
                "v12 call: float return move (fpr_mov_inst missing)".into(),
            ));
        }
    };
    let ret_move: TokenStream = quote! {
        if let Some(&__r) = results.first() {
            if ctx.xreg_types.get(&__r).is_some_and(|t| t.is_float()) {
                #fpr_ret_stmt
            } else {
                let __idx = __pack.push_inst(Inst::#mov_vn {
                    #m_src: #ret_src_expr,
                    #m_dest: Reg::from_index(0, forge_ir::RegClass::GPR64),

                });
                // 结果 vreg 绑 **dest 序号**（x86 MOV_RM8_R64 dest=op1=1；
                // riscv mv dest=op0=0）——按角色泛化，防定宽方向反。
                __pack.map_reg_field(__r, __idx, #m_dest_idx, true);
            }
        }
    };
    // Call：CALL_RIP_REL target = -(FuncRef+1)；指令缺失 → Unsupported。
    // [abi].call_inst 可覆盖（riscv "JAL"：jal ra, @N——label 槽负值 →
    // encoder 转 Relative(4,0) "@N" 符号 reloc，RiscvRelocPatcher 编码 UJ 位段）。
    let call_inst = abi
        .call_inst
        .clone()
        .unwrap_or_else(|| "CALL_RIP_REL".to_string());
    let call_f = fids(&call_inst);
    let mut call_is_err = false;
    // Call 字段构造（结构迭代，不按指令名判断形态）：
    // - Label 槽（x86 rel32 / riscv off_j）= -(FuncRef+1)（encoder 转 "@N"
    //   符号 reloc——变长 CALL 或定宽 JAL 统一）；
    // - Out/InOut Reg 槽 = [abi].call_ret_reg（返回地址寄存器；x86 无此槽）；
    // - 其余槽（in Reg / imm）置 0。
    let call_body: TokenStream = if op_name == "Call" {
        match call_f.first() {
            Some(_) => {
                let ret_reg = abi.call_ret_reg.clone().unwrap_or_else(|| "X1".to_string());
                let ret_ident = format_ident!("{ret_reg}");
                let info = infos
                    .iter()
                    .find(|i| i.inst.name == call_inst)
                    .ok_or_else(|| format!("call_inst '{call_inst}' 不存在"))?;
                let fields: Vec<TokenStream> = info
                    .operands
                    .iter()
                    .map(|(_, fid, slot, role)| {
                        let fid_ident = format_ident!("{fid}");
                        if slot.kind == OperandKind::Label {
                            quote! { #fid_ident: -(__f.0 as i64 + 1) }
                        } else if slot.kind == OperandKind::Reg
                            && matches!(role, OperandRole::Out | OperandRole::InOut)
                        {
                            quote! { #fid_ident: Reg::#ret_ident }
                        } else {
                            quote! { #fid_ident: 0u32 }
                        }
                    })
                    .collect();
                let vn = crate::v12::codegen::pascal_ident(&call_inst);
                quote! {
                    let __f = ctx.current_func_ref
                        .ok_or_else(|| crate::prelude::IrError::Unsupported("v12 call: missing func ref".into()))?;
                    __pack.push_inst(Inst::#vn { #(#fields),* });
                }
            }
            None => {
                call_is_err = true;
                quote! {
                    return Err(crate::prelude::IrError::Unsupported(
                        "v12 call lowering (call inst missing)".into(),
                    ));
                }
            }
        }
    } else {
        // CallIndirect：[abi].call_indirect_inst 键（缺省 "CALL_RM"=x86
        // FF /2；定宽可声明 "JALR"）。结构迭代：In Reg 槽 = 目标地址
        //（args[0]，map_reg_field 绑 vreg）；Out/InOut Reg 槽 =
        // call_ret_reg；imm/label 槽置 0。
        let ci_inst = abi
            .call_indirect_inst
            .clone()
            .unwrap_or_else(|| "CALL_RM".to_string());
        let ci_f = fids(&ci_inst);
        match ci_f.first() {
            Some(_) => {
                let ret_reg = abi.call_ret_reg.clone().unwrap_or_else(|| "X1".to_string());
                let ret_ident = format_ident!("{ret_reg}");
                let info = infos
                    .iter()
                    .find(|i| i.inst.name == ci_inst)
                    .ok_or_else(|| format!("call_indirect_inst '{ci_inst}' 不存在"))?;
                let mut ctor: Vec<TokenStream> = Vec::new();
                let mut binds: Vec<TokenStream> = Vec::new();
                for (idx, (_, fid, slot, role)) in info.operands.iter().enumerate() {
                    let fid_ident = format_ident!("{fid}");
                    match (slot.kind, role) {
                        (OperandKind::Reg, OperandRole::In) => {
                            ctor.push(quote! { #fid_ident: Reg::from_index(0, forge_ir::RegClass::GPR64) });
                            binds.push(quote! {
                                __pack.map_reg_field(__callee, __idx, #idx as u8, false);
                            });
                        }
                        (OperandKind::Reg, OperandRole::Out | OperandRole::InOut) => {
                            ctor.push(quote! { #fid_ident: Reg::#ret_ident });
                        }
                        _ => {
                            ctor.push(quote! { #fid_ident: 0u32 });
                        }
                    }
                }
                let vn = crate::v12::codegen::pascal_ident(&ci_inst);
                quote! {
                    let __callee = args.first().copied()
                        .unwrap_or_else(|| ctx.alloc_xreg(__DEFAULT_GPR_CLASS));
                    let __idx = __pack.push_inst(Inst::#vn { #(#ctor),* });
                    #(#binds)*
                }
            }
            None => {
                call_is_err = true;
                quote! {
                    return Err(crate::prelude::IrError::Unsupported(
                        "v12 call_indirect lowering (call_indirect_inst missing)".into(),
                    ));
                }
            }
        }
    };
    let arg_loop = arg_move_loop(
        op_name,
        &n,
        &fn_,
        &int_regs,
        &float_regs,
        &m_src,
        m_src_idx,
        &m_dest,
        &f_dest,
        &f_src,
        has_ss,
        has_fpr_mov,
        &mov_vn,
        &fpr_mov32_vn,
        &fpr_mov64_vn,
    );
    let op_ident = format_ident!("{op_name}");
    // call_body 为 Unsupported（return Err）时，其后不再生成 arg_loop/ret_move
    //（否则 `return Err(...)` 后出现不可达代码 → unreachable_code 警告）。
    let body: TokenStream = if call_is_err {
        // 无 CALL 指令：整个 arm 直接 Unsupported（无不可达代码）
        let msg = format!("v12 {op_name} lowering (call inst missing)");
        let msg_lit = syn::LitStr::new(&msg, proc_macro2::Span::call_site());
        quote! {
            crate::prelude::Opcode::#op_ident { .. } => {
                return Err(crate::prelude::IrError::Unsupported(#msg_lit.into()));
            }
        }
    } else {
        quote! {
            crate::prelude::Opcode::#op_ident { .. } => {
                let mut __pack = crate::prelude::InstPacket::new();
                ctx.current_clobbers = vec![
                    #(#gpr_clobbers),*,
                    #(#fpr_clobbers),*,
                ];
                #arg_loop
                #call_body
                #ret_move
                Ok(__pack)
            }
        }
    };
    Ok(body)
}

/// 参数 → ABI 寄存器移动语句（浮点参数按类型分派 XMM；整数按序 GPR）。
/// 整数 mov 指令名按 [abi].ret_mov_inst 泛化；浮点指令缺失时该分支
/// Unsupported（防引用不存在的 Inst 变体）。
#[allow(clippy::too_many_arguments)]
fn arg_move_loop(
    op_name: &str,
    n: &usize,
    fn_: &usize,
    int_regs: &[syn::Ident],
    float_regs: &[syn::Ident],
    m_src: &syn::Ident,
    m_src_idx: u8,
    m_dest: &syn::Ident,
    f_dest: &syn::Ident,
    f_src: &syn::Ident,
    has_ss: bool,
    has_fpr_mov: bool,
    mov_vn: &syn::Ident,
    fpr_mov32_vn: &syn::Ident,
    fpr_mov64_vn: &syn::Ident,
) -> TokenStream {
    let n = *n;
    let fn_ = *fn_;
    let int_stmt = quote! {
        let __idx = __pack.push_inst(Inst::#mov_vn {
            #m_src: Reg::from_index(0, forge_ir::RegClass::GPR64),
            #m_dest: __dst,
        });
        // 参数 vreg 绑 **src 序号**（x86 MOV_RM8_R64 src=op0=0；riscv mv
        // src=op1=1）——按角色泛化，防定宽方向反（`mv vreg, x0` 清零参数）。
        __pack.map_reg_field(__a, __idx, #m_src_idx, false);
    };
    let fpr_stmt: TokenStream = if !float_regs.is_empty() {
        let fpr_regs = float_regs;
        if has_fpr_mov {
            quote! {
                if __fi < #fn_ {
                    let __dst = [#(Reg::#fpr_regs),*][__fi];
                    __fi += 1;
                    let __idx = __pack.push_inst(if ctx.xreg_types.get(&__a).map(|t| t.bits()).unwrap_or(64) == 32 && #has_ss {
                        Inst::#fpr_mov32_vn { #f_dest: __dst, #f_src: Reg::from_index(0, __DEFAULT_FPR_CLASS) }
                    } else {
                        Inst::#fpr_mov64_vn { #f_dest: __dst, #f_src: Reg::from_index(0, __DEFAULT_FPR_CLASS) }
                    });
                    __pack.map_reg_field(__a, __idx, 1u8, false);
                }
            }
        } else {
            quote! {
                return Err(crate::prelude::IrError::Unsupported(
                    "v12 call: float arg move (MOVSD/MOVSS missing)".into(),
                ));
            }
        }
    } else {
        quote! {
            return Err(crate::prelude::IrError::Unsupported(
                "v12 call: float arg move (no float regs)".into(),
            ));
        }
    };
    let stmt = quote! {
        if ctx.xreg_types.get(&__a).is_some_and(|t| {
            ctx.type_ctx.as_ref().is_some_and(|tc| {
                let s = tc.borrow();
                (s.is_vector(*t) || s.is_scalable_vector(*t)) && s.size_bytes(*t) > 16
            })
        }) {
            // 宽向量实参（>16 字节）按引用传参：调用方需栈上副本 + 传指针。
            // 调用方侧栈拷贝尚未落地——显式拒绝（防静默截断成 GPR/XMM 低位）。
            return Err(crate::prelude::IrError::Unsupported(
                "v12 call: wide vector arg (>16B) by-ref caller-side copy not yet supported".into(),
            ));
        } else if ctx.xreg_types.get(&__a).is_some_and(|t| t.is_float()) {
            #fpr_stmt
        } else if __gi < #n {
            let __dst = [#(Reg::#int_regs),*][__gi];
            __gi += 1;
            #int_stmt
        }
    };
    if op_name == "CallIndirect" {
        quote! {
            let mut __gi = 0usize;
            let mut __fi = 0usize;
            for (__i, &__a) in args.iter().enumerate() {
                if __i == 0 { continue; }
                #stmt
            }
        }
    } else {
        quote! {
            let mut __gi = 0usize;
            let mut __fi = 0usize;
            for &__a in args.iter() {
                #stmt
            }
        }
    }
}

/// 展开一条 lowering 模板（符号化操作数）为指令构造语句序列。
/// 从 lowering 规则的 `when` 谓词提取宽度提示（`eq = ["rs1_width", N]` → Some(N/8)），
/// 供 [`gen_lowering_insts`] 候选按槽类宽度过滤（32 位规则优先 gpr32 槽形式）。
fn lowering_width_hint(rule: &Lowering) -> Option<u32> {
    let pred = pred::parse(rule.when.as_ref()?).ok()?;
    pred_width_hint(&pred)
}

fn pred_width_hint(p: &Pred) -> Option<u32> {
    match p {
        Pred::And(ps) => ps.iter().find_map(pred_width_hint),
        Pred::Or(ps) => ps.iter().find_map(pred_width_hint),
        Pred::Not(p) => pred_width_hint(p),
        Pred::Cmp(CmpOp::Eq, name, v) if name == "rs1_width" => Some((*v as u32) / 8),
        _ => None,
    }
}

fn gen_lowering_insts(
    templates: &[String],
    _infos: &[InstInfo],
    name_to_vn: &std::collections::HashMap<&str, Vec<&InstInfo>>,
    width_hint: Option<u32>,
) -> Result<Vec<TokenStream>, String> {
    let mut out = Vec::new();
    for t in templates {
        let trimmed = t.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // `{out} = INST op0, op1` 或 `INST op0, op1`（lhs 仅为文档性语法；
        // 结果 XReg 绑定由占位符注册表 {out}→rd 派生，无需单独消费 lhs）
        let (_lhs, rhs) = match trimmed.split_once('=') {
            Some((l, r)) => (Some(l.trim()), r.trim()),
            None => (None, trimmed),
        };
        let (inst_name, ops) = match rhs.split_once(char::is_whitespace) {
            Some((n, rest)) => (n.trim(), rest.trim()),
            None => (rhs.trim(), ""),
        };
        // 助记符 → 候选指令；同助记符多形状按模板操作数 token 类型消歧
        let cands = name_to_vn
            .get(inst_name)
            .ok_or_else(|| format!("lowering 模板引用了未知指令 '{inst_name}'（行: {trimmed}）"))?;
        let toks: Vec<&str> = if ops.is_empty() {
            Vec::new()
        } else {
            ops.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect()
        };
        // token 类型签名（reg/imm/mem/cond/label）
        let tok_kinds: Vec<&str> = toks.iter().map(|op| lowering_token_kind(op)).collect();
        // 先按操作数签名匹配；若多候选再用字面段（模板 vs asm 的内存括号）
        // 过滤——单候选时字面段差异不阻塞（MemRef 槽指令的 asm 无字面 `[`，
        // 但 lowering 模板用 [base+off] 表达）
        let sig_matches: Vec<&InstInfo> = cands
            .iter()
            .copied()
            .filter(|i| {
                let text_ops: Vec<&str> = i
                    .operands
                    .iter()
                    .filter(|(_, _, s, _)| {
                        matches!(
                            s.kind,
                            OperandKind::Reg
                                | OperandKind::Imm
                                | OperandKind::Mem
                                | OperandKind::Cond
                                | OperandKind::Label
                        )
                    })
                    .map(|(_, _, s, _)| match s.kind {
                        OperandKind::Reg => "reg",
                        OperandKind::Mem => "mem",
                        OperandKind::Cond => "cond",
                        OperandKind::Label => "label",
                        _ => "imm",
                    })
                    .collect();
                text_ops.len() == tok_kinds.len()
                    && text_ops.iter().zip(&tok_kinds).all(|(a, b)| {
                        a == b
                            || (*b == "num" && (*a == "imm" || *a == "cond"))
                            || (*b == "reg" && *a == "reg")
                    })
            })
            .collect();
        let mut matched: Vec<&InstInfo> = if sig_matches.len() <= 1 {
            sig_matches
        } else {
            // 宽度感知：`when = { eq = ["rs1_width", N] }` 规则优先**单类且宽度
            // 匹配**的候选（如 add32 的 gpr32 槽）；**多类（多态）槽保持声明序**
            // ——否则同助记符合并后（mov 式分发）会把 mov {out}, {0} 从
            // MOV_R_RM（operands[0]=dest）重排到 MOV_RM_R（operands[0]=src），
            // 翻转 lowering 的操作数绑定。
            let mut m = sig_matches;
            if let Some(w) = width_hint {
                // 宽度匹配：源/InOut Reg 槽必须恰为 w；Out（结果）Reg 槽——
                // 宽 > w 时若有 Reg 源槽则豁免（movzx dest 恒 64 位，扩展指令
                // 按源槽选变体）；无 Reg 源槽（Load 等 mem 源）则结果宽度决定
                // 指令宽度（64 位 load ≠ 32 位规则判 wrong）。
                m.sort_by_key(|i| {
                    let has_reg_src = i.operands.iter().any(|(_, _, s, role)| {
                        *role != OperandRole::Out && s.kind == OperandKind::Reg
                    });
                    let definitely_wrong = i.operands.iter().any(|(_, _, s, role)| {
                        s.kind == OperandKind::Reg
                            && s.classes()
                                .map(|cs| {
                                    cs.len() == 1 && {
                                        let cw = cs[0].width();
                                        if *role == OperandRole::Out {
                                            // Out：宽 < w 必错；宽 > w 且无 Reg 源
                                            //（Load）也错；有 Reg 源（movzx）豁免
                                            cw < w as u16 || (cw > w as u16 && !has_reg_src)
                                        } else {
                                            cw != w as u16
                                        }
                                    }
                                })
                                .unwrap_or(false)
                    });
                    if definitely_wrong { 1 } else { 0 }
                });
            }
            m
        };
        // 多候选：字面段一致性过滤（模板含内存括号 ↔ asm 含内存括号）
        if matched.len() > 1 {
            let tpl_has_mem = rhs.contains('[') || rhs.contains('(');
            matched.retain(|i| {
                let asm_stripped = strip_placeholder_decls(&i.inst.asm);
                let asm_has_mem = asm_stripped.contains('[') || asm_stripped.contains('(');
                tpl_has_mem == asm_has_mem
            });
        }
        let info = matched
            .first()
            .copied()
            .ok_or_else(|| {
                format!(
                    "lowering 模板 '{trimmed}' 无法匹配助记符 '{inst_name}' 的任一形状（操作数签名不符）"
                )
            })?;
        let vn = &info.vn;
        // 操作数符号 → (ctor 表达式, map_reg_field 的 XReg 表达式)
        // ctor 用占位 0（regalloc 经 xreg_map 分配后 set_reg_field 回填），
        // 与 v11 的 from_index(0) 占位语义一致；XReg 表达式供 map_reg_field。
        let mut bindings: Vec<(String, TokenStream, TokenStream)> = Vec::new();
        if !ops.is_empty() {
            for (i, op) in ops.split(',').map(|s| s.trim()).enumerate() {
                if op.is_empty() {
                    continue;
                }
                let fid = info
                    .operands
                    .get(i)
                    .map(|(_, fid, _, _)| fid.clone())
                    .ok_or_else(|| {
                        format!(
                            "lowering 模板操作数 {i} 超出指令 {inst_name} 的操作数（行: {trimmed}）"
                        )
                    })?;
                let slot = &info.operands[i].2;
                // `[{n}]` 纯基址写法 → 剥括号按 {n} 处理（内存指令的基址槽是 reg；
                // 带偏移/复杂表达式保留给 MemRef 槽）
                let op = if let Some(v) = op.strip_circumfix('[', ']')
                    && !v.contains(['+', '-', '(', ')'])
                {
                    v
                } else {
                    op
                };
                let (ctor_expr, xreg_expr) = if super::placeholder::kind_matches(op, slot.kind) {
                    // 注册表占位符（{out}/{iconst}/{g}…）：ctor 由注册表闭包
                    // 构建，xreg 绑定来自注册表。新增占位符只改 placeholder.rs。
                    let ctor = super::placeholder::ctor_for(op, slot)
                        .expect("kind_matches 为真但 ctor_for 返回 None（注册表不一致）");
                    let xreg = super::placeholder::xreg_for(op);
                    (ctor, xreg)
                } else {
                    // 字面量 fallback（非占位符 token）：
                    // 条件码/物理寄存器/MemRef/立即数——按槽类型解析。
                    match slot.kind {
                        OperandKind::Cond => {
                            // 字面条件码（seto=0/setb=2/…，模板直接写死）
                            let v: u8 = op
                                .parse()
                                .map_err(|_| format!("lowering 模板条件码 '{op}' 无法解析"))?;
                            (quote! { #v }, quote! { 0u32 })
                        }
                        OperandKind::Reg => {
                            // 物理寄存器名（RAX 等）→ Reg 枚举（类型化字段直接写入）
                            let reg = format_ident!("{op}");
                            (quote! { Reg::#reg }, quote! { 0u32 })
                        }
                        OperandKind::Mem => {
                            // 内存操作数：`{base}+{off}` → MemRef（base 物理寄存器 + 偏移）
                            let m = parse_mem_template(op, "lowering 模板")?;
                            (quote! { #m }, quote! { 0u32 })
                        }
                        OperandKind::Imm | OperandKind::Label => {
                            let v: i64 = parse_i64_lit(op)
                                .map_err(|_| format!("lowering 模板立即数 '{op}' 无法解析"))?;
                            (quote! { #v }, quote! { 0u32 })
                        }
                    }
                };
                bindings.push((fid.to_string(), ctor_expr, xreg_expr));
            }
        }
        // 构造 Inst 变体（Reg 字段占位 from_index(0)；物理寄存器/imm 直接写死）
        let fields: Vec<TokenStream> = info
            .operands
            .iter()
            .map(|(_, fid, slot, _)| {
                let fid_ident = format_ident!("{fid}");
                let expr = bindings
                    .iter()
                    .find(|(f, _, _)| f == &fid.to_string())
                    .map(|(_, e, _)| e.clone())
                    .unwrap_or_else(|| field_ctor_expr(slot, quote! { 0u32 }));
                quote! { #fid_ident: #expr }
            })
            .collect();
        let ctor = if info.operands.is_empty() {
            quote! { Inst::#vn }
        } else {
            quote! { Inst::#vn { #(#fields),* } }
        };
        out.push(quote! {
            let __idx = __pack.push_inst(#ctor);
        }); // map_reg_field：Reg 操作数（跳过 Mem 基址等非 Reg 字段；与 v11 一致，
        // 仅 Reg 槽参与 regalloc 映射）
        let mut reg_field_i = 0usize;
        for (_, fid, slot, role) in info.operands.iter() {
            if slot.kind != OperandKind::Reg {
                continue;
            }
            let is_def = matches!(role, OperandRole::Out | OperandRole::InOut);
            let xreg_expr = bindings
                .iter()
                .find(|(f, _, _)| f == &fid.to_string())
                .map(|(_, _, x)| x.clone())
                .unwrap_or_else(|| quote! { 0u32 });
            // 物理寄存器占位（xreg_expr = 0u32 字面量）不参与 map_reg_field，
            // 但 field 序号仍递增（set_reg_field 按 Inst 的 Reg 字段序列
            // 计数，含物理寄存器——与 v11 一致，否则后续操作数错位）。
            if xreg_expr.to_string() != "0u32" {
                out.push(quote! {
                    __pack.map_reg_field(#xreg_expr, __idx, #reg_field_i as u8, #is_def);
                });
            }
            reg_field_i += 1;
        }
    }
    Ok(out)
}
