//! TargetABI / TargetFrameLowering / emit 伪指令（@frame_alloc 等）生成。
//!
//! 从 `integration.rs` 拆分（原 1258-2069 行）。复用父模块的 `InstInfo`、
//! `pascal_ident`（codegen）与 integration 工具（inst_fids/inst_move_role/
//! inst_reg_imm_fids）以及 model 类型。

use super::super::model::*;
use super::integration::{inst_exists, inst_fids, inst_move_role, inst_reg_imm_fids};
use super::{InstInfo, field_ctor_expr};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

// ─────────────────────── TargetABI ───────────────────────

/// 入口：`pub(crate)` 由 `integration::gen_integration()` 调用。
pub(crate) fn gen_abi(model: &V12Model) -> Result<TokenStream, String> {
    // [abi] → arg_regs（按 arg_class 顺序：int 类在前，其余 class 依次）。
    // ret_regs：缺省空（v12 声明层暂不区分返回寄存器——后续迭代扩展）。
    let stack_align = model.abi.as_ref().and_then(|a| a.stack_align).unwrap_or(16);
    let frame_padding = model
        .abi
        .as_ref()
        .and_then(|a| a.frame_padding)
        .unwrap_or(0);
    // 寄存器参数位置上限：int arg_class 的寄存器个数（Windows x64 = 4；
    // 位置 ≥ 此值走栈）。by-class（riscv）无栈参数 → 全部 arg_regs 数。
    let int_arg_slot_count = model
        .abi
        .as_ref()
        .and_then(|a| {
            a.arg_class
                .iter()
                .find(|ac| ac.class == crate::v12::model::ArgClassKind::Int)
                .map(|ac| ac.regs.len())
        })
        .unwrap_or_else(|| {
            model
                .abi
                .as_ref()
                .map(|a| a.arg_class.iter().map(|ac| ac.regs.len()).sum())
                .unwrap_or(0)
        });
    let mut arg_regs: Vec<TokenStream> = Vec::new();
    let mut by_ref_limit: Option<u32> = None;
    if let Some(abi) = &model.abi {
        for ac in &abi.arg_class {
            for r in &ac.regs {
                let i = format_ident!("{r}");
                arg_regs.push(quote! { Reg::#i });
            }
            // by-ref 策略：`strategy = "by-ref"` + `limit`（位）→ 超过该位宽的
            // 向量按引用传参（阈值字节 = limit/8；x86 声明 128 位 → 16 字节）。
            if ac.strategy.as_deref() == Some("by-ref")
                && let Some(bits) = ac.limit
            {
                if bits % 8 != 0 {
                    return Err(format!(
                        "[abi.arg_class.{}]: by-ref limit {bits} 不是 8 的倍数（应为位宽）",
                        ac.class.name()
                    ));
                }
                by_ref_limit = Some(bits / 8);
            }
        }
    }
    let by_ref_toks: TokenStream = match by_ref_limit {
        Some(b) => quote! { Some(#b) },
        None => quote! { None },
    };
    let min_frame = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|f| f.min_frame_bytes)
        .unwrap_or(0);
    let csb_override = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|f| f.callee_saved_bytes_override);
    let csb_toks: TokenStream = match csb_override {
        Some(v) => quote! { Some(#v) },
        None => quote! { None },
    };
    let sss = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|f| f.stack_slot_shift);
    let sss_toks: TokenStream = match sss {
        Some(v) => quote! { Some(#v) },
        None => quote! { None },
    };
    // 返回寄存器：[abi].ret_regs（物理名）→ Reg::NAME；缺省空 = index 0
    //（x86 RAX 语义，由 Return/Call lowering 的 from_index(0) 兜底）。
    let mut ret_regs: Vec<TokenStream> = Vec::new();
    if let Some(abi) = &model.abi {
        for r in &abi.ret_regs {
            let i = format_ident!("{r}");
            ret_regs.push(quote! { Reg::#i });
        }
    }
    // 若没有 arg_class（或 regs 空）→ 空 arg_regs（集成层仍可组装）。
    Ok(quote! {
        pub struct ABI;

        impl crate::machine::abi::TargetABI for ABI {
            type Reg = Reg;

            fn arg_regs(&self) -> Vec<Self::Reg> {
                vec![#(#arg_regs),*]
            }
            fn ret_regs(&self) -> Vec<Self::Reg> {
                vec![#(#ret_regs),*]
            }
            fn stack_align(&self) -> u32 { #stack_align }
            fn frame_padding(&self) -> i32 { #frame_padding }
            fn vector_by_ref_limit(&self) -> Option<u32> { #by_ref_toks }
            fn min_frame_bytes(&self) -> u32 { #min_frame }
            fn callee_saved_bytes_override(&self) -> Option<u32> { #csb_toks }
            fn stack_slot_shift(&self) -> Option<i32> { #sss_toks }
            fn int_arg_slot_count(&self) -> usize { #int_arg_slot_count }
        }
    })
}

// ─────────────────────── TargetFrameLowering ───────────────────────

/// 入口：`pub(crate)` 由 `integration::gen_integration()` 调用。
pub(crate) fn gen_frame_lowering(
    infos: &[InstInfo],
    model: &V12Model,
) -> Result<TokenStream, String> {
    // 指令名 → InstInfo（[emit]/[spill] 模板用大写指令名引用）。
    let prologue_toks = gen_emit_block(infos, model, true)?;
    let epilogue_toks = gen_emit_block(infos, model, false)?;

    // 独立尾声标签：缺省 true（x86 语义：return block 经 epilogue_jump 跳到
    // 统一尾声）。定宽 ISA 无 JMP 指令时可设 false（[emit].epilogue_label）：
    // return block 不发 jump，尾声直接顺序发在 return block 之后——仅单
    // return block 函数安全（多 return block 会 fall-through 错序）。
    let needs_epilogue_label = model
        .emit
        .as_ref()
        .and_then(|e| e.epilogue_label)
        .unwrap_or(true);

    // 尾声跳转指令选择：`[emit].epilogue_jump_inst` 或 `[abi].jump_inst`
    // 键；缺省按 ISA 形态固定（变长 x86 → JMP_REL32 0xE9 rel32 手写；
    // 定宽 riscv → JAL x0, epilogue_label——label 槽 = 块号 → encoder 定宽
    // fixup Relative(4,0)，位段由 RiscvRelocPatcher 编码）。
    // **不做按名存在性猜测**：指令不存在时 inst_fids 查找报错。
    let explicit_jump = model
        .emit
        .as_ref()
        .and_then(|e| e.epilogue_jump_inst.clone());
    let abi_jump = model.abi.as_ref().and_then(|a| a.jump_inst.clone());
    let jump_inst = explicit_jump.or(abi_jump).unwrap_or_else(|| {
        if model.meta.variable_length {
            "JMP_REL32".to_string()
        } else {
            "JAL".to_string()
        }
    });
    // 尾声跳转统一走 encoder：jump_inst 指令存在即可（`inst_exists`，
    // 与操作数无关）。变长（x86 JMP_REL32）label 槽 = 尾部 imm → encoder
    // 发 REL4 fixup（与旧手写 0xE9+use_label_at 字节等价）；定宽（riscv
    // JAL）label 槽 = 位段 → Relative(4,0) fixup（patcher 重排）。**不按
    // 指令名判断形态**——变长/定宽由 `[meta].variable_length` 决定。
    let jump_f = inst_fids(infos, &jump_inst);
    let has_jump = inst_exists(infos, &jump_inst);
    let jump_vn = crate::v12::codegen::pascal_ident(&jump_inst);
    let jmp_rel = jump_f
        .first()
        .map(|f| (*f).clone())
        .unwrap_or_else(|| format_ident!("target"));
    let (jal_dest, jal_target) = if jump_f.len() >= 2 {
        (jump_f[0].clone(), jump_f[1].clone())
    } else {
        (format_ident!("dest"), format_ident!("target"))
    };
    let epilogue_jump_body: TokenStream = if has_jump && model.meta.variable_length {
        quote! {
            // 变长（x86 JMP_REL32 语义）：label 槽 = 块号 → encoder 编码 +
            // use_label_at（REL4 fixup = 指令末尾 rel32 占位）。
            let inst = Inst::#jump_vn { #jmp_rel: epilogue_block.0 as i64 };
            encoder.encode(&inst, reg_map, sink).map_err(|e| {
                crate::IrError::Internal(format!("epilogue jump encode: {e}"))
            })?;
            Ok(())
        }
    } else if has_jump {
        quote! {
            // 定宽（riscv JAL 语义）：jal x0, epilogue_block——label 槽 = 块号
            // → encoder 编码时 use_label_at（定宽 fixup = 指令起始；位段重排
            // 由 patcher）。
            let inst = Inst::#jump_vn {
                #jal_dest: Reg::from_index(0, forge_ir::RegClass::GPR64),
                #jal_target: epilogue_block.0 as i64,
            };
            encoder.encode(&inst, reg_map, sink).map_err(|e| {
                crate::IrError::Internal(format!("epilogue jump encode: {e}"))
            })?;
            Ok(())
        }
    } else {
        quote! {
            Err(crate::IrError::Unsupported(
                "epilogue jump: no jump_inst ([abi].jump_inst / [emit].epilogue_jump_inst)".into(),
            ))
        }
    };

    // spill load/store：`{0}` = 寄存器、`{1}` = 帧偏移、基址来自模板 base。
    // 模板未声明 base 时缺省取 [abi.frame].fp（x86 RBP / riscv X8）——
    // 二者正是帧指针语义；再回退 "RBP"。
    let default_base = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|fr| fr.fp.clone())
        .unwrap_or_else(|| "RBP".to_string());
    let spill_gpr = model.spill.get("GPR");
    let spill_fpr = model.spill.get("FPR");
    let gpr_load = gen_spill_stmt(infos, spill_gpr, true, &default_base)?;
    let gpr_store = gen_spill_stmt(infos, spill_gpr, false, &default_base)?;
    let fpr_load = gen_spill_stmt(infos, spill_fpr, true, &default_base)?;
    let fpr_store = gen_spill_stmt(infos, spill_fpr, false, &default_base)?;

    Ok(quote! {
        pub struct FrameLowering;

        impl crate::machine::frame::TargetFrameLowering for FrameLowering {
            type Inst = Inst;

            fn needs_epilogue_label(&self) -> bool { #needs_epilogue_label }

            fn emit_epilogue_jump(
                &self,
                encoder: &std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Self::Inst>>,
                reg_map: &crate::AllocResult,
                epilogue_block: forge_ir::Block,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::IrError> {
                #epilogue_jump_body
            }

            fn emit_prologue(
                &self,
                frame_size: u32,
                reg_map: &crate::AllocResult,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::IrError> {
                let __frame_size = frame_size;
                let __rm = reg_map;
                let __sink = sink;
                #prologue_toks
                Ok(())
            }

            fn emit_epilogue(
                &self,
                frame_size: u32,
                reg_map: &crate::AllocResult,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::IrError> {
                let __frame_size = frame_size;
                let __rm = reg_map;
                let __sink = sink;
                #epilogue_toks
                Ok(())
            }

            fn emit_spill_load(
                &self,
                dst_reg: u32,
                offset: i32,
                width: u16,
                is_fp: bool,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::IrError> {
                let __dst = dst_reg;
                let __off = offset as i64;
                let __sink = sink;
                if is_fp {
                    #fpr_load
                } else {
                    #gpr_load
                }
                let _ = width;
                Ok(())
            }

            fn emit_spill_store(
                &self,
                src_reg: u32,
                offset: i32,
                width: u16,
                is_fp: bool,
                sink: &mut crate::CodeSink,
            ) -> Result<(), crate::IrError> {
                let __dst = src_reg;
                let __off = offset as i64;
                let __sink = sink;
                if is_fp {
                    #fpr_store
                } else {
                    #gpr_store
                }
                let _ = width;
                Ok(())
            }
        }
    })
}

/// 展开 [emit] prologue/epilogue 模板为编码语句序列。
///
/// 模板行：`INST op0, op1, ...`（物理寄存器/数字立即数）或 `@伪指令`
/// （@push_callee/@pop_callee/@frame_alloc/@frame_free）。emit 模式：
/// 无 XReg 映射——物理寄存器写死物理索引，imm 写死值，直接编码进 sink。
fn gen_emit_block(
    infos: &[InstInfo],
    model: &V12Model,
    is_prologue: bool,
) -> Result<TokenStream, String> {
    let Some(emit) = &model.emit else {
        return Ok(quote! {});
    };
    let block = if is_prologue {
        emit.prologue.as_ref()
    } else {
        emit.epilogue.as_ref()
    };
    let Some(block) = block else {
        return Ok(quote! {});
    };
    let mut out: Vec<TokenStream> = Vec::new();
    for t in &block.insts {
        let trimmed = t.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // @ 伪指令
        if let Some(name) = trimmed.strip_prefix('@') {
            out.push(gen_emit_pseudo(infos, model, name)?);
            continue;
        }
        let stmts = gen_emit_inst(infos, trimmed)?;
        out.extend(stmts);
    }
    Ok(quote! { #(#out)* })
}

/// 单个 emit 指令行 → 构造 + 编码语句（emit 模式）。
fn gen_emit_inst(infos: &[InstInfo], line: &str) -> Result<Vec<TokenStream>, String> {
    let (inst_name, ops) = match line.split_once(char::is_whitespace) {
        Some((n, rest)) => (n.trim(), rest.trim()),
        None => (line.trim(), ""),
    };
    let info = inst_info_by_name(infos, inst_name)
        .ok_or_else(|| format!("emit 模板引用了未知指令 '{inst_name}'（行: {line}）"))?;
    let vn = &info.vn;
    let mut bindings: Vec<(String, TokenStream)> = Vec::new();
    if !ops.is_empty() {
        for (i, op) in ops.split(',').map(|s| s.trim()).enumerate() {
            if op.is_empty() {
                continue;
            }
            let fid = info
                .operands
                .get(i)
                .map(|(_, fid, _, _)| fid.clone())
                .ok_or_else(|| format!("emit 模板操作数 {i} 超出指令 {inst_name}（行: {line}）"))?;
            let slot = &info.operands[i].2;
            let expr: TokenStream = match op {
                other if slot.kind == OperandKind::Reg => {
                    // 物理寄存器名 → Reg 枚举（emit 模式直接写死）
                    let reg = format_ident!("{other}");
                    quote! { Reg::#reg }
                }
                other if slot.kind == OperandKind::Imm || slot.kind == OperandKind::Label => {
                    // 占位符：{frame_size} / {frame_size_neg} / {frame_size_mN}
                    //（emit 模式下由运行时 frame_size 派生；riscv 的 ra/fp
                    // 保存槽偏移 = frame_size - N）
                    let expr: TokenStream = match other {
                        "{frame_size}" => quote! { __frame_size as i64 },
                        "{frame_size_neg}" => quote! { -(__frame_size as i64) },
                        _ if other.starts_with("{frame_size_m") && other.ends_with('}') => {
                            let n: i64 = other["{frame_size_m".len()..other.len() - 1]
                                .parse()
                                .map_err(|_| format!("emit 模板占位符 '{other}' 无法解析"))?;
                            quote! { (__frame_size as i64) - #n }
                        }
                        _ => {
                            let v: i64 = other
                                .parse()
                                .map_err(|_| format!("emit 模板立即数 '{other}' 无法解析"))?;
                            quote! { #v }
                        }
                    };
                    expr
                }
                other => {
                    return Err(format!(
                        "emit 模板操作数 '{other}' 不支持（指令 {inst_name} 槽 {}）",
                        slot.kind.kind_name()
                    ));
                }
            };
            bindings.push((fid.to_string(), expr));
        }
    }
    let fields: Vec<TokenStream> = info
        .operands
        .iter()
        .map(|(_, fid, slot, _)| {
            let fid_ident = format_ident!("{fid}");
            let expr = bindings
                .iter()
                .find(|(f, _)| f == &fid.to_string())
                .map(|(_, e)| e.clone())
                .unwrap_or_else(|| field_ctor_expr(slot, quote! { 0u32 }));
            quote! { #fid_ident: #expr }
        })
        .collect();
    let ctor = if info.operands.is_empty() {
        quote! { Inst::#vn }
    } else {
        quote! { Inst::#vn { #(#fields),* } }
    };
    Ok(vec![quote! {
        let __bytes = encode(&#ctor).map_err(|e| crate::IrError::Emit(e))?;
        __sink.put_bytes(&__bytes);
    }])
}

/// 按指令名查找 InstInfo（emit/spill 模板用）。
fn inst_info_by_name<'a>(infos: &'a [InstInfo<'a>], name: &str) -> Option<&'a InstInfo<'a>> {
    infos.iter().find(|i| i.inst.name == name)
}

/// emit @ 伪指令（push_callee/pop_callee/frame_alloc/frame_free）。
fn gen_emit_pseudo(
    infos: &[InstInfo],
    model: &V12Model,
    name: &str,
) -> Result<TokenStream, String> {
    // callee-saved 区字节数（生成期常量，与 frame_layout::callee_saved_bytes
    // 一致：fp 保存槽 + callee-saved × 8）——move_args 收 spilled 栈参数时
    // 计算 spill 槽地址 sp_base = -(frame) - callee_saved + stack_arg_bytes。
    let callee_saved_bytes_lit: i64 = {
        let fp_push = model
            .abi
            .as_ref()
            .and_then(|a| a.frame.as_ref())
            .and_then(|f| f.fp_push_bytes)
            .unwrap_or(8) as i64;
        let cs = model
            .abi
            .as_ref()
            .and_then(|a| a.callee_saved.as_ref())
            .map(|c| c.gpr.len() as i64 * 8)
            .unwrap_or(0);
        fp_push + cs
    };
    match name {
        "push_callee" | "pop_callee" => {
            let Some(abi) = &model.abi else {
                return Ok(quote! {});
            };
            let Some(callee) = &abi.callee_saved else {
                return Ok(quote! {});
            };
            if callee.gpr.is_empty() {
                return Ok(quote! {});
            }
            // x86：PUSH/POP（+r 形式，硬件递减 sp）。定宽 ISA 无 push/pop
            // 指令（riscv）→ 用 [spill.GPR] store/load 模板指令（SD/LD）存到
            // 帧槽 [sp + frame - fp_push - (k+1)*8]（frame_alloc 之后执行，
            // 槽在已分配帧顶部 fp_push 区域之下；min_frame_bytes 需覆盖）。
            // `[abi].push_inst`/`pop_inst` 键驱动（缺省按 PUSH/POP 存在性检测）。
            let push_inst = abi.push_inst.clone().unwrap_or_else(|| "PUSH".to_string());
            let pop_inst = abi.pop_inst.clone().unwrap_or_else(|| "POP".to_string());
            let has_hw_push = inst_exists(infos, &push_inst) && inst_exists(infos, &pop_inst);
            if !has_hw_push {
                let Some(frame) = &abi.frame else {
                    return Err(
                        "@push_callee: 定宽 ISA 需要 [abi.frame]（sp/fp_push_bytes）".into(),
                    );
                };
                let sp = format_ident!("{}", frame.sp);
                let fp_push = frame.fp_push_bytes.unwrap_or(8) as i64;
                let spill_tpl = model
                    .spill
                    .get("GPR")
                    .ok_or_else(|| "@push_callee: 定宽 ISA 需要 [spill.GPR]".to_string())?;
                let template = if name == "push_callee" {
                    spill_tpl.store.as_str()
                } else {
                    spill_tpl.load.as_str()
                };
                let inst_name = template
                    .split(char::is_whitespace)
                    .next()
                    .ok_or_else(|| "spill 模板为空".to_string())?;
                let vn = crate::v12::codegen::pascal_ident(inst_name);
                let fid_list = inst_fids(infos, inst_name);
                let (f_reg, f_base, f_imm) = match fid_list.len() {
                    3 => (
                        fid_list[0].clone(),
                        fid_list[1].clone(),
                        fid_list[2].clone(),
                    ),
                    _ => {
                        return Err(format!(
                            "@push_callee: [{inst_name}] 需 3 操作数 (reg, base, imm)"
                        ));
                    }
                };
                // **按需保存**（阶段 G）：只 push/pop 实际分配到的 callee-saved
                //（regalloc 的 callee_saved_to_save；不再全量保存 TOML 列表）。
                // 槽偏移按正向声明序：push 正向遍历（saved[k]→槽 k）；
                // pop **逆序遍历 + 槽 n-1-k**：恢复的寄存器 = 正向[n-1-k]，
                // 槽也必须 = n-1-k（reg 与槽同源）——否则 pop 读错槽
                //（实测返回值错/挂起）。
                let push = name == "push_callee";
                let (iter, k_expr) = if push {
                    (quote! { __saved.iter() }, quote! { __k as i64 })
                } else {
                    (
                        quote! { __saved.iter().rev() },
                        quote! { __n as i64 - 1 - __k as i64 },
                    )
                };
                return Ok(quote! {
                    let __saved = __rm.callee_saved_to_save.clone();
                    let __n = __saved.len();
                    for (__k, __preg) in #iter.enumerate() {
                        let __reg = Reg::from_index(__preg.num, __preg.class);
                        let __off_val: i64 = (#k_expr + 1) * 8;
                        let __off = __frame_size as i64 - #fp_push - __off_val;
                        let __bytes = encode(&Inst::#vn {
                            #f_reg: __reg,
                            #f_base: Reg::#sp,
                            #f_imm: __off,
                        })
                        .map_err(|e| crate::IrError::Emit(e))?;
                        __sink.put_bytes(&__bytes);
                    }
                });
            }
            let regs: Vec<String> = if name == "push_callee" {
                callee.gpr.clone()
            } else {
                callee.gpr.iter().rev().cloned().collect()
            };
            let push_pop = if name == "push_callee" {
                &push_inst
            } else {
                &pop_inst
            };
            let vn = crate::v12::codegen::pascal_ident(push_pop);
            // 单操作数 +r 形式：PUSH→src（in）、POP→dest（out）——字段名取操作数 0
            let f0 = inst_fids(infos, push_pop)
                .first()
                .cloned()
                .ok_or_else(|| format!("[{push_pop}] missing operand"))?;
            let mut stmts = Vec::new();
            for r in &regs {
                let reg = format_ident!("{r}");
                stmts.push(quote! {
                    let __bytes = encode(&Inst::#vn { #f0: Reg::#reg })
                        .map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__bytes);
                });
            }
            Ok(quote! { #(#stmts)* })
        }
        "move_args" => {
            // 收参：把 [abi.arg_class].int 类的寄存器值 mov 到参数 XReg 的
            // 分配物理寄存器。整数参数按序取 int 类 regs[gi]（v11 语义）。
            let Some(abi) = &model.abi else {
                return Ok(quote! {});
            };
            let int_regs: Vec<&String> = abi
                .arg_class
                .iter()
                .find(|ac| ac.class == crate::v12::model::ArgClassKind::Int)
                .map(|ac| ac.regs.iter().collect())
                .unwrap_or_default();
            if int_regs.is_empty() {
                return Ok(quote! {});
            }
            let regs: Vec<syn::Ident> = int_regs.iter().map(|r| format_ident!("{r}")).collect();
            let n = regs.len();
            // 浮点参数类（XMM0-3）——Windows x64 浮点参数寄存器
            let float_regs: Vec<syn::Ident> = abi
                .arg_class
                .iter()
                .find(|ac| ac.class == crate::v12::model::ArgClassKind::Float)
                .map(|ac| ac.regs.iter().map(|r| format_ident!("{r}")).collect())
                .unwrap_or_default();
            let fn_ = float_regs.len();
            // MOV_RM8_R64 src=arg_reg、dest=param 分配寄存器（0x89: reg=src、rm=dest）
            // 指令名可配置（[abi].move_inst，缺省 "MOV_RM8_R64"）——demo 等
            // 定宽 ISA 声明自己的 mov（如 "MOV64"）；字段按角色解析（In=src、
            // Out/InOut=dest），两种操作数序（x86 src/dest 与 demo dest/src）皆可。
            let move_inst = model
                .abi
                .as_ref()
                .and_then(|a| a.move_inst.clone())
                .unwrap_or_else(|| "MOV_RM8_R64".to_string());
            let mov_vn = crate::v12::codegen::pascal_ident(&move_inst);
            let (m_src, _m_src_idx, m_dest, _m_dest_idx) = inst_move_role(infos, &move_inst)
                .ok_or_else(|| {
                    format!("[{move_inst}] must have In(src)/Out|InOut(dest) reg operands")
                })?;
            // Windows x64 栈参数 load：按语义标签 `stack_arg_load` 收集
            //（TOML 显式声明，不做按指令名探测——第三轮重构原则）。
            // 生成期门控：shadow 已声明但标签缺失 → Unsupported。
            let stack_shadow_ref: TokenStream = match model.abi.as_ref().and_then(|a| a.stack_arg_shadow) {
                Some(v) => quote! { Some(#v) },
                None => quote! { None },
            };
            let stack_load_tagged = crate::v12::codegen::lowering::insts_by_tag(infos, "stack_arg_load");
            let stack_store_tagged = crate::v12::codegen::lowering::insts_by_tag(infos, "stack_arg_store");
            let (int_stack_load, il_mem, il_reg) =
                if model.abi.as_ref().and_then(|a| a.stack_arg_shadow).is_some() {
                    match stack_load_tagged.first() {
                        Some(info) => {
                            let (reg, mem, _reg_idx) =
                                crate::v12::codegen::lowering::reg_mem_fids(info);
                            match (reg, mem) {
                                (Some(r), Some(m)) => (
                                    crate::v12::codegen::pascal_ident(&info.inst.name),
                                    m,
                                    r,
                                ),
                                _ => {
                                    return Err("move_args: [stack_arg_load] tag must be on Reg+Mem inst".into())
                                }
                            }
                        }
                        None => {
                            return Err(
                                "move_args: stack_arg_shadow declared but [stack_arg_load] tag missing"
                                    .into(),
                            )
                        }
                    }
                } else {
                    (format_ident!("Mov64Rm"), format_ident!("mem"), format_ident!("dest"))
                };
            // spilled 栈参数 store（ABI 槽 → spill 槽中转用 MOV64_MR：
            // Reg 槽 op0、Mem 槽 op1，与调用方 store 同标签）
            let (int_stack_store, is_mem, is_reg) =
                if model.abi.as_ref().and_then(|a| a.stack_arg_shadow).is_some() {
                    match stack_store_tagged.first() {
                        Some(info) => {
                            let (reg, mem, _reg_idx) =
                                crate::v12::codegen::lowering::reg_mem_fids(info);
                            match (reg, mem) {
                                (Some(r), Some(m)) => (
                                    crate::v12::codegen::pascal_ident(&info.inst.name),
                                    m,
                                    r,
                                ),
                                _ => {
                                    return Err("move_args: [stack_arg_store] tag must be on Reg+Mem inst".into())
                                }
                            }
                        }
                        None => {
                            return Err(
                                "move_args: stack_arg_shadow declared but [stack_arg_store] tag missing"
                                    .into(),
                            )
                        }
                    }
                } else {
                    (format_ident!("Mov64Mr"), format_ident!("mem"), format_ident!("src"))
                };
            // 栈参数收参的 scratch 寄存器（[abi].scratch 首项，缺省 R10）
            // 与 callee-saved 区字节数（sp_base 计算常量）。
            let scratch0 = abi
                .scratch
                .first()
                .map(|s| format_ident!("{s}"))
                .unwrap_or_else(|| format_ident!("R10"));
            let cs_bytes = callee_saved_bytes_lit;
            // 浮点参数移动：`[abi].fpr_mov_inst`/`fpr_mov_inst32` 键
            //（缺省 "MOVSD"/"MOVSS"；fpr out, fpr in）。
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
            let fpr_fids = inst_fids(infos, &fpr_mov64);
            let has_fpr_mov = fpr_fids.len() >= 2 && inst_fids(infos, &fpr_mov32).len() >= 2;
            let (f_dest, f_src) = if fpr_fids.len() >= 2 {
                (fpr_fids[0].clone(), fpr_fids[1].clone())
            } else {
                (format_ident!("dest"), format_ident!("src"))
            };
            // 变体名按键派生（fpr_mov_inst/fpr_mov_inst32），不硬编码
            // Inst::Movsd/Inst::Movss。
            let fpr_mov64_vn = crate::v12::codegen::pascal_ident(&fpr_mov64);
            let fpr_mov32_vn = crate::v12::codegen::pascal_ident(&fpr_mov32);
            // by-ref 向量 load：宽向量参数（>16 字节）按引用传参——ABI 传 GPR
            // 指针（int 槽位），收参时从 [ptr] load 到目标向量寄存器。
            // 优先非对齐 VMOVUPS（指针未必 32 字节对齐）；缺省回退 VMOVAPS。
            // 宽度由参数 XReg 决定：32 字节（V256）→ VMOVUPS_RM（VEX ymm）；
            // 64 字节（V512）→ VMOVUPS_ZMM_MEM（EVEX zmm）。
            let byref_loads: Vec<(&str, &str, u16)> = vec![
                ("VMOVUPS_RM", "VMOVAPS_RM", 32),
                ("VMOVUPS_ZMM_MEM", "VMOVAPS_ZMM_MEM", 64),
            ];
            let mut byref_stmt: TokenStream = quote! {
                return Err(crate::IrError::Emit(
                    "v12 by-ref vector arg load missing (VMOVUPS_RM/VMOVUPS_ZMM_MEM)".into(),
                ));
            };
            for (inst_name, fallback, width) in byref_loads {
                let fids = if inst_fids(infos, inst_name).len() == 2 {
                    inst_fids(infos, inst_name)
                } else {
                    inst_fids(infos, fallback)
                };
                if fids.len() != 2 {
                    continue;
                }
                let d_fid = fids[0].clone();
                let m_fid = fids[1].clone();
                let vn = crate::v12::codegen::pascal_ident(inst_name);
                let w = width;
                byref_stmt = quote! {
                    if __pv.width() == #w {
                        let __bytes = encode(&Inst::#vn {
                            #d_fid: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                            #m_fid: MemRef {
                                base: Reg::from_index(__src.to_index(), forge_ir::RegClass::GPR64),
                                disp: 0,
                                index: None,
                                scale: 1,
                            },
                        }).map_err(|e| crate::IrError::Emit(e))?;
                        __sink.put_bytes(&__bytes);
                    } else {
                        #byref_stmt
                    }
                };
            }
            let byref_stmt = byref_stmt;
            let fpr_stmt: TokenStream = if has_fpr_mov {
                quote! {
                    if __fi < #fn_ {
                        let __src = [#(Reg::#float_regs),*][__fi];
                        __fi += 1;
                        let __bytes = encode(&if __rm.param_is_32.get(__i) == Some(&true) {
                            Inst::#fpr_mov32_vn {
                                #f_dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                                #f_src: __src,
                            }
                        } else {
                            Inst::#fpr_mov64_vn {
                                #f_dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                                #f_src: __src,
                            }
                        }).map_err(|e| crate::IrError::Emit(e))?;
                        __sink.put_bytes(&__bytes);
                    }
                }
            } else {
                quote! {
                    let _ = __fi;
                    return Err(crate::IrError::Emit("v12 float args (MOVSD/MOVSS missing)".into()));
                }
            };
            // ABI 槽位规则：by-position（Windows x64——int/float 共享位置
            // 计数，参数 i 用 GPR{i}/XMM{i}）/ by-class（缺省，独立推进）。
            let by_position =
                model.abi.as_ref().and_then(|a| a.arg_slot.as_deref()) == Some("by-position");
            let (head, fpr_stmt_use, int_stmt_use, byref_stmt_use): (
                TokenStream,
                TokenStream,
                TokenStream,
                TokenStream,
            ) = if by_position {
                // by-position：位置 = 参数序号 + sret 偏移；int 用 GPR{pos}、
                // float 用 XMM{pos}、by-ref 指针用 GPR{pos}。
                (
                    quote! {},
                    // 浮点收参：float_regs[__pos]（位置索引）
                    quote! {
                        let __pos = __i + if __rm.sret { 1usize } else { 0usize };
                        if __pos < #fn_ {
                            let __src = [#(Reg::#float_regs),*][__pos];
                            let __bytes = encode(&if __rm.param_is_32.get(__i) == Some(&true) {
                                Inst::#fpr_mov32_vn {
                                    #f_dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                                    #f_src: __src,
                                }
                            } else {
                                Inst::#fpr_mov64_vn {
                                    #f_dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                                    #f_src: __src,
                                }
                            }).map_err(|e| crate::IrError::Emit(e))?;
                            __sink.put_bytes(&__bytes);
                        } else {
                            return Err(crate::IrError::Emit(
                                "v12 move_args: float arg position out of range".into(),
                            ));
                        }
                    },
                    // 整数收参：int_regs[__pos]；位置 ≥ 寄存器数 → 栈参数
                    //（[rbp + 16 + shadow + (pos-n)*8]：入口 rsp 指向返回地址，
                    // 返回地址 8 + shadow 之后是第 n 个栈参数）
                    quote! {
                        let __pos = __i + if __rm.sret { 1usize } else { 0usize };
                        if __pos < #n {
                            let __src = [#(Reg::#regs),*][__pos];
                            let __bytes = encode(&Inst::#mov_vn {
                                #m_src: __src,
                                #m_dest: Reg::from_index(__dest, forge_ir::RegClass::GPR64),

                            }).map_err(|e| crate::IrError::Emit(e))?;
                            __sink.put_bytes(&__bytes);
                        } else if let Some(__shadow) = #stack_shadow_ref {
                            let __off = (16i64 + __shadow as i64) + (__pos - #n) as i64 * 8;
                            let __bytes = encode(&Inst::#int_stack_load {
                                #il_mem: MemRef {
                                    base: Reg::RBP,
                                    disp: __off,
                                    index: None,
                                    scale: 1,
                                },
                                #il_reg: Reg::from_index(__dest, forge_ir::RegClass::GPR64),
                            }).map_err(|e| crate::IrError::Emit(e))?;
                            __sink.put_bytes(&__bytes);
                        } else {
                            return Err(crate::IrError::Emit(
                                "v12 move_args: int arg position out of range".into(),
                            ));
                        }
                    },
                    // by-ref 指针收参：int_regs[__pos]（指针从 GPR 槽取）
                    quote! {
                        let __pos = __i + if __rm.sret { 1usize } else { 0usize };
                        if __pos < #n {
                            let __src = [#(Reg::#regs),*][__pos];
                            #byref_stmt
                        } else {
                            return Err(crate::IrError::Emit(
                                "v12 move_args: by-ref arg position out of range".into(),
                            ));
                        }
                    },
                )
            } else {
                (
                    // by-class：__gi（int）/ __fi（float）独立推进
                    quote! {
                        let mut __gi = if __rm.sret { 1usize } else { 0usize };
                        let mut __fi = 0usize;
                    },
                    quote! { #fpr_stmt },
                    quote! {
                        if __gi < #n {
                            let __src = [#(Reg::#regs),*][__gi];
                            __gi += 1;
                            let __bytes = encode(&Inst::#mov_vn {
                                #m_src: __src,
                                #m_dest: Reg::from_index(__dest, forge_ir::RegClass::GPR64),

                            }).map_err(|e| crate::IrError::Emit(e))?;
                            __sink.put_bytes(&__bytes);
                        }
                    },
                    quote! {
                        if __gi < #n {
                            let __src = [#(Reg::#regs),*][__gi];
                            __gi += 1;
                            #byref_stmt
                        }
                    },
                )
            };
            let _ = byref_stmt_use;
            // shadow 未声明（riscv/demo 无栈参数）→ 栈参数收参分支整体不生成
            //（否则分支体引用 RBP/R10/MOV64_RM 等不存在的 Reg/Inst 变体）。
            let has_stack_arg = model.abi.as_ref().and_then(|a| a.stack_arg_shadow).is_some();
            let stack_arg_receive: TokenStream = if has_stack_arg {
                quote! {
                    // 栈参数（位置 ≥ 寄存器数且 shadow 声明）**无条件**收参到
                    // spill 槽（regalloc 强制 spill 保证有槽；即使分配了 preg
                    // 也不走寄存器分支——否则与低位置参数共享寄存器时，批量
                    // 收参顺序覆盖（a→r15 后 e→r15，a 值丢）→ five_args 14）。
                    let __pos = __i + if __rm.sret { 1usize } else { 0usize };
                    if let Some(__shadow) = #stack_shadow_ref
                        && __pos >= #n
                        && __rm.spill_slots.contains_key(&__pv)
                    {
                        let __off = (16i64 + __shadow as i64) + (__pos - #n) as i64 * 8;
                        let __sp_base = -(__frame_size as i64) - __cs_bytes + __rm.stack_arg_bytes as i64;
                        let __slot_off = __rm.spill_slot(__pv).offset as i64;
                        // load ABI 槽 → scratch
                        let __lbytes = encode(&Inst::#int_stack_load {
                            #il_mem: MemRef {
                                base: Reg::RBP,
                                disp: __off,
                                index: None,
                                scale: 1,
                            },
                            #il_reg: __scratch0,
                        }).map_err(|e| crate::IrError::Emit(e))?;
                        __sink.put_bytes(&__lbytes);
                        // store scratch → spill 槽
                        let __sbytes = encode(&Inst::#int_stack_store {
                            #is_mem: MemRef {
                                base: Reg::RBP,
                                disp: __sp_base + __slot_off,
                                index: None,
                                scale: 1,
                            },
                            #is_reg: __scratch0,
                        }).map_err(|e| crate::IrError::Emit(e))?;
                        __sink.put_bytes(&__sbytes);
                        continue;
                    }
                }
            } else {
                quote! {}
            };
            let stack_arg_prologue: TokenStream = if has_stack_arg {
                quote! {
                    let __scratch0 = Reg::#scratch0;
                    let __cs_bytes = #cs_bytes;
                }
            } else {
                quote! {}
            };
            Ok(quote! {
                #head
                // 栈参数收参的 scratch（[abi].scratch 首项）与 callee-saved
                // 字节数（sp_base 计算）——仅 shadow 声明时使用
                #stack_arg_prologue
                for (__i, &__pv) in __rm.param_vregs.iter().enumerate() {
                    #stack_arg_receive
                    if !__rm.assignments.contains_key(&__pv) {
                        continue;
                    }
                    let __dest = match __rm.preg(__pv) {
                        Some(p) => p.num,
                        None => {
                            continue;
                        }
                    };
                    if __rm.param_by_ref.get(__i) == Some(&true) {
                        // by-ref：宽向量参数按引用传——GPR 槽位是数据指针，
                        // 从 [ptr] load 到向量寄存器（VMOVAPS_RM/ZMM_MEM）。
                        #byref_stmt_use
                    } else if __rm.param_is_float.get(__i) == Some(&true) {
                        #fpr_stmt_use
                    } else {
                        #int_stmt_use
                    }
                }
            })
        }
        "frame_alloc" => {
            let Some(abi) = &model.abi else {
                return Ok(quote! {});
            };
            let Some(frame) = &abi.frame else {
                return Ok(quote! {});
            };
            let inst = frame
                .alloc_inst
                .as_deref()
                .ok_or_else(|| "[abi.frame].alloc_inst required for @frame_alloc".to_string())?;
            // 帧分配：所有 Reg 槽填 sp、所有 Imm/Label 槽填 frame_size。
            // x86 `SUB64_R_IMM32`（inout reg + imm32）；demo `SUBI16`
            //（out rd + in rs1 + imm8）两种形态皆可。
            let sp = format_ident!("{}", frame.sp);
            let vn = crate::v12::codegen::pascal_ident(inst);
            let (reg_fids, imm_fids) = inst_reg_imm_fids(infos, inst)
                .ok_or_else(|| format!("[{inst}] missing operands"))?;
            if imm_fids.is_empty() {
                return Err(format!(
                    "[{inst}] must have an imm operand for @frame_alloc"
                ));
            }
            let alloc_neg = frame.alloc_neg;
            let frame_size_expr: TokenStream = if alloc_neg {
                quote! { -(__frame_size as i64) }
            } else {
                quote! { __frame_size as i64 }
            };
            let reg_binds: Vec<TokenStream> = reg_fids
                .iter()
                .map(|f| {
                    quote! { #f: Reg::#sp }
                })
                .collect();
            let imm_binds: Vec<TokenStream> = imm_fids
                .iter()
                .map(|f| {
                    quote! { #f: #frame_size_expr }
                })
                .collect();
            Ok(quote! {
                if __frame_size != 0 {
                    let __bytes = encode(&Inst::#vn {
                        #(#reg_binds,)*
                        #(#imm_binds,)*
                    }).map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__bytes);
                }
            })
        }
        "frame_free" => {
            let Some(abi) = &model.abi else {
                return Ok(quote! {});
            };
            let Some(frame) = &abi.frame else {
                return Ok(quote! {});
            };
            let inst = frame
                .free_inst
                .as_deref()
                .ok_or_else(|| "[abi.frame].free_inst required for @frame_free".to_string())?;
            let sp = format_ident!("{}", frame.sp);
            let vn = crate::v12::codegen::pascal_ident(inst);
            let (reg_fids, imm_fids) = inst_reg_imm_fids(infos, inst)
                .ok_or_else(|| format!("[{inst}] missing operands"))?;
            if imm_fids.is_empty() {
                return Err(format!("[{inst}] must have an imm operand for @frame_free"));
            }
            let reg_binds: Vec<TokenStream> = reg_fids
                .iter()
                .map(|f| {
                    quote! { #f: Reg::#sp }
                })
                .collect();
            let imm_binds: Vec<TokenStream> = imm_fids
                .iter()
                .map(|f| {
                    quote! { #f: __frame_size as i64 }
                })
                .collect();
            Ok(quote! {
                if __frame_size != 0 {
                    let __bytes = encode(&Inst::#vn {
                        #(#reg_binds,)*
                        #(#imm_binds,)*
                    }).map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__bytes);
                }
            })
        }
        other => Err(format!("unknown emit pseudo '@{other}'")),
    }
}

/// spill 模板 → 单条 load/store 语句。
///
/// 模板操作数按位置绑定指令操作数槽；每个 token 的绑定语义：
/// - `{N}`（任意编号，不限于 {0}/{1}——指令 3+ 操作数照常支持）：
///   - Reg 槽 → 数据寄存器（`__dst`：load 目标 / store 源）
///   - Mem 槽 → MemRef{base, __off}（x86 式基址在模板 base 键）
///   - Imm/Label 槽 → `__off as i64`（riscv 式：基址在字面操作数）
/// - 字面物理寄存器（Reg 槽）→ `Reg::#reg`（riscv `LD {0}, X8, {1}` 基址）
/// - 字面立即数（Imm/Label 槽）→ 数字字面量（若 {N} 语义用尽后仍需固定值）
fn gen_spill_stmt(
    infos: &[InstInfo],
    tpl: Option<&SpillTemplate>,
    is_load: bool,
    default_base: &str,
) -> Result<TokenStream, String> {
    let Some(t) = tpl else {
        return Ok(quote! {
            let _ = (__dst, __off);
        });
    };
    let template = if is_load { &t.load } else { &t.store };
    // 指令名 = 模板首词；其余操作数按位置绑定。
    let (inst_name, ops) = match template.split_once(char::is_whitespace) {
        Some((n, rest)) => (n.trim(), rest.trim()),
        None => (template.trim(), ""),
    };
    let info = inst_info_by_name(infos, inst_name)
        .ok_or_else(|| format!("spill 模板引用了未知指令 '{inst_name}'"))?;
    let base_name = t.base.clone().unwrap_or_else(|| default_base.to_string());
    let base = format_ident!("{base_name}");
    let mut bindings: Vec<(String, TokenStream)> = Vec::new();
    if !ops.is_empty() {
        for (i, op) in ops.split(',').map(|s| s.trim()).enumerate() {
            if op.is_empty() {
                continue;
            }
            let fid = info
                .operands
                .get(i)
                .map(|(_, fid, _, _)| fid.clone())
                .ok_or_else(|| format!("spill 模板操作数 {i} 超出指令 {inst_name}"))?;
            let slot = &info.operands[i].2;
            // `{N}` 编号操作数（任意 N）：按槽类型绑定语义
            let is_numbered = op
                .strip_prefix('{')
                .and_then(|r| r.strip_suffix('}'))
                .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()));
            let expr: TokenStream = if is_numbered {
                match slot.kind {
                    OperandKind::Reg => field_ctor_expr(slot, quote! { __dst }),
                    OperandKind::Mem => {
                        quote! { MemRef { base: Reg::#base, disp: __off, index: None, scale: 1 } }
                    }
                    OperandKind::Imm | OperandKind::Label => {
                        quote! { __off as i64 }
                    }
                    _ => {
                        return Err(format!(
                            "spill 模板操作数 '{op}'（指令 {inst_name} 槽 {}）不支持编号绑定",
                            slot.kind.kind_name()
                        ));
                    }
                }
            } else if slot.kind == OperandKind::Reg {
                // 字面物理寄存器名 = 基址（riscv `LD {0}, X8, {1}`）
                let reg = format_ident!("{op}");
                quote! { Reg::#reg }
            } else if slot.kind == OperandKind::Imm || slot.kind == OperandKind::Label {
                // 字面立即数（固定偏移/掩码等）
                let v: i64 = super::integration::parse_i64_lit(op)
                    .map_err(|_| format!("spill 模板立即数 '{op}' 无法解析（指令 {inst_name}）"))?;
                quote! { #v }
            } else {
                return Err(format!(
                    "spill 模板操作数 '{op}' 不支持（指令 {inst_name} 槽 {}）",
                    slot.kind.kind_name()
                ));
            };
            bindings.push((fid.to_string(), expr));
        }
    }
    let fields: Vec<TokenStream> = info
        .operands
        .iter()
        .map(|(_, fid, slot, _)| {
            let fid_ident = format_ident!("{fid}");
            let expr = bindings
                .iter()
                .find(|(f, _)| f == &fid.to_string())
                .map(|(_, e)| e.clone())
                .unwrap_or_else(|| field_ctor_expr(slot, quote! { 0u32 }));
            quote! { #fid_ident: #expr }
        })
        .collect();
    let vn = crate::v12::codegen::pascal_ident(inst_name);
    let ctor = if info.operands.is_empty() {
        quote! { Inst::#vn }
    } else {
        quote! { Inst::#vn { #(#fields),* } }
    };
    Ok(quote! {
        let __bytes = encode(&#ctor).map_err(|e| crate::IrError::Emit(e))?;
        __sink.put_bytes(&__bytes);
    })
}
