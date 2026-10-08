//! TargetABI / TargetFrameLowering / emit 伪指令（@frame_alloc 等）生成。
//!
//! 从 `integration.rs` 拆分（原 1258-2069 行）。复用父模块的 `InstInfo`、
//! `pascal_ident`（codegen）与 integration 工具（inst_fids/inst_reg_imm_fids）
//! 以及模型类型；**搬运指令由 `super::moves::MoveTable` 从 `data_width` +
//! 操作数结构派生**（v20 V8），不再有 `gpr_mov`/`fpr_mov`/… 手写角色。

use super::super::model::*;
use super::integration::{inst_exists, inst_fids, inst_move_role, inst_reg_imm_fids};
use super::lowering::{role_name, role_name_for_class};
use super::moves::{Bank, MoveTable, Shape, Want};
use super::{InstInfo, field_ctor_expr, pascal_ident};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

// ─────────────────────── TargetABI ───────────────────────

/// 入口：`pub(crate)` 由 `integration::gen_integration()` 调用。
pub(crate) fn gen_abi(model: &IsaModel) -> Result<TokenStream, String> {
    // [abi] → arg_regs（按 arg_class 顺序：int 类在前，其余 class 依次）。
    // ret_regs：缺省空（ISA-DSL 声明层暂不区分返回寄存器——后续迭代扩展）。
    // 栈对齐：`[stack].align` > `[stack].slot`（x86 = 16/8 不变；1 字节寄存器
    // ISA 缺省即 1，不再回退 x86 的 16）。
    let stack_align = model.stack_align()?;
    // 帧填充 = **机器事实**（v20 A5-3）：`[machine].frame_padding`（x86 = 8、
    // riscv/arm64 = 0）。约定侧的 `AbiRules::frame_padding` 进 plan，由管线优先读；
    // 这里生成的 `TargetABI::frame_padding()` 是无 plan 时的回退值。
    let frame_padding = model.machine_frame_padding();
    // 位置计数规则 = **机器事实**（v20 A5-3，原 `[abi].arg_slot`）：
    // `[machine].arg_slot`（x86 = by-position、其余 = by-class）。约定侧的正式位置是
    // `AbiRules::position`（进 plan 的 `AbiPlan::position`）。
    let arg_placement_toks: TokenStream = match model.machine_arg_slot() {
        crate::dsl::model::ArgSlot::ByPosition => {
            quote! { crate::machine::abi::ArgPlacement::ByPosition }
        }
        crate::dsl::model::ArgSlot::ByClass => {
            quote! { crate::machine::abi::ArgPlacement::ByClass }
        }
    };
    // 参数/返回寄存器：v20 A5-3 起谱面不再声明（`[abi].arg_class`/`[abi].ret_regs`
    // 已删除）——落点由 plan 给，这两个 trait 方法走缺省实现（空表）。
    let arg_regs: Vec<TokenStream> = Vec::new();
    let int_arg_slot_count: usize = 0;
    // by-ref 策略：`strategy = "by-ref"` + `limit`（位）→ 超过该位宽的向量按引用
    // 传参（阈值字节 = limit/8；x86 声明 128 位 → 16 字节）。解析与校验统一在
    // `IsaModel::vector_by_ref_limit_bytes`（生成期 Err：非 8 的倍数）。
    let by_ref_limit = model.vector_by_ref_limit_bytes()?;
    let by_ref_toks: TokenStream = match by_ref_limit {
        Some(b) => quote! { Some(#b as u32) },
        None => quote! { None },
    };
    // 声明式帧布局：[machine.frame].layout（fp-inside/fp-outside）+ fp_push_bytes。
    // min_frame_bytes / callee_saved_bytes / stack_slot_shift 不再在 TOML 声明
    // ——由运行期 frame_layout_info() 从这两项 + reg_info 推导（见 pipeline/
    // frame_layout.rs）。这里只把两个正交事实落进生成的 ABI。
    let layout = model.machine_frame().map(|f| f.layout).unwrap_or_default();
    let layout_kind_toks: TokenStream = match layout {
        crate::dsl::model::LayoutMode::FpInside => {
            quote! { crate::machine::abi::FrameLayoutKind::Inside }
        }
        crate::dsl::model::LayoutMode::FpOutside => {
            quote! { crate::machine::abi::FrameLayoutKind::Outside }
        }
    };
    // fp_push_bytes：显式键 > 地址类宽度（元数据驱动；x86 = 8、riscv/arm64 = 16、
    // demo = 0 均由各自 TOML 显式声明——历史 `unwrap_or(8)` 对 1 字节寄存器 ISA
    // 是错的 8）。
    let fp_push = model
        .machine_frame()
        .and_then(|f| f.fp_push_bytes)
        .unwrap_or(model.addr_class()?.bytes() as u32);
    // 返回寄存器：v20 A5-3 起谱面不再声明（`[abi].ret_regs` 已删除）——生成物沿用
    // trait 的缺省空表（返回槽由 plan / `LowerCtx::conv_ret_gpr` 给）。
    let ret_regs: Vec<TokenStream> = Vec::new();
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
            fn arg_placement(&self) -> crate::machine::abi::ArgPlacement {
                #arg_placement_toks
            }
        }
    })
}

// ─────────────────────── TargetFrameLowering ───────────────────────

