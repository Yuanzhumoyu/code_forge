//! TargetABI / TargetFrameLowering / emit 伪指令（@frame_alloc 等）生成。
//!
//! 从 `integration.rs` 拆分（原 1258-2069 行）。复用父模块的 `InstInfo`、
//! `pascal_ident`（codegen）与 integration 工具（inst_fids/inst_move_role/
//! inst_reg_imm_fids）以及 model 类型。

use super::super::model::*;
use super::super::shared::parse_u64;
use super::integration::{inst_fids, inst_move_role, inst_reg_imm_fids};
use super::{field_ctor_expr, pascal_ident, InstInfo};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

// ─────────────────────── TargetABI ───────────────────────

/// 入口：`pub(crate)` 由 `integration::gen_integration()` 调用。
pub(crate) fn gen_abi(model: &V12Model) -> Result<TokenStream, String> {
    // [abi] → arg_regs（按 arg_class 顺序：int 类在前，其余 class 依次）。
    // ret_regs：缺省空（v12 声明层暂不区分返回寄存器——后续迭代扩展）。
    let stack_align = model.abi.as_ref().and_then(|a| a.stack_align).unwrap_or(16);
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
                        ac.class
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
            fn vector_by_ref_limit(&self) -> Option<u32> { #by_ref_toks }
            fn min_frame_bytes(&self) -> u32 { #min_frame }
            fn callee_saved_bytes_override(&self) -> Option<u32> { #csb_toks }
            fn stack_slot_shift(&self) -> Option<i32> { #sss_toks }
        }
    })
}

// ─────────────────────── TargetFrameLowering ───────────────────────

/// 入口：`pub(crate)` 由 `integration::gen_integration()` 调用。
pub(crate) fn gen_frame_lowering(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
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

    // 尾声跳转指令选择：x86 JMP_REL32（0xE9 rel32）手写；定宽（riscv）
    // JAL x0, epilogue_label（label 槽 = 块号 → encoder 定宽 fixup
    // Relative(4,0)，位段由 RiscvRelocPatcher 编码）。
    let has_jmp_f = infos.iter().any(|i| i.inst.name == "JMP_REL32");
    let has_jal_f = infos.iter().any(|i| i.inst.name == "JAL");
    let jal_f = inst_fids(infos, "JAL");
    let (jal_dest, jal_target) = if jal_f.len() >= 2 {
        (jal_f[0].clone(), jal_f[1].clone())
    } else {
        (format_ident!("dest"), format_ident!("target"))
    };
    let epilogue_jump_body: TokenStream = if has_jmp_f {
        quote! {
            // JMP rel32（0xE9 + 占位）+ label 记录（链接期回填）。
            sink.put1(0xE9);
            let fixup = sink.offset();
            sink.put4(0u32);
            sink.use_label_at(fixup, epilogue_block, crate::RelocKind::REL4);
            Ok(())
        }
    } else if has_jal_f {
        quote! {
            // jal x0, epilogue_block——label 槽 = 块号 → encoder 编码时
            // use_label_at（定宽 fixup = 指令起始；位段重排由 patcher）。
            let inst = Inst::Jal {
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
                "epilogue jump: need JMP_REL32 or JAL".into(),
            ))
        }
    };

    // spill load/store：`{0}` = 寄存器、`{1}` = 帧偏移、基址来自模板 base。
    let spill_gpr = model.spill.get("GPR");
    let spill_fpr = model.spill.get("FPR");
    let gpr_load = gen_spill_stmt(infos, spill_gpr, true)?;
    let gpr_store = gen_spill_stmt(infos, spill_gpr, false)?;
    let fpr_load = gen_spill_stmt(infos, spill_fpr, true)?;
    let fpr_store = gen_spill_stmt(infos, spill_fpr, false)?;

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
            if inst_fids(infos, "PUSH").is_empty() && inst_fids(infos, "POP").is_empty() {
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
            let push_pop = if name == "push_callee" { "PUSH" } else { "POP" };
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
                .find(|ac| ac.class == "int")
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
                .find(|ac| ac.class == "float")
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
            // 浮点参数移动：MOVSD/MOVSS（fpr out, fpr in；reg=dest、rm=src）
            let fpr_fids = inst_fids(infos, "MOVSD");
            let has_fpr_mov = fpr_fids.len() >= 2 && inst_fids(infos, "MOVSS").len() >= 2;
            let (f_dest, f_src) = if fpr_fids.len() >= 2 {
                (fpr_fids[0].clone(), fpr_fids[1].clone())
            } else {
                (format_ident!("dest"), format_ident!("src"))
            };
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
                            Inst::Movss {
                                #f_dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                                #f_src: __src,
                            }
                        } else {
                            Inst::Movsd {
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
            Ok(quote! {
                let mut __gi = 0usize;
                let mut __fi = 0usize;
                for (__i, &__pv) in __rm.param_vregs.iter().enumerate() {
                    if !__rm.assignments.contains_key(&__pv) {
                        continue;
                    }
                    let __dest = match __rm.preg(__pv) {
                        Some(p) => p.num,
                        None => continue,
                    };
                    if __rm.param_by_ref.get(__i) == Some(&true) {
                        // by-ref：宽向量参数按引用传——GPR 槽位是数据指针，
                        // 从 [ptr] load 到向量寄存器（VMOVAPS_RM/ZMM_MEM）。
                        if __gi < #n {
                            let __src = [#(Reg::#regs),*][__gi];
                            __gi += 1;
                            #byref_stmt
                        }
                    } else if __rm.param_is_float.get(__i) == Some(&true) {
                        #fpr_stmt
                    } else if __gi < #n {
                        let __src = [#(Reg::#regs),*][__gi];
                        __gi += 1;
                        let __bytes = encode(&Inst::#mov_vn {
                            #m_src: __src,
                            #m_dest: Reg::from_index(__dest, forge_ir::RegClass::GPR64),

                        }).map_err(|e| crate::IrError::Emit(e))?;
                        __sink.put_bytes(&__bytes);
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
/// 两种形态：
/// - x86 式 2 操作数 + Mem 槽：`MOV64_RM {0}, {1}`（{0}=寄存器、{1}=MemRef
///   {base, __off}，base 来自模板 base 键）。
/// - riscv 式 3 操作数 reg+reg+imm：`LD {0}, X2, {1}`（{0}=寄存器、字面
///   物理寄存器 = 基址、{1}=__off 立即数）。
fn gen_spill_stmt(
    infos: &[InstInfo],
    tpl: Option<&SpillTemplate>,
    is_load: bool,
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
    let base_name = t.base.clone().unwrap_or_else(|| "RBP".to_string());
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
            let expr: TokenStream = match op {
                "{0}" if slot.kind == OperandKind::Reg => field_ctor_expr(slot, quote! { __dst }),
                "{1}" if slot.kind == OperandKind::Mem => {
                    quote! { MemRef { base: Reg::#base, disp: __off, index: None, scale: 1 } }
                }
                "{1}" if slot.kind == OperandKind::Imm || slot.kind == OperandKind::Label => {
                    // riscv 式：`LD {0}, X2, {1}`——基址在字面操作数、偏移在 {1}
                    quote! { __off as i64 }
                }
                other if slot.kind == OperandKind::Reg => {
                    // 字面物理寄存器名 = 基址（riscv `LD {0}, X2, {1}`）
                    let reg = format_ident!("{other}");
                    quote! { Reg::#reg }
                }
                other => {
                    return Err(format!(
                        "spill 模板操作数 '{other}' 不支持（指令 {inst_name} 槽 {}）",
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

