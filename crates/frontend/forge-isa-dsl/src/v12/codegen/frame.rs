//! TargetABI / TargetFrameLowering / emit 伪指令（@frame_alloc 等）生成。
//!
//! 从 `integration.rs` 拆分（原 1258-2069 行）。复用父模块的 `InstInfo`、
//! `pascal_ident`（codegen）与 integration 工具（inst_fids/inst_move_role/
//! inst_reg_imm_fids）以及 model 类型。

use super::super::model::*;
use super::integration::{inst_exists, inst_fids, inst_move_role, inst_reg_imm_fids};
use super::lowering::{inst_by_role_for, reg_mem_fids, role_name, role_name_for};
use super::{InstInfo, field_ctor_expr, pascal_ident};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

// ─────────────────────── TargetABI ───────────────────────

/// 入口：`pub(crate)` 由 `integration::gen_integration()` 调用。
pub(crate) fn gen_abi(model: &V12Model) -> Result<TokenStream, String> {
    // [abi] → arg_regs（按 arg_class 顺序：int 类在前，其余 class 依次）。
    // ret_regs：缺省空（v12 声明层暂不区分返回寄存器——后续迭代扩展）。
    // 栈对齐：`[stack].align` > `[stack].slot`（x86 = 16/8 不变；1 字节寄存器
    // ISA 缺省即 1，不再回退 x86 的 16）。
    let stack_align = model.stack_align()?;
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
    if let Some(abi) = &model.abi {
        for ac in &abi.arg_class {
            for r in &ac.regs {
                let i = format_ident!("{r}");
                arg_regs.push(quote! { Reg::#i });
            }
        }
    }
    // by-ref 策略：`strategy = "by-ref"` + `limit`（位）→ 超过该位宽的向量按引用
    // 传参（阈值字节 = limit/8；x86 声明 128 位 → 16 字节）。解析与校验统一在
    // `V12Model::vector_by_ref_limit_bytes`（生成期 Err：非 8 的倍数）。
    let by_ref_limit = model.vector_by_ref_limit_bytes()?;
    let by_ref_toks: TokenStream = match by_ref_limit {
        Some(b) => quote! { Some(#b as u32) },
        None => quote! { None },
    };
    // 声明式帧布局：[abi.frame].layout（fp-inside/fp-outside）+ fp_push_bytes。
    // min_frame_bytes / callee_saved_bytes / stack_slot_shift 不再在 TOML 声明
    // ——由运行期 frame_layout_info() 从这两项 + reg_info 推导（见 pipeline/
    // frame_layout.rs）。这里只把两个正交事实落进生成的 ABI。
    let layout = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .map(|f| f.layout)
        .unwrap_or_default();
    let layout_kind_toks: TokenStream = match layout {
        crate::v12::model::LayoutMode::FpInside => {
            quote! { crate::machine::abi::FrameLayoutKind::Inside }
        }
        crate::v12::model::LayoutMode::FpOutside => {
            quote! { crate::machine::abi::FrameLayoutKind::Outside }
        }
    };
    // fp_push_bytes：显式键 > 地址类宽度（元数据驱动；x86 = 8、riscv/arm64 = 16、
    // demo = 0 均由各自 TOML 显式声明——历史 `unwrap_or(8)` 对 1 字节寄存器 ISA
    // 是错的 8）。
    let fp_push = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|f| f.fp_push_bytes)
        .unwrap_or(model.addr_class()?.width() as u32);
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
            fn frame_layout(&self) -> crate::machine::abi::FrameLayout {
                crate::machine::abi::FrameLayout {
                    kind: #layout_kind_toks,
                    fp_push_bytes: #fp_push,
                }
            }
            fn int_arg_slot_count(&self) -> usize { #int_arg_slot_count }
        }
    })
}

// ─────────────────────── TargetFrameLowering ───────────────────────

/// 跳转指令 dest 槽的寄存器类（元数据：槽声明的 class；多类/未声明 → 主 GPR
/// 类常量 `__DEFAULT_GPR_CLASS`）。历史实现写死 `GPR(64)`——非 x86 ISA 的
/// 零寄存器（riscv x0）宽度不同 ⇒ 会构造出该 ISA 不存在的类。
fn jump_dest_class(
    model: &V12Model,
    infos: &[InstInfo],
    inst_name: &str,
    fid: &syn::Ident,
) -> TokenStream {
    let slot = infos
        .iter()
        .find(|i| i.inst.name == inst_name)
        .and_then(|i| i.operands.iter().find(|(_, f, _, _)| f == fid))
        .map(|(_, _, slot, _)| *slot);
    match slot.and_then(|s| s.class.as_ref()) {
        Some(c) => quote! { #c },
        None => {
            let _ = model;
            quote! { __DEFAULT_GPR_CLASS }
        }
    }
}

/// 入口：`pub(crate)` 由 `integration::gen_integration()` 调用。
pub(crate) fn gen_frame_lowering(
    infos: &[InstInfo],
    model: &V12Model,
) -> Result<TokenStream, String> {
    // 序/尾声：由生成器按机器事实 + 角色生成（谱里不再有模板/伪指令）。
    let prologue_toks = gen_frame_sequence(infos, model, true)?;
    let epilogue_toks = gen_frame_sequence(infos, model, false)?;

    // 独立尾声标签：缺省 true（x86 语义：return block 经 epilogue_jump 跳到
    // 统一尾声）。定宽 ISA 无 JMP 指令时可设 false（[emit].epilogue_label）：
    // return block 不发 jump，尾声直接顺序发在 return block 之后——仅单
    // return block 函数安全（多 return block 会 fall-through 错序）。
    let needs_epilogue_label = model
        .emit
        .as_ref()
        .and_then(|e| e.epilogue_label)
        .unwrap_or(true);

    // 尾声跳转指令：角色 `epilogue_jump`，缺省回退 `jump`（多数 ISA 两者同一条）。
    // v14 是 `[emit].epilogue_jump_inst` / `[abi].jump_inst` 两个名指针 + 按 ISA
    // 形态猜的默认名（变长 → JMP_REL32、定宽 → JAL）。
    let jump_inst = role_name(infos, Role::EpilogueJump)
        .or_else(|_| role_name(infos, Role::Jump))
        .unwrap_or_default();
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
    let epilogue_jump_body: TokenStream = if has_jump && model.is_prefix_scan() {
        quote! {
            // 变长（x86 JMP_REL32 语义）：label 槽 = 块号 → encoder 编码 +
            // use_label_at（REL4 fixup = 指令末尾 rel32 占位）。
            let inst = Inst::#jump_vn { #jmp_rel: epilogue_block.id() as i64 };
            encoder.encode(&inst, reg_map, sink).map_err(|e| {
                crate::IrError::Internal(format!("epilogue jump encode: {e}"))
            })?;
            Ok(())
        }
    } else if has_jump && jump_f.len() >= 2 {
        // 定宽（riscv JAL 语义）：jal x0, epilogue_block——label 槽 = 块号
        // → encoder 编码时 use_label_at（定宽 fixup = 指令起始；位段重排由
        // patcher）。dest 用该槽声明的类（缺省 = 主 GPR 类，元数据派生，
        // 不写死 x86 的 GPR64）。
        let jal_dest_cls = jump_dest_class(model, infos, &jump_inst, &jal_dest);
        quote! {
            let inst = Inst::#jump_vn {
                #jal_dest: Reg::from_index(0, #jal_dest_cls),
                #jal_target: epilogue_block.id() as i64,
            };
            encoder.encode(&inst, reg_map, sink).map_err(|e| {
                crate::IrError::Internal(format!("epilogue jump encode: {e}"))
            })?;
            Ok(())
        }
    } else if has_jump {
        quote! {
            // 定宽 bare 跳（arm64 B 语义：仅 label 槽，无 dest 寄存器）：
            // b epilogue_block——label 槽 = 块号 → encoder 定宽 fixup
            // Relative(4,0)，位段重排由 Arm64RelocPatcher（imm26）。
            let inst = Inst::#jump_vn { #jmp_rel: epilogue_block.id() as i64 };
            encoder.encode(&inst, reg_map, sink).map_err(|e| {
                crate::IrError::Internal(format!("epilogue jump encode: {e}"))
            })?;
            Ok(())
        }
    } else {
        quote! {
            Err(crate::IrError::Unsupported(
                "epilogue jump: 本 ISA 未声明 roles = [\"jump\"]/[\"epilogue_jump\"] 的指令".into(),
            ))
        }
    };

    // spill load/store：`{0}` = 寄存器、`{1}` = 帧偏移、基址来自模板 base。
    // 模板未声明 base 时取 [abi.frame].fp（x86 RBP / riscv X8 / arm64 X29）——
    // 正是帧指针语义。**fp 也未声明 → 生成期 Err**（历史实现回退字面量
    // `"RBP"`：非 x86 ISA 会生成一个不存在的寄存器名）。仅在"存在未声明 base
    // 的溢出模板"时才要求该键——完全没有 spill 的 ISA（如纯算术夹具）不受影响。
    let needs_default_base = model.spill.values().any(|t| t.base.is_none());
    let default_base = match model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|fr| fr.fp.clone())
    {
        Some(fp) => fp,
        None if needs_default_base => {
            return Err(
                "[spill.*]: 存在未声明 base 的溢出模板，但 [abi.frame].fp 缺失——spill 基址缺省取帧指针，不能回退 x86 的 \"RBP\"（生成期 fail-closed：请声明 [abi.frame].fp 或在每个 [spill.*] 显式写 base）"
                    .to_string(),
            );
        }
        // 所有模板都自带 base（或无 spill 模板）：该缺省值不会被使用。
        None => String::new(),
    };
    let spill_gpr = model.spill.get("GPR");
    let gpr_load = gen_spill_stmt(infos, spill_gpr, true, &default_base)?;
    let gpr_store = gen_spill_stmt(infos, spill_gpr, false, &default_base)?;
    // FPR（含向量）溢出**按值宽分派**（WA-46）：≤8 字节走缺省 `[spill.FPR]`，
    // 16/32/64 走 `[spill.FPR16/32/64]`；>8 字节缺模板 → 生成期 fail-closed。
    let fpr_load = gen_fpr_spill_dispatch(infos, model, true, &default_base)?;
    let fpr_store = gen_fpr_spill_dispatch(infos, model, false, &default_base)?;

    Ok(quote! {
        pub struct FrameLowering;

        impl crate::machine::frame::TargetFrameLowering for FrameLowering {
            type Inst = Inst;

            fn needs_epilogue_label(&self) -> bool { #needs_epilogue_label }

            fn emit_epilogue_jump(
                &self,
                encoder: &std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Self::Inst>>,
                reg_map: &crate::AllocResult,
                epilogue_block: crate::emit::LabelRef,
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

/// 按指令名查找 InstInfo（emit/spill 模板用）。
fn inst_info_by_name<'a>(infos: &'a [InstInfo<'a>], name: &str) -> Option<&'a InstInfo<'a>> {
    infos.iter().find(|i| i.inst.name == name)
}

// ───────────────────── 序/尾声的规范序列（v20 A4）─────────────────────

/// **序言 / 尾声**：由生成器按**机器事实 + 角色**生成——谱里只有裸指令。
///
/// v20 A4 删掉了 `[emit.prologue|epilogue]` 与四个伪指令（`@push_callee`/`@pop_callee`/
/// `@frame_alloc`/`@frame_free`）：**函数调用平衡**（保存谁、帧多大、怎么建立帧指针）
/// 是**调用约定**的事，由这里统一发射。顺序由被调方保存**机制**决定：
///
/// - **push 机制**（谱里声明了 `roles = ["push"]`/`["pop"]`，如 x86）：序言
///   `push fp` → `frame_set(fp←sp)` → 逐个 `push` callee-saved → **收参** → `frame_alloc`；
///   尾声 `frame_set(sp←fp)` → `sp -= callee-saved 区` → 逆序 `pop` → `pop fp` → `ret`。
/// - **store_to_frame 机制**（定宽 ISA：riscv/arm64）：序言 `frame_alloc` →
///   `callee_save(link → [sp+frame-8])` → `callee_save(fp → [sp+frame-fp_push])` →
///   `frame_set(fp←sp+frame)` → 逐个 `callee_save` → **收参**；尾声逆序 `callee_load` →
///   `callee_load(fp)` → `callee_load(link)` → `frame_free` → `ret`。
///
/// 三条不变量（守卫 `tests/call_layout_emission.rs` 钉住）：① **保存早于收参**——否则存
/// 下来的是实参值而不是调用者的寄存器值，尾声恢复会毁掉调用者的寄存器；② 帧内保存槽
/// 只在 `frame_alloc` 之后写；③ 尾声与序言**同源**（同一份列表与偏移公式，逆序恢复）。
///
/// **缺角色的步不发射**：角色是 ISA 的**能力申报**，没申报就做不了（例如没有 `frame_set`
/// 的谱不建立帧指针、没有 `callee_save` 的谱不写帧内保存槽）。缺口由
/// `forge-isa abi check`/`validate` 与运行期行为说话——这里不猜别家 ISA 的指令名。
fn gen_frame_sequence(
    infos: &[InstInfo],
    model: &V12Model,
    is_prologue: bool,
) -> Result<TokenStream, String> {
    let mut pre: Vec<TokenStream> = Vec::new();
    let mut post: Vec<TokenStream> = Vec::new();
    if let Some(frame) = model.abi.as_ref().and_then(|a| a.frame.as_ref()) {
        let push_inst = role_name(infos, Role::Push).unwrap_or_default();
        let pop_inst = role_name(infos, Role::Pop).unwrap_or_default();
        if inst_exists(infos, &push_inst) && inst_exists(infos, &pop_inst) {
            gen_push_mechanism(
                infos,
                model,
                frame,
                &push_inst,
                &pop_inst,
                is_prologue,
                &mut pre,
                &mut post,
            )?;
        } else {
            gen_store_mechanism(infos, model, frame, is_prologue, &mut pre, &mut post)?;
        }
    }
    // 收参：序言里**无条件**发射（与 `[abi.frame]` 无关——只声明参数类的谱也收得到参）。
    let recv = if is_prologue {
        gen_arg_receive(infos, model)?
    } else {
        quote! {}
    };
    Ok(quote! { #(#pre)* #recv #(#post)* })
}

/// 帧分配/释放的立即数：`[abi.frame].alloc_neg` 决定符号（riscv `addi sp, sp, -N`
/// 是加法指令 + 负立即数；x86/arm64 的 `sub` 自带减号语义）。
fn frame_size_imm(frame: &AbiFrame) -> TokenStream {
    if frame.alloc_neg {
        quote! { -(__frame_size as i64) }
    } else {
        quote! { __frame_size as i64 }
    }
}

/// `sp ← sp ± imm`（`frame_alloc` = 减、`frame_free` = 加）。
///
/// `guarded` = 用 `if __frame_size != 0` 包起来（帧分配/释放都是"零帧不发"）；
/// `trail` 只影响字段列表的尾逗号——为了让**生成物与手写模板时代逐字一致**
/// （伪指令发射带尾逗号、模板行发射不带），与机器码无关。
fn sp_adjust_stmt(
    infos: &[InstInfo],
    frame: &AbiFrame,
    role: Role,
    imm: &TokenStream,
    guarded: bool,
    trail: bool,
) -> Result<Option<TokenStream>, String> {
    let Ok(inst) = role_name(infos, role) else {
        return Ok(None);
    };
    let sp = format_ident!("{}", frame.sp);
    let vn = pascal_ident(&inst);
    let Some((reg_fids, imm_fids)) = inst_reg_imm_fids(infos, &inst) else {
        return Err(format!("[{inst}] missing operands"));
    };
    if imm_fids.is_empty() {
        return Err(format!("[{inst}] 作为帧调整指令需要一个 imm 槽"));
    }
    let mut binds: Vec<TokenStream> = reg_fids.iter().map(|f| quote! { #f: Reg::#sp }).collect();
    binds.extend(imm_fids.iter().map(|f| quote! { #f: #imm }));
    let ctor = if trail {
        quote! { Inst::#vn { #(#binds,)* } }
    } else {
        quote! { Inst::#vn { #(#binds),* } }
    };
    let body = quote! {
        let __bytes = encode(&#ctor).map_err(|e| crate::IrError::Emit(e))?;
        __sink.put_bytes(&__bytes);
    };
    Ok(Some(if guarded {
        quote! { if __frame_size != 0 { #body } }
    } else {
        body
    }))
}

/// `frame_set`：序言 `fp ← sp`（定宽 ISA 同时 `+ frame_size`）/ 尾声 `sp ← fp`。
fn frame_set_stmt(
    infos: &[InstInfo],
    frame: &AbiFrame,
    is_prologue: bool,
) -> Result<Option<TokenStream>, String> {
    let Some(fp_name) = frame.fp.clone() else {
        return Ok(None);
    };
    let Ok(inst) = role_name(infos, Role::FrameSet) else {
        return Ok(None);
    };
    let Some((src_fid, _si, dst_fid, _di)) = inst_move_role(infos, &inst) else {
        return Err(format!(
            "[{inst}] 作为 frame_set 需要「一个 in Reg 槽 + 一个 out/inout Reg 槽」"
        ));
    };
    let Some(info) = infos.iter().find(|i| i.inst.name == inst) else {
        return Err(format!("[{inst}] 不在指令表里"));
    };
    let vn = info.vn.clone();
    let sp = format_ident!("{}", frame.sp);
    let fp = format_ident!("{fp_name}");
    let (src_reg, dst_reg) = if is_prologue { (sp, fp) } else { (fp, sp) };
    let mut binds: Vec<TokenStream> = Vec::new();
    for (_, fid, slot, _) in info.operands.iter() {
        if *fid == src_fid {
            binds.push(quote! { #fid: Reg::#src_reg });
        } else if *fid == dst_fid {
            binds.push(quote! { #fid: Reg::#dst_reg });
        } else if matches!(slot.kind, OperandKind::Imm | OperandKind::Label) {
            let imm = if is_prologue {
                quote! { __frame_size as i64 }
            } else {
                quote! { 0i64 }
            };
            binds.push(quote! { #fid: #imm });
        }
    }
    Ok(Some(quote! {
        let __bytes = encode(&Inst::#vn { #(#binds),* }).map_err(|e| crate::IrError::Emit(e))?;
        __sink.put_bytes(&__bytes);
    }))
}

/// `callee_save` / `callee_load`：Reg 槽[0] = 值、Reg 槽[1] = 基址、Imm 槽 = 偏移。
///
/// `trail` 同 [`sp_adjust_stmt`]：只为了让生成物与手写模板时代逐字一致
/// （模板行不带尾逗号、伪指令循环带），与机器码无关。
fn callee_mem_stmt(
    infos: &[InstInfo],
    role: Role,
    value: &TokenStream,
    base: &TokenStream,
    off: &TokenStream,
    trail: bool,
) -> Result<Option<TokenStream>, String> {
    let Ok(inst) = role_name(infos, role) else {
        return Ok(None);
    };
    let Some(info) = infos.iter().find(|i| i.inst.name == inst) else {
        return Err(format!("[{inst}] 不在指令表里"));
    };
    let vn = info.vn.clone();
    let mut binds: Vec<TokenStream> = Vec::new();
    let mut reg_i = 0usize;
    for (_, fid, slot, _) in info.operands.iter() {
        match slot.kind {
            OperandKind::Reg => {
                let expr = if reg_i == 0 { value } else { base };
                reg_i += 1;
                binds.push(quote! { #fid: #expr });
            }
            OperandKind::Imm | OperandKind::Label => binds.push(quote! { #fid: #off }),
            _ => {}
        }
    }
    if reg_i < 2 {
        return Err(format!("[{inst}] 作为 {role} 需要「值 + 基址」两个 Reg 槽"));
    }
    let ctor = if trail {
        quote! { Inst::#vn { #(#binds,)* } }
    } else {
        quote! { Inst::#vn { #(#binds),* } }
    };
    Ok(Some(quote! {
        let __bytes = encode(&#ctor).map_err(|e| crate::IrError::Emit(e))?;
        __sink.put_bytes(&__bytes);
    }))
}

/// 动态 callee-saved 的保存/恢复循环：保存哪些寄存器由 regalloc 决定
/// （`__rm.callee_saved_to_save`），偏移 = `frame_size - fp_push - (k+1)*slot`；
/// 恢复是同一公式的**逆序**（`__n - 1 - k`），寄存器与槽位同源。
fn callee_saved_loop(
    infos: &[InstInfo],
    role: Role,
    is_store: bool,
    sp: &syn::Ident,
    fp_push: i64,
) -> Result<Option<TokenStream>, String> {
    let value = quote! { __reg };
    let base = quote! { Reg::#sp };
    let off = quote! { __off };
    let Some(stmt) = callee_mem_stmt(infos, role, &value, &base, &off, true)? else {
        return Ok(None);
    };
    let fp_push_lit = proc_macro2::Literal::i64_suffixed(fp_push);
    let (iter, k_expr) = if is_store {
        (
            quote! { __saved.iter().enumerate() },
            quote! { (__k as i64 + 1) * __SLOT_BYTES as i64 },
        )
    } else {
        (
            quote! { __saved.iter().rev().enumerate() },
            quote! { (__n as i64 - 1 - __k as i64 + 1) * __SLOT_BYTES as i64 },
        )
    };
    Ok(Some(quote! {
        let __saved = __rm.callee_saved_to_save.clone();
        let __n = __saved.len();
        for (__k, __preg) in #iter {
            let __reg = Reg::from_index(__preg.num, __preg.class);
            let __off_val: i64 = #k_expr;
            let __off = __frame_size as i64 - #fp_push_lit - __off_val;
            #stmt
        }
    }))
}

/// 尾声的返回指令（角色 `ret`；缺角色**明确报错**——没有返回的函数不是函数）。
fn ret_stmt(infos: &[InstInfo]) -> Result<TokenStream, String> {
    let inst = role_name(infos, Role::Ret)
        .map_err(|e| format!("尾声需要返回指令（roles = [\"ret\"]）：{e}"))?;
    let Some(info) = infos.iter().find(|i| i.inst.name == inst) else {
        return Err(format!("[{inst}] 不在指令表里"));
    };
    if !info.operands.is_empty() {
        return Err(format!("[{inst}] 作为 ret 不应带操作数"));
    }
    let vn = info.vn.clone();
    Ok(quote! {
        let __bytes = encode(&Inst::#vn).map_err(|e| crate::IrError::Emit(e))?;
        __sink.put_bytes(&__bytes);
    })
}

/// push 机制（x86）：`push`/`pop` 由硬件调整 sp，callee-saved 用**静态**表。
#[allow(clippy::too_many_arguments)]
fn gen_push_mechanism(
    infos: &[InstInfo],
    model: &V12Model,
    frame: &AbiFrame,
    push_inst: &str,
    pop_inst: &str,
    is_prologue: bool,
    pre: &mut Vec<TokenStream>,
    post: &mut Vec<TokenStream>,
) -> Result<(), String> {
    let slot = model.slot_bytes()? as i64;
    let callee: Vec<String> = model
        .abi
        .as_ref()
        .and_then(|a| a.callee_saved.as_ref())
        .map(|c| c.gpr.clone())
        .unwrap_or_default();
    let fp_name = frame.fp.clone().unwrap_or_default();
    let push_vn = pascal_ident(push_inst);
    let push_fid = inst_fids(infos, push_inst)
        .first()
        .cloned()
        .ok_or_else(|| format!("[{push_inst}] 作为 push 需要 1 个 Reg 槽"))?;
    let pop_vn = pascal_ident(pop_inst);
    let pop_fid = inst_fids(infos, pop_inst)
        .first()
        .cloned()
        .ok_or_else(|| format!("[{pop_inst}] 作为 pop 需要 1 个 Reg 槽"))?;
    let push_one = |name: &str| -> TokenStream {
        let r = format_ident!("{name}");
        quote! {
            let __bytes = encode(&Inst::#push_vn { #push_fid: Reg::#r }).map_err(|e| crate::IrError::Emit(e))?;
            __sink.put_bytes(&__bytes);
        }
    };
    let pop_one = |name: &str| -> TokenStream {
        let r = format_ident!("{name}");
        quote! {
            let __bytes = encode(&Inst::#pop_vn { #pop_fid: Reg::#r }).map_err(|e| crate::IrError::Emit(e))?;
            __sink.put_bytes(&__bytes);
        }
    };
    if is_prologue {
        // 先保存调用者的帧指针，再建立新帧指针，然后保存 callee-saved，
        // 最后（**收参之后**）分配帧。
        if !fp_name.is_empty() {
            pre.push(push_one(&fp_name));
        }
        if let Some(s) = frame_set_stmt(infos, frame, true)? {
            pre.push(s);
        }
        for r in &callee {
            pre.push(push_one(r));
        }
        let imm = frame_size_imm(frame);
        if let Some(s) = sp_adjust_stmt(infos, frame, Role::FrameAlloc, &imm, true, true)? {
            post.push(s);
        }
    } else {
        if let Some(s) = frame_set_stmt(infos, frame, false)? {
            pre.push(s);
        }
        // `sp -= callee-saved 区`：把 sp 从 fp 退到最后一个 push 槽（静态表长，
        // 与 push 侧的列表同源）。`trail = false` = 模板时代的字面量形态。
        let cs = proc_macro2::Literal::i64_suffixed(callee.len() as i64 * slot);
        let cs_imm = quote! { #cs };
        if let Some(s) = sp_adjust_stmt(infos, frame, Role::FrameAlloc, &cs_imm, false, false)? {
            pre.push(s);
        }
        for r in callee.iter().rev() {
            pre.push(pop_one(r));
        }
        if !fp_name.is_empty() {
            pre.push(pop_one(&fp_name));
        }
        pre.push(ret_stmt(infos)?);
    }
    Ok(())
}

/// store_to_frame 机制（riscv/arm64）：帧内保存槽 + `sp` 调整，
/// 保存哪些寄存器由 regalloc（动态）决定。
fn gen_store_mechanism(
    infos: &[InstInfo],
    model: &V12Model,
    frame: &AbiFrame,
    is_prologue: bool,
    pre: &mut Vec<TokenStream>,
    post: &mut Vec<TokenStream>,
) -> Result<(), String> {
    let _ = post; // 帧内机制没有"收参之后"的步（收参固定在最后）
    let slot = model.slot_bytes()? as i64;
    let fp_push = frame.fp_push_bytes.unwrap_or(0) as i64;
    let sp = format_ident!("{}", frame.sp);
    let link = model.abi.as_ref().and_then(|a| a.call_ret_reg.clone());
    // 帧顶 fp 保存区：`fp_push_bytes` 个字节里装 link（低偏移）+ fp（高偏移）。
    let fp_off = {
        let lit = proc_macro2::Literal::i64_suffixed(fp_push);
        quote! { (__frame_size as i64) - #lit }
    };
    let link_off = {
        let lit = proc_macro2::Literal::i64_suffixed(slot);
        quote! { (__frame_size as i64) - #lit }
    };
    let base = quote! { Reg::#sp };
    if is_prologue {
        let imm = frame_size_imm(frame);
        if let Some(s) = sp_adjust_stmt(infos, frame, Role::FrameAlloc, &imm, true, true)? {
            pre.push(s);
        }
        if fp_push >= 2 * slot
            && let Some(l) = link.clone()
        {
            let l = format_ident!("{l}");
            let value = quote! { Reg::#l };
            if let Some(s) =
                callee_mem_stmt(infos, Role::CalleeSave, &value, &base, &link_off, false)?
            {
                pre.push(s);
            }
        }
        if fp_push >= slot
            && let Some(f) = frame.fp.clone()
        {
            let f = format_ident!("{f}");
            let value = quote! { Reg::#f };
            if let Some(s) =
                callee_mem_stmt(infos, Role::CalleeSave, &value, &base, &fp_off, false)?
            {
                pre.push(s);
            }
        }
        if let Some(s) = frame_set_stmt(infos, frame, true)? {
            pre.push(s);
        }
        if let Some(s) = callee_saved_loop(infos, Role::CalleeSave, true, &sp, fp_push)? {
            pre.push(s);
        }
    } else {
        if let Some(s) = callee_saved_loop(infos, Role::CalleeLoad, false, &sp, fp_push)? {
            pre.push(s);
        }
        // 恢复 ra / fp：**与保存同序**（低偏移在前），偏移与保存逐字相同。
        if fp_push >= 2 * slot
            && let Some(l) = link
        {
            let l = format_ident!("{l}");
            let value = quote! { Reg::#l };
            if let Some(s) =
                callee_mem_stmt(infos, Role::CalleeLoad, &value, &base, &link_off, false)?
            {
                pre.push(s);
            }
        }
        if fp_push >= slot
            && let Some(f) = frame.fp.clone()
        {
            let f = format_ident!("{f}");
            let value = quote! { Reg::#f };
            if let Some(s) =
                callee_mem_stmt(infos, Role::CalleeLoad, &value, &base, &fp_off, false)?
            {
                pre.push(s);
            }
        }
        let imm = quote! { __frame_size as i64 };
        if let Some(s) = sp_adjust_stmt(infos, frame, Role::FrameFree, &imm, true, true)? {
            pre.push(s);
        }
        pre.push(ret_stmt(infos)?);
    }
    Ok(())
}

/// **收参**（被调方入口）：把 ABI 寄存器里的实参搬进分配的物理寄存器。
///
/// v20 A3b-2b-2b 起由生成器在 callee-saved 保存之后自动发射（谱里不再写 @move_args）；
/// 来源寄存器由 forge-abi 的调用布局给出（AllocResult::call_layout），未覆盖的落点
/// 整函数退回 [abi] 路径。
fn gen_arg_receive(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    // callee-saved 区字节数（生成期常量，与 frame_layout::callee_saved_bytes
    // 一致：fp 保存槽 + callee-saved × 槽单位）——move_args 收 spilled 栈参数时
    // 计算 spill 槽地址 sp_base = -(frame) - callee_saved + stack_arg_bytes。
    // 缺省/步长全部元数据派生（x86 = 8；1 字节寄存器 ISA = 1）。
    let slot_bytes_lit = model.slot_bytes()? as i64;
    let callee_saved_bytes_lit: i64 = {
        let fp_push = model
            .abi
            .as_ref()
            .and_then(|a| a.frame.as_ref())
            .and_then(|f| f.fp_push_bytes)
            .unwrap_or(model.addr_class()?.width() as u32) as i64;
        let cs = model
            .abi
            .as_ref()
            .and_then(|a| a.callee_saved.as_ref())
            .map(|c| c.gpr.len() as i64 * slot_bytes_lit)
            .unwrap_or(0);
        fp_push + cs
    };
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
    let move_inst = role_name(infos, Role::GprMov).unwrap_or_default();
    let mov_vn = crate::v12::codegen::pascal_ident(&move_inst);
    let (m_src, _m_src_idx, m_dest, _m_dest_idx) = inst_move_role(infos, &move_inst)
        .ok_or_else(|| format!("[{move_inst}] must have In(src)/Out|InOut(dest) reg operands"))?;
    // 栈参数收参指令：**按语义角色**取（`stack_arg_load` / `stack_arg_store`，
    // 角色全 ISA 唯一、validate 保证）。缺角色 → `None`，由下面各调用点给出
    // 生成期的明确错误——**不再用 x86 指令名（Mov64Rm/Mov64Mr + mem/dest/src）
    // 兜底**（那样等于把某个 ISA 的命名约定写进通用生成器；见
    // docs/reference/isa-dsl.md「角色缺失 → 明确 Unsupported，不再静默去查一个
    // 别的 ISA 的指令名」）。
    // 生成期门控：shadow 已声明但角色缺失 → 直接报错（不产错码）。
    let stack_shadow_ref: TokenStream = match model
        .abi
        .as_ref()
        .and_then(|a| a.stack_args.as_ref())
        .and_then(|s| s.shadow_bytes)
    {
        Some(v) => quote! { Some(#v) },
        None => quote! { None },
    };
    let has_shadow = model
        .abi
        .as_ref()
        .and_then(|a| a.stack_args.as_ref())
        .and_then(|s| s.shadow_bytes)
        .is_some();
    // 栈参数内存基址 = `[abi.stack_args].callee_base`（缺省 fp → 用
    // `[abi.frame].fp` 的名字；x86 = RBP）。历史实现写死字面量 `Reg::RBP`
    // ——非 x86 ISA 一旦声明 shadow 会生成引用不存在寄存器的代码；这里改为
    // 元数据派生 + 生成期 fail-closed（fp 未声明 → 报错）。
    let callee_base_kind = model
        .abi
        .as_ref()
        .and_then(|a| a.stack_args.as_ref())
        .and_then(|s| s.callee_base.clone())
        .unwrap_or_else(|| "fp".to_string());
    let callee_base: TokenStream = match model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .map(|f| {
            if callee_base_kind == "sp" {
                f.sp.clone()
            } else {
                f.fp.clone().unwrap_or_default()
            }
        })
        .filter(|n| !n.is_empty())
    {
        Some(n) => {
            let id = format_ident!("{n}");
            quote! { Reg::#id }
        }
        None if has_shadow => {
            return Err(format!(
                "move_args: [abi.stack_args].shadow_bytes 已声明，但 [abi.frame].{callee_base_kind} 缺失——栈参数内存基址需要{}（不再回退字面量 \"RBP\"）",
                if callee_base_kind == "sp" {
                    "栈指针"
                } else {
                    "帧指针"
                }
            ));
        }
        None => quote! { Reg::from_index(0, __DEFAULT_GPR_CLASS) },
    };
    // 被调方第一个栈参数的槽偏移与步长（`[abi.stack_args]`；x86 = 2/1）。
    let first_off: u32 = model
        .abi
        .as_ref()
        .and_then(|a| a.stack_args.as_ref())
        .and_then(|s| s.first_offset_slots)
        .unwrap_or(2);
    let stride: u32 = model
        .abi
        .as_ref()
        .and_then(|a| a.stack_args.as_ref())
        .and_then(|s| s.stride_slots)
        .unwrap_or(1);
    // 发射用字面量：无后缀（`2` 而非 `2u32`）；stride == 1（紧凑布局，
    // x86/riscv/arm64 均如此）时不发射 `* 1`——这样"键归类"这类纯重构
    // 的生成代码与重构前**逐字节一致**（可用 dump 对照证明行为不变），
    // 只有声明了 stride ≠ 1 的 ISA 才多出步长因子。
    let first_off_lit = proc_macro2::Literal::u32_unsuffixed(first_off);
    let stride_factor: TokenStream = if stride == 1 {
        quote! {}
    } else {
        let lit = proc_macro2::Literal::u32_unsuffixed(stride);
        quote! { * #lit }
    };
    // (变体名, Mem 字段, Reg 字段)——load 的 Reg 槽是 dest、store 是 src，
    // 字段名由 `reg_mem_fids` 从操作数结构派生。
    let tagged = |role: Role,
                  what: &str|
     -> Result<Option<(syn::Ident, syn::Ident, syn::Ident)>, String> {
        let Some(info) = crate::v12::codegen::lowering::inst_by_role(infos, role) else {
            if has_shadow {
                return Err(format!(
                    "move_args: [abi.stack_args].shadow_bytes 已声明，但本 ISA 缺 roles = [\"{role}\"] 的指令（不按指令名兜底）"
                ));
            }
            return Ok(None);
        };
        let (reg, mem, _idx) = crate::v12::codegen::lowering::reg_mem_fids(info);
        match (reg, mem) {
            (Some(r), Some(m)) => Ok(Some((info.vn.clone(), m, r))),
            _ => Err(format!(
                "move_args: roles = [\"{role}\"] 的{what}必须是 Reg+Mem 形状"
            )),
        }
    };
    let load_triple = tagged(Role::StackArgLoad, "收参指令")?;
    let store_triple = tagged(Role::StackArgStore, "写回指令")?;
    // 栈参数收参语句（by-position 的 int 臂用）：有角色 → 用角色命中的指令
    // 从 [rbp+off] load 到 __dest；无角色 → **生成期**就给明确错误
    //（不引用任何指令名，也不再让 by-position 臂去插值假名字）。
    let stack_int_recv: TokenStream = match (&load_triple, has_shadow) {
        (Some((vn, mem, reg)), true) => quote! {
            let __off = (#first_off_lit * __SLOT_BYTES as i64 + __shadow as i64) + (__pos - #n) as i64 #stride_factor * __SLOT_BYTES as i64;
            let __bytes = encode(&Inst::#vn {
                #mem: MemRef {
                    base: #callee_base,
                    disp: __off,
                    index: None,
                    scale: 1,
                },
                #reg: Reg::from_index(__dest, __DEFAULT_GPR_CLASS),
            }).map_err(|e| crate::IrError::Emit(e))?;
            __sink.put_bytes(&__bytes);
        },
        _ => quote! {
            return Err(crate::IrError::Emit(
                "v12 move_args: 本 ISA 不支持栈参数（未声明 [abi.stack_args] / roles = [\"stack_arg_load\"] 的指令）".into(),
            ));
        },
    };
    // 栈参数收参的 scratch 寄存器（`[abi].scratch` 首项）与 callee-saved 区
    // 字节数（sp_base 计算常量）。**只在真的要走栈参数收参时**要求声明
    //（无栈参数的 ISA 不需要 scratch）；缺声明 = 生成期明确报错——
    // 不回退到某个 ISA 的寄存器名（那等于把别家的命名约定写进通用生成器）。
    let scratch0 = match abi.scratch.first() {
        Some(s) => format_ident!("{s}"),
        None if has_shadow => {
            return Err("move_args: 本 ISA 声明了 [abi.stack_args].shadow_bytes，但 [abi].scratch 未声明——栈参数收参需要一个临时寄存器（不按某个 ISA 的寄存器名兜底）"
                        .into());
        }
        None => format_ident!("__unused_scratch"),
    };
    let cs_bytes = callee_saved_bytes_lit;
    // 浮点参数移动：`[abi].fpr_mov_inst`/`fpr_mov_inst32` 键
    //（缺省 "MOVSD"/"MOVSS"；fpr out, fpr in）。
    let fpr_mov64 = role_name_for(infos, Role::FprMov, 64).unwrap_or_default();
    let fpr_mov32 = role_name_for(infos, Role::FprMov, 32).unwrap_or_default();
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
    // 按值向量（≤16 字节，VEC(16)——V64/V128）收参用**全宽 XMM
    // 寄存器移动**（128 位；缺省 "MOVAPS"——MOVSD/MOVSS 只移动
    // 8/4 字节，高半被静默截断/依赖寄存器遗留值）。与标量浮点
    //（fpr_mov_inst*）区分：参数类 VEC(16) → 全宽、FPR(8) →
    // 标量。指令缺失的 ISA（riscv）→ 运行时 Unsupported（不引用
    // 不存在的变体）。
    let vec_mov = role_name(infos, Role::VecMov).unwrap_or_default();
    let vec_fids = inst_fids(infos, &vec_mov);
    let has_vec_mov = vec_fids.len() >= 2;
    let (v_dest, v_src) = if vec_fids.len() >= 2 {
        (vec_fids[0].clone(), vec_fids[1].clone())
    } else {
        (format_ident!("dest"), format_ident!("src"))
    };
    let vec_mov_vn = crate::v12::codegen::pascal_ident(&vec_mov);
    // 全宽向量收参语句（VEC(16) 参数分支）：movaps XMM_{dest}, XMM{slot}
    let vec16_stmt: TokenStream = if has_vec_mov {
        quote! {
            let __bytes = encode(&Inst::#vec_mov_vn {
                #v_dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                #v_src: __src,
            }).map_err(|e| crate::IrError::Emit(e))?;
            __sink.put_bytes(&__bytes);
        }
    } else {
        quote! {
            return Err(crate::IrError::Emit(
                "v12 vector arg receive: 未声明 roles = [\"vec_mov\"] 的指令".into(),
            ));
        }
    };
    // by-value 向量阈值 = `[abi.arg_class].limit`（by-ref 策略，字节；
    // x86 = 16B）——元数据驱动，取代写死的 `class == VEC(16)` 判定
    // （1 字节/非常规宽度 ISA 的向量类不是 VEC(16)）。
    let fpr_pool_w = model.value_fpr_class()?.map_or(16, |c| c.width());
    let vec_by_val_max = model.vector_by_ref_limit_bytes()?.unwrap_or(fpr_pool_w);
    let vec16_cond = quote! {
        __rm.param_vregs
            .get(__i)
            .map(|x| x.width() <= #vec_by_val_max)
            .unwrap_or(false)
    };
    // by-ref 向量 load：宽向量参数（>16 字节）按引用传参——ABI 传 GPR
    // 指针（int 槽位），收参时从 [ptr] load 到目标向量寄存器。
    // **按语义角色取指令**（`roles = ["wide_vec_load_32"]` /
    // `["wide_vec_load_64"]`——角色全 ISA 唯一，validate 保证）：不做
    // 「按指令名搜索 + 非对齐优先/对齐兜底」的隐式回退——ISA 把角色打在
    // 自己选定的那条指令上即可（x86 打在非对齐 VMOVUPS_RM /
    // VMOVUPS_ZMM_MEM；想用对齐变体的 ISA 就把角色打在那条上），生成器
    // 只认角色、不猜名字；字段名同样由操作数结构派生（reg_mem_fids）。
    // **宽度按参数 IR 类型字节数分派**（`__rm.param_bytes`，与
    // param_vregs 对齐）：寄存器类宽对 >128 位向量恒为 VEC(32)
    //（`reg_class_for`），旧实现用 `__pv.width()` → 64B（V512）分支
    // 永不可达、只 load 32B（lane8..15 丢失，WA-37 D5）。
    // 32 字节（V256）→ wide_vec_load_32；64 字节（V512）→
    // wide_vec_load_64；其它 >32B 宽度无对应角色 → 显式 Unsupported
    //（不静默截断）。
    let mut byref_32: Option<TokenStream> = None;
    let mut byref_64: Option<TokenStream> = None;
    // 宽度是**字节**（32 = V256、64 = V512），角色声明里写的是**位**（256/512）。
    for width in [32u16, 64u16] {
        let Some(info) = inst_by_role_for(infos, Role::WideVecLoad, width * 8) else {
            continue; // 本 ISA 未声明该宽度的角色 → 该宽度不可用（下方给 Unsupported）
        };
        let (d_fid, m_fid, _) = reg_mem_fids(info);
        let (Some(d_fid), Some(m_fid)) = (d_fid, m_fid) else {
            continue; // 非 Reg+Mem 形状（validate 层应已拒绝）
        };
        let vn = info.vn.clone();
        let stmt: TokenStream = quote! {
            let __bytes = encode(&Inst::#vn {
                #d_fid: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                #m_fid: MemRef {
                    base: Reg::from_index(__src.to_index(), __ADDR_CLASS),
                    disp: 0,
                    index: None,
                    scale: 1,
                },
            }).map_err(|e| crate::IrError::Emit(e))?;
            __sink.put_bytes(&__bytes);
        };
        if width == 32 {
            byref_32 = Some(stmt);
        } else {
            byref_64 = Some(stmt);
        }
    }
    let byref_stmt: TokenStream = match (byref_32, byref_64) {
        (Some(s32), Some(s64)) => quote! {
            let __vbytes = __rm.param_bytes.get(__i).copied().unwrap_or(0);
            if __vbytes == 64 {
                #s64
            } else if __vbytes <= 32 {
                #s32
            } else {
                return Err(crate::IrError::Emit(
                    "v12 by-ref vector arg: unsupported width (仅支持 32B/64B 向量；其它 >32B 宽度无 load 变体)".into(),
                ));
            }
        },
        (Some(s32), None) => s32,
        (None, Some(s64)) => s64,
        (None, None) => quote! {
            return Err(crate::IrError::Emit(
                "v12 by-ref vector arg load missing（本 ISA 未声明 roles = [\"wide_vec_load_32\"] / [\"wide_vec_load_64\"] 的指令）".into(),
            ));
        },
    };
    let fpr_stmt: TokenStream = if has_fpr_mov {
        quote! {
            if __fi < #fn_ {
                let __src = [#(Reg::#float_regs),*][__fi];
                __fi += 1;
                if #vec16_cond {
                    // ≤16B 向量（V64/V128，VEC(16) 类）按值参数：
                    // 全宽 128 位 XMM 移动（MOVAPS）——MOVSD/MOVSS
                    // 只移 8/4 字节，高半静默截断（WA-37 D3）。
                    #vec16_stmt
                } else {
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
        }
    } else {
        quote! {
            let _ = __fi;
            return Err(crate::IrError::Emit("v12 float args (MOVSD/MOVSS missing)".into()));
        }
    };
    // ABI 槽位规则：by-position（Windows x64——int/float 共享位置
    // 计数，参数 i 用 GPR{i}/XMM{i}）/ by-class（缺省，独立推进）。
    let by_position = model.abi.as_ref().and_then(|a| a.arg_slot) == Some(ArgSlot::ByPosition);
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
                    if #vec16_cond {
                        // ≤16B 向量按值参数：全宽 XMM 移动
                        #vec16_stmt
                    } else {
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
                        #m_dest: Reg::from_index(__dest, __DEFAULT_GPR_CLASS),

                    }).map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__bytes);
                } else if let Some(__shadow) = #stack_shadow_ref {
                    #stack_int_recv
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
                        #m_dest: Reg::from_index(__dest, __DEFAULT_GPR_CLASS),

                    }).map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__bytes);
                }
            },
            quote! {
                if __gi < #n {
                    let __src = [#(Reg::#regs),*][__gi];
                    __gi += 1;
                    #byref_stmt
                } else {
                    // WA-37 D4：by-class ABI（riscv 等）下 by-ref 宽向量参数
                    // 超出 int 槽位上限时**显式拒绝**——旧实现无 else，会静默
                    // 不收参（值垃圾）。与 by-position 分支同款 fail-closed。
                    return Err(crate::IrError::Unsupported(
                        "v12 move_args: by-ref 宽向量参数超出 int 槽位上限（by-class ABI）".into(),
                    ));
                }
            },
        )
    };
    let _ = byref_stmt_use;
    // shadow 未声明（riscv/demo 无栈参数）→ 栈参数收参分支整体不生成
    //（否则分支体引用 RBP/R10/MOV64_RM 等不存在的 Reg/Inst 变体）。
    let has_stack_arg = model
        .abi
        .as_ref()
        .and_then(|a| a.stack_args.as_ref())
        .and_then(|s| s.shadow_bytes)
        .is_some();
    let stack_arg_receive: TokenStream = match (
        has_stack_arg,
        load_triple.as_ref(),
        store_triple.as_ref(),
    ) {
        (true, Some((l_vn, l_mem, l_reg)), Some((s_vn, s_mem, s_reg))) => quote! {
            // 栈参数（位置 ≥ 寄存器数且 shadow 声明）**无条件**收参到
            // spill 槽（regalloc 强制 spill 保证有槽；即使分配了 preg
            // 也不走寄存器分支——否则与低位置参数共享寄存器时，批量
            // 收参顺序覆盖（a→r15 后 e→r15，a 值丢）→ five_args 14）。
            if let Some(__shadow) = #stack_shadow_ref
                && __pos >= #n
                && __rm.spill_slots.contains_key(&__pv)
            {
                let __off = (#first_off_lit * __SLOT_BYTES as i64 + __shadow as i64) + (__pos - #n) as i64 #stride_factor * __SLOT_BYTES as i64;
                let __sp_base = -(__frame_size as i64) - __cs_bytes + __rm.stack_arg_bytes as i64;
                let __slot_off = __rm.spill_slot(__pv).offset as i64;
                // load ABI 槽 → scratch（角色 stack_arg_load 命中的指令）
                let __lbytes = encode(&Inst::#l_vn {
                    #l_mem: MemRef {
                        base: #callee_base,
                        disp: __off,
                        index: None,
                        scale: 1,
                    },
                    #l_reg: __scratch0,
                }).map_err(|e| crate::IrError::Emit(e))?;
                __sink.put_bytes(&__lbytes);
                // store scratch → spill 槽（角色 stack_arg_store 命中的指令）
                let __sbytes = encode(&Inst::#s_vn {
                    #s_mem: MemRef {
                        base: #callee_base,
                        disp: __sp_base + __slot_off,
                        index: None,
                        scale: 1,
                    },
                    #s_reg: __scratch0,
                }).map_err(|e| crate::IrError::Emit(e))?;
                __sink.put_bytes(&__sbytes);
                continue;
            }
        },
        // shadow 未声明（riscv/demo 无栈参数）→ 分支整体不生成
        //（否则分支体引用不存在的 Reg/Inst 变体）。
        _ => quote! {},
    };
    // spilled 的寄存器参数（位置 < n）收参到 spill 槽：需要 MOV 指令
    //（`roles = ["gpr_mov"]`，**按角色查**——不再按 x86 指令名
    // `MOV_RM8_R64` 探测）+ 角色 stack_arg_store（reg→mem）。
    let has_mov_inst = !move_inst.is_empty();
    let spilled_int_receive: TokenStream = match (
        has_stack_arg && has_mov_inst,
        store_triple.as_ref(),
    ) {
        (true, Some((s_vn, s_mem, s_reg))) => quote! {
            // 位置 < n 的 spilled 寄存器参数：load ABI 寄存器 → scratch
            // → spill 槽（mod.rs 210 写槽依赖 entry vreg 值正确）
            if __pos < #n
                && !__rm.param_is_float.get(__i).copied().unwrap_or(false)
                && __rm.spill_slots.contains_key(&__pv)
            {
                let __src = [#(Reg::#regs),*][__pos];
                let __sp_base = -(__frame_size as i64) - __cs_bytes + __rm.stack_arg_bytes as i64;
                let __slot_off = __rm.spill_slot(__pv).offset as i64;
                // ABI 寄存器 → scratch（用参数移动指令 mov_vn）
                let __lbytes = encode(&Inst::#mov_vn {
                    #m_src: __src,
                    #m_dest: __scratch0,
                }).map_err(|e| crate::IrError::Emit(e))?;
                __sink.put_bytes(&__lbytes);
                // scratch → spill 槽（角色 stack_arg_store 命中的指令）
                let __sbytes = encode(&Inst::#s_vn {
                    #s_mem: MemRef {
                        base: #callee_base,
                        disp: __sp_base + __slot_off,
                        index: None,
                        scale: 1,
                    },
                    #s_reg: __scratch0,
                }).map_err(|e| crate::IrError::Emit(e))?;
                __sink.put_bytes(&__sbytes);
            }
        },
        _ => quote! {},
    };
    let stack_arg_prologue: TokenStream = if has_stack_arg {
        quote! {
            let __scratch0 = Reg::#scratch0;
            let __cs_bytes = #cs_bytes;
        }
    } else {
        quote! {}
    };
    // ── 布局驱动的收参（v20 A3b-2b-2a）──
    // 有 `call_layout`（A3b-2b-1 由管线塞进 `AllocResult`）且**每个参数**都落在
    // 本片支持的落点（单寄存器 / 间接指针）时，收参**按布局来**：`sret` 占不占
    // 首槽、按位置还是按类计数，都是绑定/规则算出来的，生成器不再自己数
    // "第几个 int 槽"。有一个参数落在本片未覆盖的落点（`Pair`/`Group`/`Stack`/
    // 无指针的 `Indirect`）→ **整函数**退回既有 `[abi]` 路径：两条路径不混用
    //（混用会让旧路径的 `__gi`/`__fi` 游标与实际参数错位），这也让"逐字节不变"
    // 这条验收可判。
    let vec_mov_body: TokenStream = if has_vec_mov {
        quote! {
            let __bytes = encode(&Inst::#vec_mov_vn {
                #v_dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                #v_src: Reg::from_index(*index, *class),
            })
            .map_err(|e| crate::IrError::Emit(e))?;
            __sink.put_bytes(&__bytes);
        }
    } else {
        quote! {}
    };
    let fpr_by_size_body: TokenStream = if has_fpr_mov {
        quote! {
            let __src = Reg::from_index(*index, *class);
            let __bytes = encode(&if __a.size == 4 {
                Inst::#fpr_mov32_vn {
                    #f_dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                    #f_src: __src,
                }
            } else {
                Inst::#fpr_mov64_vn {
                    #f_dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                    #f_src: __src,
                }
            })
            .map_err(|e| crate::IrError::Emit(e))?;
            __sink.put_bytes(&__bytes);
        }
    } else {
        quote! {}
    };
    // 浮点/向量类落点：按**类宽**分派（与既有路径同一判据——宽类（≤16B 向量、
    // scalars 落在同一 FPR 池）走全宽 `vec_mov`，否则按参数字节宽走 `fpr_mov`）。
    let fp_from_class: TokenStream = if has_vec_mov && has_fpr_mov {
        quote! {
            if class.width() <= #vec_by_val_max {
                #vec_mov_body
            } else {
                #fpr_by_size_body
            }
        }
    } else if has_vec_mov {
        vec_mov_body
    } else if has_fpr_mov {
        fpr_by_size_body
    } else {
        quote! {
            return Err(crate::IrError::Emit(
                "v12 float args (MOVSD/MOVSS missing)".into(),
            ));
        }
    };
    Ok(quote! {
        #head
        // 栈参数收参的 scratch（[abi].scratch 首项）与 callee-saved
        // 字节数（sp_base 计算）——仅 shadow 声明时使用
        #stack_arg_prologue
        // 布局路径的入场判定：**全部**参数都落在受支持的落点才启用
        let __layout_ok = match __rm.call_layout.as_ref() {
            Some(__cl) => __rm.param_vregs.iter().enumerate().all(|(__li, _)| {
                matches!(
                    __cl.arg(__li as u32).map(|__a| &__a.place),
                    Some(crate::machine::call_layout::ArgPlace::Reg { .. })
                        | Some(crate::machine::call_layout::ArgPlace::Indirect {
                            reg: Some(_),
                            ..
                        })
                )
            }),
            None => false,
        };
        for (__i, &__pv) in __rm.param_vregs.iter().enumerate() {
            // 位置（sret 时首槽被隐藏指针占用）
            let __pos = __i + if __rm.sret { 1usize } else { 0usize };
            #stack_arg_receive
            if !__rm.assignments.contains_key(&__pv) {
                // spilled 的寄存器参数（位置 < n，int 类）：move_args
                // 必须把 ABI 寄存器值写入 spill 槽（否则 mod.rs 210 写
                // 槽读垃圾 → l1/l2.size() 值错）。与栈参数中转同构：
                // load ABI 寄存器 → scratch → store spill 槽。
                #spilled_int_receive
                continue;
            }
            let __dest = match __rm.preg(__pv) {
                Some(p) => p.num,
                None => {
                    continue;
                }
            };
            // ── 布局路径：来源寄存器由布局给出（类 + 类内号）──
            if __layout_ok
                && let Some(__cl) = __rm.call_layout.as_ref()
                && let Some(__a) = __cl.arg(__i as u32)
            {
                match &__a.place {
                    crate::machine::call_layout::ArgPlace::Reg { class, index, .. } => {
                        if class.is_int() {
                            let __src = Reg::from_index(*index, *class);
                            let __bytes = encode(&Inst::#mov_vn {
                                #m_src: __src,
                                #m_dest: Reg::from_index(__dest, __DEFAULT_GPR_CLASS),
                            })
                            .map_err(|e| crate::IrError::Emit(e))?;
                            __sink.put_bytes(&__bytes);
                        } else if class.is_fp() {
                            #fp_from_class
                        } else {
                            // 既不是整数类也不是浮点/向量类（如掩码寄存器类）——
                            // 本片没有对应的搬运角色，**明确拒绝**而不是当浮点搬。
                            return Err(crate::IrError::Unsupported(
                                "v12 move_args: 布局给的寄存器类没有收参搬运指令".into(),
                            ));
                        }
                    }
                    crate::machine::call_layout::ArgPlace::Indirect {
                        reg: Some((class, index)),
                        ..
                    } => {
                        let __src = Reg::from_index(*index, *class);
                        #byref_stmt
                    }
                    _ => {
                        return Err(crate::IrError::Internal(
                            "v12 move_args: 布局落点未过 __layout_ok 判定".into(),
                        ));
                    }
                }
                continue;
            }
            if __rm.param_by_ref.get(__i) == Some(&true) {
                // by-ref：宽向量参数按引用传——GPR 槽位是数据指针，
                // 从 [ptr] load 到向量寄存器（roles = wide_vec_load_32/64）。
                #byref_stmt_use
            } else if __rm.param_is_float.get(__i) == Some(&true) {
                #fpr_stmt_use
            } else {
                #int_stmt_use
            }
        }
    })
}

/// FPR（标量 + 向量）溢出语句的**宽度分派**（WA-46）。
///
/// 旧实现：只有一份 `[spill.FPR]`（x86 是 8 字节 MOVSD），而生成的
/// `emit_spill_load/store` **忽略 width 参数** ⇒ 任何被 spill 的向量值只搬
/// 8 字节：V128/V256/V512 的高半区既不写也不读（栈上残留）——CI 上
/// `test_jit_v512_byref_param` 偶发 `lane15 != 16` 的根因。
///
/// 现按宽度分派：`width <= scalar_max` → `[spill.FPR]`（标量缺省，
/// `scalar_max` = `[meta].value_fpr_width` / `FPR(8)`）；其余宽度档 = **已声明的
/// 模板键** `FPR<bytes>`（元数据驱动——x86 声明 FPR16/32/64；不是代码里的
/// 16/32/64 字面量）；未声明的宽度 → `Unsupported`（fail-closed，**不**退回
/// 窄搬运）。ISA 完全未声明 FPR 溢出模板时保持原 no-op（riscv/定宽试点不变）。
fn gen_fpr_spill_dispatch(
    infos: &[InstInfo],
    model: &V12Model,
    is_load: bool,
    default_base: &str,
) -> Result<TokenStream, String> {
    let Some(default_tpl) = model.spill.get("FPR") else {
        return Ok(quote! {
            let _ = (__dst, __off);
        });
    };
    let scalar = gen_spill_stmt(infos, Some(default_tpl), is_load, default_base)?;
    let what = if is_load { "load" } else { "store" };
    // 标量缺省档上限 = 浮点值**池**类宽（x86 = 8）；池未声明时退回主浮点类宽，
    // 仍无 → 0（表示"没有标量档"，全部宽度走 `[spill.FPR<bytes>]` 档位）。
    let scalar_max = match model.value_fpr_class()? {
        Some(c) => c.width(),
        None => model.main_fpr_class()?.map(|c| c.width()).unwrap_or(0),
    };
    // 宽度档：从已声明模板键 `FPR<bytes>` 派生（`m.spill` 是 BTreeMap，键序稳定）。
    let mut tiers: Vec<(String, u16)> = Vec::new();
    for key in model.spill.keys() {
        if key == "FPR" {
            continue;
        }
        let Some(num) = key.strip_prefix("FPR") else {
            continue;
        };
        if let Ok(w) = num.parse::<u16>()
            && w > scalar_max
        {
            tiers.push((key.clone(), w));
        }
    }
    tiers.sort_by_key(|(_, w)| *w);
    let mut arms: Vec<TokenStream> = Vec::new();
    for (key, w) in &tiers {
        let stmt = match model.spill.get(key) {
            Some(t) => gen_spill_stmt(infos, Some(t), is_load, default_base)?,
            // 理论到不了（档位即来自已声明键）；留作防御性 fail-closed。
            None => quote! {
                return Err(crate::IrError::Unsupported(format!(
                    "FPR spill {}: 本 ISA 未声明 [spill.{}]（{} 字节）——不退回窄搬运（会静默截断向量高半区）",
                    #what, #key, #w
                )));
            },
        };
        arms.push(quote! { else if width == #w { #stmt } });
    }
    let supported = if tiers.is_empty() {
        format!("≤{scalar_max}")
    } else {
        let mut s = format!("≤{scalar_max}");
        for (_, w) in &tiers {
            s.push('/');
            s.push_str(&w.to_string());
        }
        s
    };
    Ok(quote! {
        if width <= #scalar_max {
            #scalar
        } #(#arms)*
        else {
            return Err(crate::IrError::Unsupported(format!(
                "FPR spill {}: 不支持 {} 字节宽度（本 ISA 支持 {}）",
                #what, width, #supported
            )));
        }
    })
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