/// 跳转指令 dest 槽的寄存器类（元数据：槽声明的 class；多类/未声明 → 主 GPR
/// 类常量 `__DEFAULT_GPR_CLASS`）。历史实现写死 `GPR(64)`——非 x86 ISA 的
/// 零寄存器（riscv x0）宽度不同 ⇒ 会构造出该 ISA 不存在的类。
fn jump_dest_class(
    model: &IsaModel,
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
    model: &IsaModel,
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
    let jump_vn = crate::dsl::codegen::pascal_ident(&jump_inst);
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
    // 模板未声明 base 时取 [machine.frame].fp（x86 RBP / riscv X8 / arm64 X29）——
    // 正是帧指针语义。**fp 也未声明 → 生成期 Err**（历史实现回退字面量
    // `"RBP"`：非 x86 ISA 会生成一个不存在的寄存器名）。仅在"存在未声明 base
    // 的溢出模板"时才要求该键——完全没有 spill 的 ISA（如纯算术夹具）不受影响。
    let needs_default_base = model.spill.values().any(|t| t.base.is_none());
    let default_base = match model.machine_frame().and_then(|fr| fr.fp.clone()) {
        Some(fp) => fp,
        None if needs_default_base => {
            return Err(
                "[spill.*]: 存在未声明 base 的溢出模板，但 [machine.frame].fp 缺失——spill 基址缺省取帧指针，不能回退 x86 的 \"RBP\"（生成期 fail-closed：请声明 [machine.frame].fp 或在每个 [spill.*] 显式写 base）"
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

/// 按**形状**拼一条栈参数访存指令（load/store 共用）：`Reg+Mem` 填 MemRef；
/// `值Reg+基址Reg+位移Imm` 把基址填成给定的基址寄存器、位移填给定的偏移表达式。
fn mem_inst_toks(
    shape: &super::lowering::StackMemShape,
    base: &TokenStream,
    disp: &TokenStream,
    reg: &TokenStream,
) -> TokenStream {
    let vn = &shape.vn;
    let r = &shape.reg;
    match &shape.flavor {
        super::lowering::StackMemFlavor::Mem(m) => quote! {
            Inst::#vn {
                #m: MemRef { base: Some(#base), disp: #disp, index: None, scale: 1 },
                #r: #reg,
            }
        },
        super::lowering::StackMemFlavor::BaseDisp { base: b, imm } => quote! {
            Inst::#vn {
                #r: #reg,
                #b: #base,
                #imm: #disp,
            }
        },
    }
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
    model: &IsaModel,
    is_prologue: bool,
) -> Result<TokenStream, String> {
    let mut pre: Vec<TokenStream> = Vec::new();
    let mut post: Vec<TokenStream> = Vec::new();
    if let Some(frame) = model.machine_frame() {
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
    // 收参：序言里**无条件**发射（与 `[machine.frame]` 无关——只声明参数类的谱也收得到参）。
    let recv = if is_prologue {
        gen_arg_receive(infos, model)?
    } else {
        quote! {}
    };
    Ok(quote! { #(#pre)* #recv #(#post)* })
}

/// 帧分配/释放的立即数：`[machine.frame].alloc_neg` 决定符号（riscv `addi sp, sp, -N`
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
/// `inst` 是**已解析好的指令名**（调用方按寄存器类解析，见
/// [`role_name_for_class`]——同一个"存到帧"能力在 GPR/FPR 上可能是不同指令）。
///
/// `trail` 同 [`sp_adjust_stmt`]：只为了让生成物与手写模板时代逐字一致
/// （模板行不带尾逗号、伪指令循环带），与机器码无关。
fn callee_mem_stmt(
    infos: &[InstInfo],
    inst: &str,
    value: &TokenStream,
    base: &TokenStream,
    off: &TokenStream,
    trail: bool,
) -> Result<Option<TokenStream>, String> {
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
        return Err(format!(
            "[{inst}] 作为 callee_save/callee_load 需要「值 + 基址」两个 Reg 槽"
        ));
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
///
/// **按寄存器类分派指令**（v20 A6）：同一个能力在 GPR 与 FPR 上要用不同指令时
/// （arm64：`STURX`/`STURD`），生成物在循环里按 `__preg.class.is_fp()` 选一条。
/// 只申报了一类（或两类解析到同一条指令）时**照旧发单条语句**——生成物与
/// 迁移前逐字一致，不引入无用分支。
fn callee_saved_loop(
    infos: &[InstInfo],
    role: Role,
    is_store: bool,
    sp: &syn::Ident,
    fp_push: i64,
) -> Result<Option<TokenStream>, String> {
    use crate::dsl::model::RoleClass;
    let gpr_inst = role_name_for_class(infos, role, RoleClass::Gpr);
    let fpr_inst = role_name_for_class(infos, role, RoleClass::Fpr);
    // 两条类解析到同一条指令（或只有一类申报）⇒ 不分派。
    let split = match (&gpr_inst, &fpr_inst) {
        (Some(g), Some(f)) if g != f => Some((g.clone(), f.clone())),
        _ => None,
    };
    let single = match &split {
        Some(_) => None,
        None => gpr_inst.clone().or_else(|| fpr_inst.clone()),
    };
    let value = quote! { __reg };
    let base = quote! { Reg::#sp };
    let off = quote! { __off };
    let (stmt_single, stmt_gpr, stmt_fpr) = match split {
        Some((g, f)) => (
            None,
            callee_mem_stmt(infos, &g, &value, &base, &off, true)?,
            callee_mem_stmt(infos, &f, &value, &base, &off, true)?,
        ),
        None => (
            match single {
                Some(i) => callee_mem_stmt(infos, &i, &value, &base, &off, true)?,
                None => None,
            },
            None,
            None,
        ),
    };
    if stmt_single.is_none() && stmt_gpr.is_none() && stmt_fpr.is_none() {
        return Ok(None);
    }
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
    // 分派形态：GPR 走一条、FPR 走另一条（缺哪类就不发那支——声明是能力申报）。
    let dispatch = match (stmt_gpr, stmt_fpr) {
        (Some(g), Some(f)) => quote! {
            if __preg.class.is_fp() {
                #f
            } else {
                #g
            }
        },
        (Some(g), None) => g,
        (None, Some(f)) => f,
        (None, None) => stmt_single.expect("上面已排除全空"),
    };
    Ok(Some(quote! {
        let __saved = __rm.callee_saved_to_save.clone();
        let __n = __saved.len();
        for (__k, __preg) in #iter {
            let __reg = Reg::from_index(__preg.num, __preg.class);
            let __off_val: i64 = #k_expr;
            let __off = __frame_size as i64 - #fp_push_lit - __rm.va_top as i64 - __off_val;
            #dispatch
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

/// push 机制（x86）：`push`/`pop` 由硬件调整 sp；保存哪些 callee-saved 由
/// `alloc_result.callee_saved_to_save`（约定数据，v20 A6）在**运行时**决定。
#[allow(clippy::too_many_arguments)]
fn gen_push_mechanism(
    infos: &[InstInfo],
    _model: &IsaModel,
    frame: &AbiFrame,
    push_inst: &str,
    pop_inst: &str,
    is_prologue: bool,
    pre: &mut Vec<TokenStream>,
    post: &mut Vec<TokenStream>,
) -> Result<(), String> {
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
        // callee-saved：**运行时按 `alloc_result.callee_saved_to_save` 循环**（v20 A6）。
        // 这份列表来自"约定"——有 plan 时是 plan 的 callee-saved（x86 显式选 sysv64 就只有
        // 5 个，不是谱里那份 win64 的 7 个），无 plan 的夹具由 compiler.rs 退回谱面表。
        // 静态发谱面列表会让"约定"与"实际推入数"脱钩，从而挡住帧字节数跟着约定走。
        // **帧已为此让位**：序言实际分配的字节数由管线补上"少推的那几个槽"
        // （见 `emission.rs` 的 `cs_skipped`），rsp 落点与静态表时代逐字节相同。
        pre.push(quote! {
            for __preg in __rm.callee_saved_to_save.iter() {
                let __reg = Reg::from_index(__preg.num, __preg.class);
                let __bytes = encode(&Inst::#push_vn { #push_fid: __reg }).map_err(|e| crate::IrError::Emit(e))?;
                __sink.put_bytes(&__bytes);
            }
        });
        let imm = frame_size_imm(frame);
        if let Some(s) = sp_adjust_stmt(infos, frame, Role::FrameAlloc, &imm, true, true)? {
            post.push(s);
        }
    } else {
        if let Some(s) = frame_set_stmt(infos, frame, false)? {
            pre.push(s);
        }
        // `sp -= callee-saved 区`：把 sp 从 fp 退到最后一个 push 槽。**长度取运行时值**
        // （与上面的 push 循环同源），否则"少保存时 sp 落点错位"。
        // `trail = false` = 模板时代的字面量形态。
        let cs_imm = quote! { (__rm.callee_saved_to_save.len() as i64) * (__SLOT_BYTES as i64) };
        if let Some(s) = sp_adjust_stmt(infos, frame, Role::FrameAlloc, &cs_imm, false, false)? {
            pre.push(s);
        }
        pre.push(quote! {
            for __preg in __rm.callee_saved_to_save.iter().rev() {
                let __reg = Reg::from_index(__preg.num, __preg.class);
                let __bytes = encode(&Inst::#pop_vn { #pop_fid: __reg }).map_err(|e| crate::IrError::Emit(e))?;
                __sink.put_bytes(&__bytes);
            }
        });
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
    model: &IsaModel,
    frame: &AbiFrame,
    is_prologue: bool,
    pre: &mut Vec<TokenStream>,
    post: &mut Vec<TokenStream>,
) -> Result<(), String> {
    let _ = post; // 帧内机制没有"收参之后"的步（收参固定在最后）
    let slot = model.slot_bytes()? as i64;
    let fp_push = frame.fp_push_bytes.unwrap_or(0) as i64;
    let sp = format_ident!("{}", frame.sp);
    let link = model.machine_link_reg().map(str::to_string);
    // 帧顶保存区（v20 V7）：形状要求"保存区与栈实参连续"时（`AllocResult.va_top` > 0），
    // 保存区占 `[入口 sp - va_top, 入口 sp)`，于是 **fp/lr 保存槽与 callee-saved 整体下移
    // 一个 `va_top`**（`va_top = 0` 时与历史逐字节一致）。这里把它写进下面三个偏移表达式。
    let va_top = quote! { __rm.va_top as i64 };
    // 帧顶 fp 保存区：`fp_push_bytes` 个字节里装 link（低偏移）+ fp（高偏移）。
    let fp_off = {
        let lit = proc_macro2::Literal::i64_suffixed(fp_push);
        quote! { (__frame_size as i64) - #lit - #va_top }
    };
    let link_off = {
        let lit = proc_macro2::Literal::i64_suffixed(slot);
        quote! { (__frame_size as i64) - #lit - #va_top }
    };
    let base = quote! { Reg::#sp };
    // link/fp 的保存槽永远是 GPR（链接寄存器与帧指针都是 GPR）——按 **GPR 类**解析
    // 那一条（没申报类限定时就是唯一的裸声明）。
    let gpr_save_inst =
        role_name_for_class(infos, Role::CalleeSave, crate::dsl::model::RoleClass::Gpr);
    let gpr_load_inst =
        role_name_for_class(infos, Role::CalleeLoad, crate::dsl::model::RoleClass::Gpr);
    if is_prologue {
        let imm = frame_size_imm(frame);
        if let Some(s) = sp_adjust_stmt(infos, frame, Role::FrameAlloc, &imm, true, true)? {
            pre.push(s);
        }
        if fp_push >= 2 * slot
            && let Some(l) = link.clone()
            && let Some(inst) = gpr_save_inst.as_deref()
        {
            let l = format_ident!("{l}");
            let value = quote! { Reg::#l };
            if let Some(s) = callee_mem_stmt(infos, inst, &value, &base, &link_off, false)? {
                pre.push(s);
            }
        }
        if fp_push >= slot
            && let Some(f) = frame.fp.clone()
            && let Some(inst) = gpr_save_inst.as_deref()
        {
            let f = format_ident!("{f}");
            let value = quote! { Reg::#f };
            if let Some(s) = callee_mem_stmt(infos, inst, &value, &base, &fp_off, false)? {
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
            && let Some(inst) = gpr_load_inst.as_deref()
        {
            let l = format_ident!("{l}");
            let value = quote! { Reg::#l };
            if let Some(s) = callee_mem_stmt(infos, inst, &value, &base, &link_off, false)? {
                pre.push(s);
            }
        }
        if fp_push >= slot
            && let Some(f) = frame.fp.clone()
            && let Some(inst) = gpr_load_inst.as_deref()
        {
            let f = format_ident!("{f}");
            let value = quote! { Reg::#f };
            if let Some(s) = callee_mem_stmt(infos, inst, &value, &base, &fp_off, false)? {
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
fn gen_arg_receive(infos: &[InstInfo], model: &IsaModel) -> Result<TokenStream, String> {
    // callee-saved 区字节数（生成期常量，与 frame_layout::callee_saved_bytes
    // 一致：fp 保存槽 + callee-saved × 槽单位）——move_args 收 spilled 栈参数时
    // 计算 spill 槽地址 sp_base = -(frame) - callee_saved + stack_arg_bytes。
    // 缺省/步长全部元数据派生（x86 = 8；1 字节寄存器 ISA = 1）。
    let slot_bytes_lit = model.slot_bytes()? as i64;
    let callee_saved_bytes_lit: i64 = {
        let fp_push = model
            .machine_frame()
            .and_then(|f| f.fp_push_bytes)
            .unwrap_or(model.addr_class()?.bytes() as u32) as i64;
        // **机器事实**（v20 A6）：帧按几个推入槽算——与「哪些寄存器必须保住」（约定）
        // 分开；`[machine].callee_save_slots` 缺省回退谱面 `[abi].callee_saved` 的表长。
        let cs = model.machine_callee_save_slots() as i64 * slot_bytes_lit;
        fp_push + cs
    };
    // 收参：v20 A5-3 起**只走布局路径**（`ArgPlace::Reg`/`Indirect`/`Stack`）。
    // 谱面的 `[abi].arg_class` 已删除——拿不到布局或落点不受支持时，下面的
    // `*_stmt_use` 一律 **fail-closed**（旧实现按谱面顺序塞寄存器，那是静默错值）。
    // MOV_RM8_R64 src=arg_reg、dest=param 分配寄存器（0x89: reg=src、rm=dest）
    // 指令名可配置（[abi].move_inst，缺省 "MOV_RM8_R64"）——demo 等
    // 定宽 ISA 声明自己的 mov（如 "MOV64"）；字段按角色解析（In=src、
    // Out/InOut=dest），两种操作数序（x86 src/dest 与 demo dest/src）皆可。
    // v20 A5-3/V8：谱面不再有 `[abi]`，**搬运指令由派生表说话**——没有 GPR ← GPR 搬运的
    // ISA（如只做编码试点的夹具）没有收参可生成 ⇒ 空实现（Call/Return 那边同样降级）。
    let moves = MoveTable::collect(infos)?;
    let gpr_want = Want {
        dst: Shape::Reg(Bank::Gpr),
        src: Shape::Reg(Bank::Gpr),
    };
    let fpr_want = Want {
        dst: Shape::Reg(Bank::Fpr),
        src: Shape::Reg(Bank::Fpr),
    };
    let to_gpr_want = Want {
        dst: Shape::Reg(Bank::Gpr),
        src: Shape::Reg(Bank::Fpr),
    };
    let to_fpr_want = Want {
        dst: Shape::Reg(Bank::Fpr),
        src: Shape::Reg(Bank::Gpr),
    };
    if !moves.has(&gpr_want) {
        return Ok(quote! {});
    }
    let addr_bits: u16 = model.addr_class()?.bits();
    let mov_arm = moves.pick(&gpr_want, addr_bits)?;
    let mov_vn = mov_arm.vn.clone();
    let (m_src, m_dest) = (mov_arm.src.clone(), mov_arm.dst.clone());
    // **类间位搬移**（v20 V7/V8，被调方视角）：落点的类与值所在的池不一致时的按位搬移。
    //
    // 方向与调用点**相反**，因为数据流向相反：被调方是"落点寄存器（约定给，如 a0）→ 值自己的
    // FPR"，所以"落点是整数 + 值是浮点"要用 **`(Fpr ← Gpr)`** 那条搬运（`fmv.d.x f<dest>, a0`）。
    // 缺 ⇒ 该支明确 `Unsupported`（旧实现把 FPR 的号当 GPR 号用 = 静默错值）。
    let has_to_fpr = moves.has(&to_fpr_want);
    let has_to_gpr = moves.has(&to_gpr_want);
    let cross = |want: &Want, have: bool| -> Result<TokenStream, String> {
        if !have {
            return Ok(quote! {
                return Err(crate::IrError::Unsupported(
                    "ISA-DSL move_args: ABI 落点的寄存器类与值的类不同（整数约定收浮点），需要跨类的\
                     位搬移指令（在指令上写 `data_width`，方向由操作数结构定）\
                     ——本 ISA 没有（拿同类搬移顶上是静默错值）"
                        .into(),
                ));
            });
        }
        let bits = quote! { __a.size.wrapping_mul(8) };
        moves.dispatch(want, &bits, |arm| {
            let vn = &arm.vn;
            let (dest, src) = (&arm.dst, &arm.src);
            let dest_cls = if matches!(want.dst, Shape::Reg(Bank::Fpr)) {
                quote! { __DEFAULT_FPR_CLASS }
            } else {
                quote! { __DEFAULT_GPR_CLASS }
            };
            quote! {
                let __bytes = encode(&Inst::#vn {
                    #dest: Reg::from_index(__dest, #dest_cls),
                    #src: Reg::from_index(*index, *class),
                })
                .map_err(|e| crate::IrError::Emit(e))?;
                __sink.put_bytes(&__bytes);
            }
        })
    };
    let callee_to_fpr = cross(&to_fpr_want, has_to_fpr)?;
    let callee_to_gpr = cross(&to_gpr_want, has_to_gpr)?;
    // 栈参数收参指令：**按语义角色**取（`stack_arg_load` / `stack_arg_store`，
    // 角色全 ISA 唯一、validate 保证）。缺角色 → `None`，由下面各调用点给出
    // 生成期的明确错误——**不再用 x86 指令名（Mov64Rm/Mov64Mr + mem/dest/src）
    // 兜底**（那样等于把某个 ISA 的命名约定写进通用生成器；见
    // docs/reference/isa-dsl.md「角色缺失 → 明确 Unsupported，不再静默去查一个
    // 别的 ISA 的指令名」）。
    // **栈参数能力 = 角色声明**（v20 A5-3 收口）：谱面不再有 `[abi.stack_args]`，
    // 布局事实（shadow / 首个栈参偏移 / 每个栈参的落点）全部来自 plan 的
    // `CallLayout`（`ArgPlace::Stack { offset }`）；"本 ISA 支不支持栈参数"就只能由
    // `roles = ["stack_arg_load"]` 说话（validate 保证角色全 ISA 唯一）。
    let has_stack_arg =
        crate::dsl::codegen::lowering::inst_by_role(infos, Role::StackArgLoad).is_some();
    // 栈参数内存基址 = `[machine.frame].fp`（x86 = RBP）：被调方的传入参数区相对
    // **帧指针**（fp-outside 机制）。历史实现写死字面量 `Reg::RBP`——非 x86 ISA
    // 一旦支持栈参数会生成引用不存在寄存器的代码；这里元数据派生 + 生成期
    // fail-closed（fp 未声明 → 报错，不按别家寄存器名兜底）。
    let callee_base: TokenStream = match model
        .machine_frame()
        .and_then(|f| f.fp.clone())
        .filter(|n| !n.is_empty())
    {
        Some(n) => {
            let id = format_ident!("{n}");
            quote! { Reg::#id }
        }
        None if has_stack_arg => {
            return Err(
                "move_args: 本 ISA 声明了 roles = [\"stack_arg_load\"]（支持栈参数），但 \
                 [machine.frame].fp 缺失——栈参数内存基址需要帧指针（不回退字面量 \"RBP\"）"
                    .into(),
            );
        }
        None => quote! { Reg::from_index(0, __DEFAULT_GPR_CLASS) },
    };
    // (变体名, 值寄存器槽, 形状)——load 的 Reg 槽是 dest、store 是 src，形状由
    // `stack_mem_shape` 从操作数结构派生（`Reg+Mem` 或 `值Reg+基址Reg+位移Imm` 两种）。
    let tagged = |role: Role,
                  what: &str|
     -> Result<Option<crate::dsl::codegen::lowering::StackMemShape>, String> {
        // **无限定**声明（发射要的是那条通用指令；类/宽度分派另有出口）——
        // 见 `inst_by_plain_role` 的说明：宽松查找会静默选中类限定版。
        let Some(info) = crate::dsl::codegen::lowering::inst_by_plain_role(infos, role) else {
            if has_stack_arg {
                return Err(format!(
                    "move_args: 本 ISA 声明了 roles = [\"stack_arg_load\"]（支持栈参数），但缺 roles = [\"{role}\"] 的指令（不按指令名兜底）"
                ));
            }
            return Ok(None);
        };
        crate::dsl::codegen::lowering::stack_mem_shape(infos, info, role)
            .map_err(|e| format!("move_args: {what}：{e}"))
    };
    let load_shape = tagged(Role::StackArgLoad, "收参指令")?;
    let store_shape = tagged(Role::StackArgStore, "写回指令")?;
    // 浮点版的写回指令（`{ role = "stack_arg_store", class = "fpr" }`，v20 变参 V3）：
    // 寄存器保存区里既有 GP 也有 FP 槽，spill 按槽的类分派。
    let store_fpr_shape = match crate::dsl::codegen::lowering::role_name_for_class(
        infos,
        Role::StackArgStore,
        crate::dsl::model::RoleClass::Fpr,
    )
    .and_then(|name| infos.iter().find(|i| i.inst.name == name))
    {
        Some(info) => {
            crate::dsl::codegen::lowering::stack_mem_shape(infos, info, Role::StackArgStore)
                .map_err(|e| format!("move_args: 浮点写回指令：{e}"))?
        }
        None => None,
    };
    // 栈参数收参的 scratch 寄存器（`[machine].spill_scratch` 首项；迁移期回退
    // `[abi].scratch`）与 callee-saved 区字节数（sp_base 计算常量）。**只在真的要走
    // 栈参数收参时**要求声明（声明了 `stack_arg_load` 角色的 ISA）；缺声明 = 生成期
    // 明确报错——不回退到某个 ISA 的寄存器名（那等于把别家的命名约定写进通用生成器）。
    let scratch0 = match model.machine_scratch().first() {
        Some(s) => format_ident!("{s}"),
        None if has_stack_arg => {
            return Err("move_args: 本 ISA 声明了 roles = [\"stack_arg_load\"]（支持栈参数），但 [machine].spill_scratch 未声明——栈参数收参需要一个临时寄存器（不按某个 ISA 的寄存器名兜底）"
                        .into());
        }
        None => format_ident!("__unused_scratch"),
    };
    let cs_bytes = callee_saved_bytes_lit;
    // 浮点参数收参（v20 V8 派生）：**一条道**——按参数**位宽**（`__a.size × 8`）在这张
    // 派生表上取"最窄覆盖者"，不写死 32/64。未申报的位宽 ⇒ 生成物运行期明确 `Unsupported`。
    let has_fpr_mov = moves.has(&fpr_want);
    // 有没有**整寄存器宽（128 位）**的搬移：有 = 按值向量（≤16B 的向量类）走全宽槽搬移；
    // 没有（如 riscv 只有 fsgnj.d/fsgnj.s）⇒ 一律按参数类型宽度走，不假装有向量能力。
    let has_vec_mov = moves.covers(&fpr_want, 128);
    let fpr_body = |bits: &TokenStream, what: &str| -> Result<TokenStream, String> {
        if !has_fpr_mov {
            let msg = format!("ISA-DSL {what}: 本 ISA 没有「浮点寄存器 ← 浮点寄存器」的搬运指令");
            return Ok(quote! {
                return Err(crate::IrError::Unsupported(#msg.into()));
            });
        }
        moves.dispatch(&fpr_want, bits, |arm| {
            let vn = &arm.vn;
            let (dest, src) = (&arm.dst, &arm.src);
            let third: Vec<TokenStream> = arm
                .extra
                .iter()
                .map(|(f, _)| quote! { #f: __src, })
                .collect();
            quote! {
                let __bytes = encode(&Inst::#vn {
                    #dest: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                    #src: __src,
                    #(#third)*
                })
                .map_err(|e| crate::IrError::Emit(e))?;
                __sink.put_bytes(&__bytes);
            }
        })
    };
    let wrap_src = |body: TokenStream| -> TokenStream {
        quote! {
            {
                let __src = Reg::from_index(*index, *class);
                #body
            }
        }
    };
    // 按值向量（≤16B，VEC(16)——V64/V128）收参用**整寄存器槽**（128 位）搬运：
    // `MOVSD`/`MOVSS` 只移动 8/4 字节，高半被静默截断/依赖寄存器遗留值。
    let vec_mov_body: TokenStream = if has_vec_mov {
        wrap_src(fpr_body(&quote! { 128u32 }, "float/vector args")?)
    } else {
        quote! {}
    };
    let fpr_by_size_body: TokenStream = if has_fpr_mov {
        wrap_src(fpr_body(
            &quote! { __a.size.wrapping_mul(8) },
            "float args",
        )?)
    } else {
        quote! {}
    };
    // by-value 向量阈值 = `[abi.arg_class].limit`（by-ref 策略，字节；
    // x86 = 16B）——元数据驱动，取代写死的 `class == VEC(16)` 判定
    // （1 字节/非常规宽度 ISA 的向量类不是 VEC(16)）。
    let fpr_pool_w = model.value_fpr_class()?.map_or(16, |c| c.bytes());
    let vec_by_val_max = model.vector_by_ref_limit_bytes()?.unwrap_or(fpr_pool_w);
    // 生成物里的类宽是**位**（`RegClass` payload 口径），阈值是**字节** ⇒ 换算一次。
    let vec_by_val_max_bits: u16 = vec_by_val_max * 8;
    // by-ref 向量 load：宽向量参数（>16 字节）按引用传参——ABI 传 GPR
    // 指针（int 槽位），收参时从 [ptr] load 到目标向量寄存器。
    // **按形状 + 精确宽度派生**（v20 V8）：内存 ↔ 向量寄存器的搬运指令由 `MoveTable`
    // 按参数**字节宽**选（`data_width` 说宽度）。不做「按指令名搜索 + 非对齐优先/对齐
    // 兜底」的隐式回退——ISA 把 `data_width` 打在自己选定的那条指令上即可（x86 打在
    // 非对齐 VMOVUPS_* 上），生成器只认结构、不猜名字；字段名同样由操作数结构派生。
    // **宽度按参数 IR 类型字节数分派**（`__rm.param_bytes`，与
    // param_vregs 对齐）：寄存器类宽对 >128 位向量恒为 VEC(32)
    //（`reg_class_for`），旧实现用 `__pv.width()` → 64B（V512）分支
    // 永不可达、只 load 32B（lane8..15 丢失，WA-37 D5）。
    // 32 字节（V256）/ 64 字节（V512）各要一条**精确 256/512 位**的 load——
    // 其它宽度没有对应搬运 ⇒ 显式 Unsupported（不静默截断）。
    //（不静默截断）。
    let mut byref_32: Option<TokenStream> = None;
    let mut byref_64: Option<TokenStream> = None;
    // 宽度是**字节**（32 = V256、64 = V512），`data_width` 里写的是**位**（256/512）——
    // 内存操作数要求精确宽度，派生表按形状+精确宽度挑。
    let byref_want = Want {
        dst: Shape::Reg(Bank::Fpr),
        src: Shape::Mem,
    };
    for width in [32u16, 64u16] {
        let Ok(arm) = moves.pick(&byref_want, width * 8) else {
            continue; // 本 ISA 没有这个宽度的 load → 该宽度不可用（下方给 Unsupported）
        };
        let (d_fid, m_fid) = (arm.dst.clone(), arm.src.clone());
        let vn = arm.vn.clone();
        let stmt: TokenStream = quote! {
            let __bytes = encode(&Inst::#vn {
                #d_fid: Reg::from_index(__dest, __DEFAULT_FPR_CLASS),
                #m_fid: MemRef {
                    base: Some(Reg::from_index(__src.to_index(), __ADDR_CLASS)),
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
                    "ISA-DSL by-ref vector arg: unsupported width (仅支持 32B/64B 向量；其它 >32B 宽度无 load 变体)".into(),
                ));
            }
        },
        (Some(s32), None) => s32,
        (None, Some(s64)) => s64,
        (None, None) => quote! {
            return Err(crate::IrError::Emit(
                "ISA-DSL by-ref vector arg load missing（本 ISA 没有「向量寄存器 ← 内存」的搬运指令）".into(),
            ));
        },
    };
    // **收参只走布局路径**（v20 A5-3）：谱面的 `[abi].arg_class` 已删除。没有布局
    // （`__layout_ok == false`：Pair/Group/无指针 Indirect，或宿主没给约定数据）
    // 一律 **fail-closed**——旧实现按谱面顺序把值塞进"第几个寄存器"，那是静默错值。
    let (head, fpr_stmt_use, int_stmt_use, byref_stmt_use): (
        TokenStream,
        TokenStream,
        TokenStream,
        TokenStream,
    ) = (
        quote! {},
        quote! {
            return Err(crate::IrError::Unsupported(
                "ISA-DSL move_args: 浮点/向量参数收参需要调用布局（plan）".into(),
            ));
        },
        quote! {
            return Err(crate::IrError::Unsupported(
                "ISA-DSL move_args: 整数参数收参需要调用布局（plan）".into(),
            ));
        },
        quote! {
            return Err(crate::IrError::Unsupported(
                "ISA-DSL move_args: by-ref 参数收参需要调用布局（plan）".into(),
            ));
        },
    );
    let _ = byref_stmt_use;
    // 栈参数收参（`ArgPlace::Stack` 且被 regalloc 强制 spill 的那个参数）**无条件**
    // 先收进 spill 槽（regalloc 强制 spill 保证有槽；即使分配了 preg 也不走寄存器
    // 分支——否则与低位置参数共享寄存器时，批量收参顺序覆盖（a→r15 后 e→r15，
    // a 值丢）→ five_args 14）。偏移**来自布局**（v20 A5-3：谱面不再声明
    // `[abi.stack_args]`，`first_offset_slots`/`shadow`/`stride` 三处常量都删了）。
    let stack_arg_receive: TokenStream = match (load_shape.as_ref(), store_shape.as_ref()) {
        (Some(l), Some(s)) => {
            let l_inst = mem_inst_toks(l, &callee_base, &quote! { __off }, &quote! { __scratch0 });
            let s_inst = mem_inst_toks(
                s,
                &callee_base,
                &quote! { __sp_base + __slot_off },
                &quote! { __scratch0 },
            );
            quote! {
                if let Some(__cl) = __rm.call_layout.as_ref()
                    && let Some(crate::machine::call_layout::ArgPlace::Stack { offset: __soff, .. }) =
                        __cl.arg(__i as u32).map(|__a| &__a.place)
                    && __rm.spill_slots.contains_key(&__pv)
                {
                    let __off = *__soff as i64;
                    let __sp_base =
                        -(__frame_size as i64) - __cs_bytes + __rm.stack_arg_bytes as i64;
                    let __slot_off = __rm.spill_slot(__pv).offset as i64;
                    // load 布局给的栈槽 → scratch（角色 stack_arg_load 命中的指令）
                    let __lbytes = encode(&#l_inst).map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__lbytes);
                    // store scratch → spill 槽（角色 stack_arg_store 命中的指令）
                    let __sbytes = encode(&#s_inst).map_err(|e| crate::IrError::Emit(e))?;
                    __sink.put_bytes(&__sbytes);
                    continue;
                }
            }
        }
        // 本 ISA 不支持栈参数（未声明角色）→ 分支整体不生成
        //（否则分支体引用不存在的 Reg/Inst 变体）。
        _ => quote! {},
    };
    // spilled 的寄存器参数（位置 < n）收参到 spill 槽：需要 MOV 指令
    //（派生的 GPR ← GPR 搬运——不再按 x86 指令名
    // `MOV_RM8_R64` 探测）+ 角色 stack_arg_store（reg→mem）。
    let has_mov_inst = true; // 上面的 moves.has(&gpr_want) 门控已经保证有搬运指令
    let spilled_int_receive: TokenStream = match (
        has_stack_arg && has_mov_inst,
        store_shape.as_ref(),
    ) {
        (true, Some(s)) => {
            let s_inst = mem_inst_toks(
                s,
                &callee_base,
                &quote! { __sp_base + __slot_off },
                &quote! { __scratch0 },
            );
            quote! {
            // spilled 的**寄存器**参数：load 被调方 ABI 寄存器 → scratch
            // → spill 槽（mod.rs 210 写槽依赖 entry vreg 值正确）。
            //
            // 来源寄存器由**被调方布局**给（v20 A5-3）：不再按谱面 `[abi.arg_class]` 的
            // "第 __pos 个寄存器"取——按位置/按类计数、sret 占不占首槽都是引擎算好的；
            // 拿不到布局 ⇒ **fail-closed**（不按谱面顺序猜）。
            if !__rm.param_is_float.get(__i).copied().unwrap_or(false)
                && __rm.spill_slots.contains_key(&__pv)
            {
                let __src = match __rm.call_layout.as_ref().and_then(|__cl| __cl.arg(__i as u32)) {
                    Some(__ca) => match &__ca.place {
                        crate::machine::call_layout::ArgPlace::Reg { class, index, .. } => {
                            Reg::from_index(*index, *class)
                        }
                        _ => {
                            return Err(crate::IrError::Unsupported(
                                "ISA-DSL move_args: spilled 寄存器参数需要布局给的寄存器落点".into(),
                            ));
                        }
                    },
                    None => {
                        return Err(crate::IrError::Unsupported(
                            "ISA-DSL move_args: spilled 寄存器参数需要调用布局（plan）".into(),
                        ));
                    }
                };
                let __sp_base = -(__frame_size as i64) - __cs_bytes + __rm.stack_arg_bytes as i64;
                let __slot_off = __rm.spill_slot(__pv).offset as i64;
                // ABI 寄存器 → scratch（用参数移动指令 mov_vn）
                let __lbytes = encode(&Inst::#mov_vn {
                    #m_src: __src,
                    #m_dest: __scratch0,
                }).map_err(|e| crate::IrError::Emit(e))?;
                __sink.put_bytes(&__lbytes);
                // scratch → spill 槽（角色 stack_arg_store 命中的指令）
                let __sbytes = encode(&#s_inst).map_err(|e| crate::IrError::Emit(e))?;
                __sink.put_bytes(&__sbytes);
            }
            }
        }
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
    // 浮点/向量类落点：按**类宽**分派（与既有路径同一判据——宽类（≤16B 向量、
    // scalars 落在同一 FPR 池）走全宽槽搬移（128 位），否则按参数字节宽走。
    // `has_vec_mov` = 本 ISA 真有 128 位档（riscv 只有 fsgnj.s/d ⇒ 一律按类型宽度）。
    let fp_from_class: TokenStream = if has_vec_mov && has_fpr_mov {
        quote! {
            if class.bits() <= #vec_by_val_max_bits {
                #vec_mov_body
            } else {
                #fpr_by_size_body
            }
        }
    } else if has_fpr_mov {
        fpr_by_size_body
    } else {
        vec_mov_body
    };
    // **布局给的栈落点**收参（A3b-2b-2c-2）：`[frame_base + offset]` → 分配的寄存器。
    // 指令按角色 `stack_arg_load` 取（与旧路径同一条），偏移来自布局（常量）。
    let layout_stack_stmt: TokenStream = match load_shape.as_ref() {
        Some(l) => {
            let l_inst = mem_inst_toks(
                l,
                &callee_base,
                &quote! { *offset as i64 },
                &quote! { Reg::from_index(__dest, __DEFAULT_GPR_CLASS) },
            );
            quote! {
                let __bytes = encode(&#l_inst).map_err(|e| crate::IrError::Emit(e))?;
                __sink.put_bytes(&__bytes);
            }
        }
        None => quote! {
            return Err(crate::IrError::Unsupported(
                "ISA-DSL move_args: 栈参数收参需要 roles = [\"stack_arg_load\"] 的指令".into(),
            ));
        },
    };
    // 寄存器保存区 spill（v20 变参 V3）：需要保存区的约定（sysv64/aapcs64/riscv）在序言里
    // 把**参数寄存器**存进帧内，`va_list` 的 `reg_save_area` 字段指向它。槽表来自 plan
    // （`CallLayout.va.save`），偏移由管线分配（`__rm.va_save_off`）——这里只按槽表发 store，
    // **按槽的寄存器类分派**（GP 槽用通用 store，FP 槽用 `class = "fpr"` 那条）。
    //
    // 放在收参**之前**：收参会用 scratch，先存一份原始参数寄存器最稳。
    let va_save_spill: TokenStream = match store_shape.as_ref() {
        Some(s) => {
            let s_inst = mem_inst_toks(
                s,
                &callee_base,
                &quote! { __disp },
                &quote! { Reg::from_index(__slot.index, __slot.class) },
            );
            let fpr_store: TokenStream = match store_fpr_shape.as_ref() {
                Some(f) => {
                    let f_inst = mem_inst_toks(
                        f,
                        &callee_base,
                        &quote! { __disp },
                        &quote! { Reg::from_index(__slot.index, __slot.class) },
                    );
                    quote! {
                        let __bytes = encode(&#f_inst).map_err(|e| crate::IrError::Emit(e))?;
                    }
                }
                None => quote! {
                    return Err(crate::IrError::Unsupported(
                        "ISA-DSL move_args: 寄存器保存区里的浮点槽需要 \
                         { role = \"stack_arg_store\", class = \"fpr\" } 的指令"
                            .into(),
                    ));
                },
            };
            quote! {
                if let Some(__off) = __rm.va_save_off
                    && let Some(__cl) = __rm.call_layout.as_ref()
                    && let Some(__save) = __cl.va.as_ref().and_then(|__v| __v.save.as_ref())
                {
                    for __slot in __save.slots.iter() {
                        let __disp = __off + __slot.offset as i64;
                        let __bytes = if __slot.class.is_fp() {
                            #fpr_store
                            __bytes
                        } else {
                            let __bytes = encode(&#s_inst).map_err(|e| crate::IrError::Emit(e))?;
                            __bytes
                        };
                        __sink.put_bytes(&__bytes);
                    }
                }
            }
        }
        // 没有通用 store 角色 ⇒ 本 ISA 不支持栈参数/保存区：整条留空（`va_start` 那条路
        // 已经会因缺角色 fail-closed，不在这里重复报）。
        None => quote! {},
    };
    Ok(quote! {
        #head
        // 栈参数收参的 scratch（[machine].spill_scratch 首项）与 callee-saved
        // 字节数（sp_base 计算）——仅 shadow 声明时使用
        #stack_arg_prologue
        #va_save_spill
        // 布局路径的入场判定：**全部**参数都落在受支持的落点才启用
        //（`Reg` / 带指针的 `Indirect` / `Stack`——栈参数的位置由 forge-abi 按
        // `first_offset_slots + shadow + k×slot` 算好，生成器不再自己数位置）。
        let __layout_ok = match __rm.call_layout.as_ref() {
            Some(__cl) => __rm.param_vregs.iter().enumerate().all(|(__li, _)| {
                matches!(
                    __cl.arg(__li as u32).map(|__a| &__a.place),
                    Some(crate::machine::call_layout::ArgPlace::Reg { .. })
                        | Some(crate::machine::call_layout::ArgPlace::Indirect {
                            reg: Some(_),
                            ..
                        })
                        | Some(crate::machine::call_layout::ArgPlace::Stack { .. })
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
                        // 四路（v20 V7）：**落点的类**（约定给）× **值所在的池**（IR 类型给，
                        // `param_is_float`）——不一致时走**类间位搬移**。旧实现只看落点的类：
                        // 浮点值落在整数寄存器时把 FPR 的号当 GPR 号用（**静默错值**）。
                        let __val_fp = __rm.param_is_float.get(__i).copied().unwrap_or(false);
                        if class.is_int() {
                            let __src = Reg::from_index(*index, *class);
                            if __val_fp {
                                #callee_to_fpr
                            } else {
                                let __bytes = encode(&Inst::#mov_vn {
                                    #m_src: __src,
                                    #m_dest: Reg::from_index(__dest, __DEFAULT_GPR_CLASS),
                                })
                                .map_err(|e| crate::IrError::Emit(e))?;
                                __sink.put_bytes(&__bytes);
                            }
                        } else if class.is_fp() {
                            if __val_fp {
                                #fp_from_class
                            } else {
                                #callee_to_gpr
                            }
                        } else {
                            // 既不是整数类也不是浮点/向量类（如掩码寄存器类）——
                            // 本片没有对应的搬运角色，**明确拒绝**而不是当浮点搬。
                            return Err(crate::IrError::Unsupported(
                                "ISA-DSL move_args: 布局给的寄存器类没有收参搬运指令".into(),
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
                    crate::machine::call_layout::ArgPlace::Stack { offset, .. } => {
                        // 栈参数：**偏移由 forge-abi 给**（被调方视角，已含 shadow 与
                        // `first_offset_slots`），生成器不再自己数位置。
                        //
                        // **实际到不了这一支**（2026-10-01 核实）：ABI 落在栈上的形参没有
                        // 入场寄存器 ⇒ 不在 `assignments` 里 ⇒ 上面的 `#stack_arg_receive`
                        // 已经把它收进 spill 槽并 `continue` 了。那条路是
                        // `load 布局槽 → scratch(GPR) → store spill 槽` 的**按位搬运**，
                        // 因此**浮点参数也正确**（位型不变），FPR 溢出槽的 load 再按类还原。
                        // 两条真跑用例：`test_jit_float_params_beyond_xmm_registers_come_from_the_stack`
                        // （6 个 f64 形参取后两个）与
                        // `test_jit_stack_float_param_is_received_into_a_register`（只用第 5 个）。
                        //
                        // 留下的这一支是**防御性**的：万一将来某条路径让"栈落点 + 已分配寄存器"
                        // 同时成立，浮点必须走类限定收参（`{ role = "stack_arg_load",
                        // class = "fpr" }`）而不是把 FPR 当整数搬——所以这里**明确拒绝**。
                        if __rm.param_is_float.get(__i).copied().unwrap_or(false) {
                            return Err(crate::IrError::Unsupported(
                                "ISA-DSL move_args: 栈上的浮点参数还有'直接收进寄存器'的路径\
                                 （本片未接：需要 { role = \"stack_arg_load\", class = \"fpr\" }）"
                                    .into(),
                            ));
                        }
                        #layout_stack_stmt
                    }
                    _ => {
                        return Err(crate::IrError::Internal(
                            "ISA-DSL move_args: 布局落点未过 __layout_ok 判定".into(),
                        ));
                    }
                }
                continue;
            }
            if __rm.param_by_ref.get(__i) == Some(&true) {
                // by-ref：宽向量参数按引用传——GPR 槽位是数据指针，
                // 从 [ptr] load 到向量寄存器（派生：向量寄存器 ← 内存，精确宽度）。
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
    model: &IsaModel,
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
        Some(c) => c.bytes(),
        None => model.main_fpr_class()?.map(|c| c.bytes()).unwrap_or(0),
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
                        quote! { MemRef { base: Some(Reg::#base), disp: __off, index: None, scale: 1 } }
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
    let vn = crate::dsl::codegen::pascal_ident(inst_name);
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
