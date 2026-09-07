//! v12 codegen — TargetMachine 集成层（迭代 5）。
//!
//! 自包含模块（encode/decode/disassemble/assemble）之上生成 forge-codegen
//! 组件：MachineInst impl、TargetEncoder/Decoder/Disassembler/Assembler、
//! TargetABI、TargetFrameLowering、TargetLowering、TargetMachine 组装。
//!
//! 关键语义（与 v11 一致）：
//! - Inst 寄存器字段为类型化 `Reg` 枚举（v12 类型化重构：不再裸 u32）：
//!   lowering 构造时占位 `Reg::from_index(0, class)`，regalloc 经 `xreg_map`
//!   分配后 `set_reg_field` 回填物理索引（`Reg::from_index` 重建枚举），
//!   encode 读 `to_index()`。
//! - `uses()/defs()` 返回寄存器字段的物理索引（回填后即分配结果）；regalloc
//!   的活区间由 pipeline 聚合的 `xreg_map` 驱动（见 pipeline/liverange.rs），
//!   MachineInst::uses/defs 仅作辅助查询。
//! - effect 标签（指令 `effect` 键）驱动 is_branch/is_call/is_ret/effects。
//!
//! ## x86 缺省语义（其他 ISA 应显式覆盖）
//! 下列 ABI/集成键的缺省值 = x86 指令名/语义。x86 是参考实现，缺省合理；
//! riscv/demo 等已在 TOML `[abi]` 显式声明自己的指令（如 `move_inst =
//! "MV"`、`call_inst = "JAL"`、`ret_mov_inst = "MV"`）。缺省清单：
//! - `[abi].move_inst` 缺省 `"MOV_RM8_R64"`（整数收参/返参移动）
//! - `[abi].ret_mov_inst` 缺省 `"MOV_RM8_R64"`
//! - `[abi].call_inst` 缺省 `"CALL_RIP_REL"`（rel32 函数符号调用）
//! - 浮点参数/返回移动硬编码 `MOVSD`/`MOVSS`（f64/f32；缺失 → 浮点路径
//!   降级 Unsupported）
//! - 尾声跳转：变长 ISA 缺省 `JMP_REL32`（0xE9 rel32）；定宽缺省 JAL
//!   （[emit].epilogue_label 覆盖）
//! - 条件码表/前缀扫描缺省 = x86 集（`cond_default`/`x86_scan_default`，
//!   见 codegen/asm.rs 与 codegen/vlen.rs）
//!
//! 新增 ISA 时若这些指令不存在，调用/参数/尾声路径会按缺省名查找失败并
//! 报错（或降级 Unsupported）——优先在 TOML 显式声明。

use crate::v12::pred::CmpOp;

use super::super::model::*;
use super::super::pred::Pred;
use super::super::shared::group_names;
use super::InstInfo;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

/// 解析 lowering 模板立即数字面量：支持十进制、`0x`/`0X` 十六进制、
/// 负号（含 `-0x…`），超过 i64 正范围的 64 位 hex 按 u64 解析后转
/// 两补码（如 `0x8000000000000000` → i64::MIN）。
pub(crate) fn parse_i64_lit(text: &str) -> Result<i64, ()> {
    let t = text.trim();
    let (neg, body) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t),
    };
    let v = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).map_err(|_| ()).or_else(|_| {
            // 超出 i64 正范围 → 按 u64 取两补码
            u64::from_str_radix(hex, 16)
                .map(|u| u as i64)
                .map_err(|_| ())
        })?
    } else {
        body.parse::<i64>().map_err(|_| ())?
    };
    Ok(if neg { v.wrapping_neg() } else { v })
}

/// 结构化谓词 → Rust 布尔表达式（`__attr(name) -> Option<i64>` 求值；
/// 未知属性 → false，与 pred.rs eval 语义一致）。
pub(crate) fn compile_pred_guard(pred: &Pred, attr: &syn::Ident) -> TokenStream {
    match pred {
        Pred::And(ps) => {
            let gs: Vec<_> = ps.iter().map(|p| compile_pred_guard(p, attr)).collect();
            quote! { (#(#gs)&&*) }
        }
        Pred::Or(ps) => {
            let gs: Vec<_> = ps.iter().map(|p| compile_pred_guard(p, attr)).collect();
            quote! { (#(#gs)||*) }
        }
        Pred::Not(p) => {
            let g = compile_pred_guard(p, attr);
            quote! { !(#g) }
        }
        Pred::Cmp(op, name, want) => {
            let n = syn::LitStr::new(name, proc_macro2::Span::call_site());
            let f = match op {
                CmpOp::Eq => quote! { == },
                CmpOp::Ne => quote! { != },
                CmpOp::Lt => quote! { < },
                CmpOp::Le => quote! { <= },
                CmpOp::Gt => quote! { > },
                CmpOp::Ge => quote! { >= },
            };
            quote! { #attr(#n).map_or(false, |__g| __g #f #want) }
        }
        Pred::In(name, vals) => {
            let n = syn::LitStr::new(name, proc_macro2::Span::call_site());
            quote! { #attr(#n).map_or(false, |__g| [#(#vals),*].contains(&__g)) }
        }
    }
}

/// lowering 谓词的运行时属性源：预计算到 Option<i64> 局部（避免闭包捕获 ctx）。
/// `cond` = Fcmp/Icmp 条件 id（fcmp_id/icmp_id；非比较 op 恒 0）。
/// `elem` = 结果类型 id（向量 → 元素类型 id；F32=1/F64=2/I32=3/I64=4）。
pub(crate) fn gen_lowering_attrs() -> TokenStream {
    quote! {
        let __a_rd = results.first().and_then(|x| ctx.xreg_types.get(x)).map(|t| {
            if ctx.type_ctx.as_ref().is_some_and(|tc| tc.is_vector(*t)) {
                (ctx.type_ctx.as_ref().map(|tc| tc.size_bytes(*t)).unwrap_or(0) * 8) as i64
            } else {
                t.bits() as i64
            }
        });
        let __a_rs1 = args.first().and_then(|x| ctx.xreg_types.get(x)).map(|t| {
            if ctx.type_ctx.as_ref().is_some_and(|tc| tc.is_vector(*t)) {
                (ctx.type_ctx.as_ref().map(|tc| tc.size_bytes(*t)).unwrap_or(0) * 8) as i64
            } else {
                t.bits() as i64
            }
        });
        let __a_rs2 = args.get(1).and_then(|x| ctx.xreg_types.get(x)).map(|t| {
            if ctx.type_ctx.as_ref().is_some_and(|tc| tc.is_vector(*t)) {
                (ctx.type_ctx.as_ref().map(|tc| tc.size_bytes(*t)).unwrap_or(0) * 8) as i64
            } else {
                t.bits() as i64
            }
        });
        let __a_elem = results.first().and_then(|x| ctx.xreg_types.get(x)).map(|t| {
            if ctx.type_ctx.as_ref().is_some_and(|tc| tc.is_vector(*t)) {
                ctx.type_ctx
                    .as_ref()
                    .and_then(|tc| tc.element_type(*t))
                    .map(elem_id_of)
                    .unwrap_or(0)
            } else {
                elem_id_of(*t)
            }
        });
        // 向量大小标记（字节）：结果/实参是向量类型 → Some(size_bytes)，否则
        // None。Store 无结果（elem 恒 None）时据此区分向量 store 与标量
        //（WA-37 D3：≤16B Direct 向量值的 Load/Store 需全宽向量内存移动）。
        let __a_rd_vec = results.first().and_then(|x| ctx.xreg_types.get(x)).and_then(|t| {
            ctx.type_ctx.as_ref().and_then(|tc| {
                let s = tc.borrow();
                if s.is_vector(*t) {
                    Some(s.size_bytes(*t) as i64)
                } else {
                    None
                }
            })
        });
        let __a_rs1_vec = args.first().and_then(|x| ctx.xreg_types.get(x)).and_then(|t| {
            ctx.type_ctx.as_ref().and_then(|tc| {
                let s = tc.borrow();
                if s.is_vector(*t) {
                    Some(s.size_bytes(*t) as i64)
                } else {
                    None
                }
            })
        });
        let __a_cond = match op {
            crate::prelude::Opcode::Fcmp { cond } => Some(fcmp_id(cond)),
            crate::prelude::Opcode::Icmp { cond } => Some(icmp_id(cond)),
            _ => None,
        };
        let __a_imm0 = ctx.current_immediates.first().copied().map(|v| v as i64);
        // `iconst` = 当前指令常量池解析出的**真值**（signed i64）。
        // 与 `imm0` 的区别：Iconst 的 immediate 是 `Immediate::Const(cid)`
        // （builder 统一 `insert_int` 入池），imm0 只是池索引（正数）——
        // 判断符号/大小必须用池解析值。Constant 引用恒以 cid 指向池条目。
        let __a_iconst = ctx.constant_pool.as_ref().and_then(|p| {
            p.resolve_int(crate::prelude::ConstId(ctx.current_const_index))
        });
        let __attr = |name: &str| -> Option<i64> {
            match name {
                "rd" => __a_rd,
                "rs1_width" => __a_rs1,
                "rs2_width" => __a_rs2,
                "rd_vec" => __a_rd_vec,
                "rs1_vec" => __a_rs1_vec,
                "elem" => __a_elem,
                "cond" => __a_cond,
                "imm0" => __a_imm0,
                "iconst" => __a_iconst,
                _ => None,
            }
        };
    }
}

/// 指令存在性（按 name 精确匹配，**与操作数无关**——RET/NOP 等无操作数
/// 指令存在性必须可判定；`inst_fids` 返回空 vec 仅表示无字段，不表示
/// 指令不存在）。
pub(crate) fn inst_exists(infos: &[InstInfo], name: &str) -> bool {
    infos.iter().any(|i| i.inst.name == name)
}

/// 按指令名取操作数序字段 ident（集成层硬编码构造 Inst 用；字段名随
/// 类型化重构变化，避免各处硬编码 op{i}）。
pub(crate) fn inst_fids<'a>(infos: &'a [InstInfo], name: &str) -> Vec<&'a syn::Ident> {
    infos
        .iter()
        .find(|i| i.inst.name == name)
        .map(|i| i.operands.iter().map(|(_, fid, _, _)| fid).collect())
        .unwrap_or_default()
}

/// 按角色解析移动指令字段：返回 (src 字段名, src 操作数序号, dest 字段名,
/// dest 操作数序号)。src = 第一个 In 角色 Reg 槽；dest = 第一个 Out/InOut
/// 角色 Reg 槽。该角色语义对两种操作数序都成立：x86 的 `MOV_RM8_R64`
///（op0=In=src、op1=InOut=dest）与 riscv 的 `mv`（op0=Out=dest、op1=In=src）。
/// 调用方用 src/dest **序号**做 map_reg_field（vreg 绑定字段），不可硬编码
/// 0/1——那按 x86 方向写死在定宽 ISA（dest=op0）上会把参数移动反成
/// `mv vreg, x0`、返回值接收反成 `mv x0, src`。
pub(crate) fn inst_move_role(
    infos: &[InstInfo],
    name: &str,
) -> Option<(syn::Ident, u8, syn::Ident, u8)> {
    let info = infos.iter().find(|i| i.inst.name == name)?;
    let src = info
        .operands
        .iter()
        .enumerate()
        .find(|(_, (_, _, s, r))| *r == OperandRole::In && s.kind == OperandKind::Reg)?;
    let dest = info.operands.iter().enumerate().find(|(_, (_, _, s, r))| {
        matches!(r, OperandRole::Out | OperandRole::InOut) && s.kind == OperandKind::Reg
    })?;
    Some((src.1.1.clone(), src.0 as u8, dest.1.1.clone(), dest.0 as u8))
}

/// 按指令名取 (所有 Reg 槽字段, 所有 Imm/Label 槽字段)——@frame_alloc 等
/// 需要把"全部 reg 槽填 sp、全部 imm 槽填 frame_size"的指令（x86 的
/// `SUB64_R_IMM32` 是 2 操作数、demo 的 `ADDI16` 是 rd/rs1/imm 3 操作数）。
/// `pub(crate)`：lowering 模块（lowering.rs）复用。
pub(crate) fn inst_reg_imm_fids<'a>(
    infos: &'a [InstInfo],
    name: &str,
) -> Option<(Vec<&'a syn::Ident>, Vec<&'a syn::Ident>)> {
    let info = infos.iter().find(|i| i.inst.name == name)?;
    let regs: Vec<&syn::Ident> = info
        .operands
        .iter()
        .filter(|(_, _, s, _)| s.kind == OperandKind::Reg)
        .map(|(_, f, _, _)| f)
        .collect();
    let imms: Vec<&syn::Ident> = info
        .operands
        .iter()
        .filter(|(_, _, s, _)| matches!(s.kind, OperandKind::Imm | OperandKind::Label))
        .map(|(_, f, _, _)| f)
        .collect();
    Some((regs, imms))
}
/// 生成集成层组件（Reg 枚举 + MachineInst + Encoder + Decoder + Disasm +
/// Assembler + ABI + FrameLowering + Lowering + TargetMachine）。
pub fn gen_integration(infos: &[InstInfo], model: &V12Model) -> Result<TokenStream, String> {
    let reg_enum = super::machine::gen_reg_enum(model)?;
    let machine_inst = super::machine::gen_machine_inst(infos, model)?;
    let encoder = super::machine::gen_encoder(infos, model)?;
    let decoder = super::machine::gen_decoder();
    let disasm = super::machine::gen_disasm(infos)?;
    let assembler = super::machine::gen_assembler(model);
    let abi = super::frame::gen_abi(model)?;
    let frame_lowering = super::frame::gen_frame_lowering(infos, model)?;
    let lowering = super::lowering::gen_lowering(infos, model)?;
    let isa_info = gen_isa_info(model, infos)?;
    let reg_info = gen_reg_info(model)?;
    let target_machine = gen_target_machine(model)?;
    let supported_ops = gen_supported_ops(model);
    Ok(quote! {
        // ── v12 TargetMachine 集成层（迭代 5/6）──
        use forge_ir::PhysReg;
        #reg_enum
        #machine_inst
        #encoder
        #decoder
        #disasm
        #assembler
        #abi
        #frame_lowering
        #lowering
        #isa_info
        #reg_info
        #target_machine
        #supported_ops
    })
}

// ─────────────────────── 能力集（P1-16）───────────────────────

/// P1-16：能力集单一事实源——`[[lowering]].op` 唯一集（排序去重）。
///
/// forge-tests 的 `Capabilities` 由此派生（不再手写同步）；TOML 新增
/// lowering op 后矩阵用例自动转绿。Call/CallIndirect（ABI 专用生成路径）
/// 与 GEP 等无 `[[lowering]]` 条目的 op 由消费方补充声明。
fn gen_supported_ops(model: &V12Model) -> TokenStream {
    let mut ops: Vec<&str> = model.lowering.iter().map(|l| l.op.as_str()).collect();
    ops.sort_unstable();
    ops.dedup();
    let items: Vec<TokenStream> = ops.iter().map(|op| quote! { #op }).collect();
    quote! {
        /// 本 ISA TOML 声明的 lowering op 集（`[[lowering]].op` 唯一集；
        /// 排序去重）。能力集单一事实源——矩阵 `Capabilities` 由此派生。
        pub const SUPPORTED_OPS: &[&str] = &[#(#items),*];
    }
}

// ─────────────────────── IsaInfo / RegInfo ───────────────────────

/// 剥离 asm 模板中的占位符声明 `{n:[slot:role]}` → `{n}`（字面段检查用；
/// 声明里的 `[`/`]` 不是内存形状，字面段的括号保留）。
pub(crate) fn strip_placeholder_decls(asm: &str) -> String {
    let mut out = String::with_capacity(asm.len());
    let mut chars = asm.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut inner = String::new();
            while let Some(&d) = chars.peek() {
                if d == '}' {
                    break;
                }
                inner.push(d);
                chars.next();
            }
            chars.next(); // '}'
            // 仅保留序号部分
            let idx = inner.split(':').next().unwrap_or("").trim();
            out.push('{');
            out.push_str(idx);
            out.push('}');
        } else {
            out.push(c);
        }
    }
    out
}

/// lowering 模板操作数 token → 槽类型签名（形状消歧用）。
/// 委托占位符注册表（placeholder.rs）——分类、临时、xreg、ctor 单一事实源。
pub(crate) fn lowering_token_kind(op: &str) -> &'static str {
    super::placeholder::token_kind(op)
}

fn gen_isa_info(model: &V12Model, infos: &[InstInfo]) -> Result<TokenStream, String> {
    let name_str = &model.meta.name;
    let version_str = model.meta.version.as_deref().unwrap_or("");
    let mode = model.meta.mode;
    let vl = model.meta.variable_length;
    let n_insts = infos.len();
    let max_inst_len = model.meta.max_inst_len.unwrap_or(if vl { 15 } else { 4 });
    let min_inst_len: u8 = if vl { 1 } else { 4 };
    let endian = if model.meta.endian == Endian::Big {
        quote! { forge_ir::Endianness::Big }
    } else {
        quote! { forge_ir::Endianness::Little }
    };
    Ok(quote! {
        pub struct IsaInfo;

        impl crate::machine::isa_info::IsaInfo for IsaInfo {
            fn name(&self) -> &'static str { #name_str }
            fn version(&self) -> &'static str { #version_str }
            fn address_size(&self) -> u8 { #mode }
            fn endianness(&self) -> forge_ir::Endianness { #endian }
            fn capabilities(&self) -> crate::machine::isa_info::IsaCapabilities {
                crate::machine::isa_info::IsaCapabilities {
                    variable_length: #vl,
                    fixed_inst_size: 0,
                    prefix_layers: 1,
                    addressing_modes: &[],
                    simd_widths: &[],
                    mask_registers: false,
                    broadcast: false,
                    rounding_mode: false,
                    endianness: #endian,
                    min_inst_len: #min_inst_len,
                    max_inst_len: #max_inst_len,
                }
            }
            fn num_instructions(&self) -> usize { #n_insts }
        }
    })
}

fn gen_reg_info(model: &V12Model) -> Result<TokenStream, String> {
    // 主 GPR 组（gpr/gpr64）与浮点组（xmm/fpr）数量（names.len() 优先，
    // 缺省 count 字段）。
    let gpr_count = model
        .reg
        .get(&RegClass::GPR(8))
        .or_else(|| model.reg.get(&RegClass::GPR(4)))
        .map(|g| {
            g.names
                .as_ref()
                .map(|n| n.len() as u32)
                .or(g.count.map(|c| c as u32))
                .unwrap_or(16)
        })
        .unwrap_or(16);
    // 主浮点/向量组数量：与默认 FPR 类一致（优先 16 字节 XMM 组——ABI/SSE
    // 占位以 XMM 为基准；ZMM 等 EVEX 组不改变 num_fp_regs，否则 regalloc 会
    // 用 XMM 类分配 16-31 号越界）。riscv F32/F64 同为 32 号组。
    let fpr_count = model
        .reg
        .iter()
        .filter(|(rc, _)| matches!(rc, RegClass::FPR(_)))
        .find(|(rc, _)| **rc == RegClass::FPR(16))
        .or_else(|| {
            model
                .reg
                .iter()
                .filter(|(rc, _)| matches!(rc, RegClass::FPR(_)))
                .max_by_key(|(rc, _)| rc.width())
        })
        .map(|(_, g)| {
            g.names
                .as_ref()
                .map(|n| n.len() as u32)
                .or(g.count.map(|c| c as u32))
                .unwrap_or(0)
        })
        .unwrap_or(0);
    // 主 GPR 组寄存器名（用于 sp/fp 引用解析）。
    let gpr_names: Vec<String> = model
        .reg
        .get(&RegClass::GPR(8))
        .or_else(|| model.reg.get(&RegClass::GPR(4)))
        .map(group_names)
        .transpose()?
        .unwrap_or_default();
    let name_to_idx: std::collections::HashMap<&str, u32> = gpr_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i as u32))
        .collect();
    // SP/FP：优先 [abi.frame].sp/.fp 声明的名字（demo "X7"/"X6" 等自定义
    // 寄存器名），否则按惯例名（"RSP"/"SP"、"RBP"/"FP"）解析；缺失回退
    // 索引（x86 RSP=4/RBP=5；riscv 无显式声明时用 2/8——但组内索引可能
    // 不同，缺省用 0 号避免越界）。
    let frame_sp_name = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .map(|f| f.sp.clone());
    let frame_fp_name = model
        .abi
        .as_ref()
        .and_then(|a| a.frame.as_ref())
        .and_then(|f| f.fp.clone());
    let sp_idx = frame_sp_name
        .as_ref()
        .and_then(|n| name_to_idx.get(n.as_str()).copied())
        .or_else(|| {
            name_to_idx
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("rsp") || n.eq_ignore_ascii_case("sp"))
                .map(|(_, &i)| i)
        })
        .unwrap_or(0);
    let fp_idx = frame_fp_name
        .as_ref()
        .and_then(|n| name_to_idx.get(n.as_str()).copied())
        .or_else(|| {
            name_to_idx
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("rbp") || n.eq_ignore_ascii_case("fp"))
                .map(|(_, &i)| i)
        })
        .unwrap_or(0);
    // sp/fp 引用：能解析到名字 → Reg::NAME；否则用 from_index（避免不存在的变体）
    let sp_ref = frame_sp_name
        .as_ref()
        .and_then(|n| name_to_idx.get(n.as_str()).map(|&i| (n.clone(), i)))
        .or_else(|| {
            name_to_idx
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("rsp") || n.eq_ignore_ascii_case("sp"))
                .map(|(n, &i)| ((*n).to_string(), i))
        })
        .map(|(n, i)| {
            let ident = format_ident!("{n}");
            (quote! { Reg::#ident }, i)
        })
        .unwrap_or_else(|| {
            (
                quote! { <Reg as forge_ir::PhysReg>::from_index(#sp_idx, forge_ir::RegClass::GPR64) },
                sp_idx,
            )
        });
    let fp_ref = frame_fp_name
        .as_ref()
        .and_then(|n| name_to_idx.get(n.as_str()).map(|&i| (n.clone(), i)))
        .or_else(|| {
            name_to_idx
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("rbp") || n.eq_ignore_ascii_case("fp"))
                .map(|(n, &i)| ((*n).to_string(), i))
        })
        .map(|(n, i)| {
            let ident = format_ident!("{n}");
            (quote! { Reg::#ident }, i)
        })
        .unwrap_or_else(|| {
            (
                quote! { <Reg as forge_ir::PhysReg>::from_index(#fp_idx, forge_ir::RegClass::GPR64) },
                fp_idx,
            )
        });
    let sp_expr = sp_ref.0;
    let fp_expr = fp_ref.0;
    let sp_idx = sp_ref.1;
    let fp_idx = fp_ref.1;
    // callee_saved：从 [abi].callee_saved.gpr 解析物理索引（顺序 = prologue push 序）。
    let callee_saved: Vec<TokenStream> = model
        .abi
        .as_ref()
        .and_then(|a| a.callee_saved.as_ref())
        .map(|cs| {
            cs.gpr
                .iter()
                .filter_map(|n| name_to_idx.get(n.as_str()).copied())
                .map(|i| quote! { #i })
                .collect()
        })
        .unwrap_or_default();
    // allocatable：全量 0..count（排除 SP/FP + spill scratch + [abi].reserved）。
    // reserved：不可分配寄存器（riscv X0=zero 写入无效、X1=ra 被 prologue/
    // call 占用、X3/X4=gp/tp）——不排除会分配出垃圾（实测 subw x0 结果丢失）。
    // spill scratch（[abi].scratch = R10/R11）必须排除——emission 的 spill
    // load/store 用 scratch 寄存器，若 regalloc 把活跃 XReg 分配到 scratch，
    // spill 重写会覆盖其值（t+= 循环崩溃：i 地址在 R11 被 spill load 覆盖）。
    // 6i 曾尝试排除但触发循环 spill 暴露 v12 spill bug；6k/6m/6n 修复后
    // 重新排除（scatch 全时保留给 spill 机制）。
    let scratch_idx: std::collections::HashSet<u32> = model
        .abi
        .as_ref()
        .map(|a| {
            a.scratch
                .iter()
                .filter_map(|n| name_to_idx.get(n.as_str()).copied())
                .collect()
        })
        .unwrap_or_default();
    let reserved_idx: std::collections::HashSet<u32> = model
        .abi
        .as_ref()
        .map(|a| {
            a.reserved
                .iter()
                .filter_map(|n| name_to_idx.get(n.as_str()).copied())
                .collect()
        })
        .unwrap_or_default();
    let gp_alloc: Vec<TokenStream> = (0..gpr_count)
        .filter(|&i| {
            i != sp_idx && i != fp_idx && !scratch_idx.contains(&i) && !reserved_idx.contains(&i)
        })
        .map(|i| quote! { #i })
        .collect();
    let fp_alloc: Vec<TokenStream> = (0..fpr_count).map(|i| quote! { #i }).collect();
    // scratch：从 [abi].scratch 解析物理索引（spill load/store 用）。
    let scratch: Vec<TokenStream> = model
        .abi
        .as_ref()
        .map(|a| {
            a.scratch
                .iter()
                .filter_map(|n| name_to_idx.get(n.as_str()).copied())
                .map(|i| quote! { #i })
                .collect()
        })
        .unwrap_or_default();
    Ok(quote! {
        pub struct RegInfo;

        impl crate::machine::reg_info::TargetRegInfo for RegInfo {
            type Reg = Reg;

            fn num_gp_regs(&self) -> u32 { #gpr_count }
            fn num_fp_regs(&self) -> u32 { #fpr_count }
            fn default_gpr_class(&self) -> forge_ir::RegClass { __DEFAULT_GPR_CLASS }
            fn default_fpr_class(&self) -> forge_ir::RegClass { __DEFAULT_FPR_CLASS }
            fn sp_reg(&self) -> forge_ir::FrameAccess<Self::Reg> {
                forge_ir::FrameAccess::Register(#sp_expr)
            }
            fn fp_reg(&self) -> Option<Self::Reg> {
                Some(#fp_expr)
            }
            fn allocatable_gp_order(&self) -> Vec<u32> {
                vec![#(#gp_alloc),*]
            }
            fn allocatable_fp_order(&self) -> Vec<u32> {
                vec![#(#fp_alloc),*]
            }
            fn scratch_regs(&self) -> Vec<u32> {
                vec![#(#scratch),*]
            }
            fn callee_saved(&self) -> Vec<u32> {
                vec![#(#callee_saved),*]
            }
            fn frame_pointer_overhead(&self) -> u32 { 8 }
        }
    })
}

// ─────────────────────── TargetMachine ───────────────────────

fn gen_target_machine(model: &V12Model) -> Result<TokenStream, String> {
    let isa_name_str = &model.meta.name;
    Ok(quote! {
        #[derive(Clone)]
        pub struct TargetMachine {
            isa_info: std::sync::Arc<dyn crate::machine::isa_info::IsaInfo>,
            reg_info: std::sync::Arc<dyn crate::machine::reg_info::TargetRegInfo<Reg = Reg>>,
            abi: std::sync::Arc<dyn crate::machine::abi::TargetABI<Reg = Reg>>,
            lowering: std::sync::Arc<dyn crate::machine::lowering::TargetLowering<Inst = Inst>>,
            encoder: std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Inst>>,
            frame_lowering: std::sync::Arc<dyn crate::machine::frame::TargetFrameLowering<Inst = Inst>>,
            disassembler: std::sync::Arc<dyn crate::machine::disasm::TargetDisassembler<Inst = Inst>>,
            assembler: std::sync::Arc<dyn crate::machine::assembler::TargetAssembler<Inst = Inst>>,
            decoder: std::sync::Arc<dyn crate::machine::decoder::TargetDecoder<Inst = Inst>>,
        }

        impl TargetMachine {
            pub fn new() -> Self {
                Self {
                    isa_info: std::sync::Arc::new(IsaInfo),
                    reg_info: std::sync::Arc::new(RegInfo),
                    abi: std::sync::Arc::new(ABI),
                    lowering: std::sync::Arc::new(Lowering),
                    encoder: std::sync::Arc::new(Encoder),
                    frame_lowering: std::sync::Arc::new(FrameLowering),
                    disassembler: std::sync::Arc::new(Disassembler),
                    assembler: std::sync::Arc::new(Assembler),
                    decoder: std::sync::Arc::new(Decoder),
                }
            }

            pub fn new_arc() -> std::sync::Arc<Self> {
                std::sync::Arc::new(Self::new())
            }
        }

        impl crate::machine::target::TargetMachine for TargetMachine {
            type Inst = Inst;
            type Reg = Reg;

            fn isa_info(&self) -> &std::sync::Arc<dyn crate::machine::isa_info::IsaInfo> { &self.isa_info }
            fn reg_info(&self) -> &std::sync::Arc<dyn crate::machine::reg_info::TargetRegInfo<Reg = Self::Reg>> { &self.reg_info }
            fn abi(&self) -> &std::sync::Arc<dyn crate::machine::abi::TargetABI<Reg = Self::Reg>> { &self.abi }
            fn lowering(&self) -> &std::sync::Arc<dyn crate::machine::lowering::TargetLowering<Inst = Self::Inst>> { &self.lowering }
            fn encoder(&self) -> &std::sync::Arc<dyn crate::machine::encoder::TargetEncoder<Inst = Self::Inst>> { &self.encoder }
            fn frame_lowering(&self) -> &std::sync::Arc<dyn crate::machine::frame::TargetFrameLowering<Inst = Self::Inst>> { &self.frame_lowering }
            fn disassembler(&self) -> Option<&std::sync::Arc<dyn crate::machine::disasm::TargetDisassembler<Inst = Self::Inst>>> { Some(&self.disassembler) }
            fn assembler(&self) -> Option<&std::sync::Arc<dyn crate::machine::assembler::TargetAssembler<Inst = Self::Inst>>> { Some(&self.assembler) }
            fn decoder(&self) -> Option<&std::sync::Arc<dyn crate::machine::decoder::TargetDecoder<Inst = Self::Inst>>> { Some(&self.decoder) }
        }

        crate::impl_erased_target_machine!(TargetMachine);

        /// 注册该 ISA 后端到全局 Registry。
        pub fn ensure_registered() {
            static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
            INIT.get_or_init(|| {
                crate::machine::reloc_patcher::register_default_reloc_patcher(#isa_name_str);
                if !crate::Registry::global().contains(#isa_name_str) {
                    crate::Registry::global().register_backend(TargetMachine::new_arc());
                }
            });
        }
    })
}

/// 解析 lowering/emit 模板中的内存操作数文本 → MemRef 构造表达式。
/// 支持 `{base}+{off}`（base = 物理寄存器名或 `{off}` 偏移符号）。
pub(crate) fn parse_mem_template(text: &str, ctx: &str) -> Result<TokenStream, String> {
    let t = text.trim().trim_start_matches('[').trim_end_matches(']');
    let (base_part, disp_part) = match t.split_once('+') {
        Some((b, d)) => (b.trim().to_string(), Some(d.trim().to_string())),
        None => match t.split_once('-') {
            Some((b, d)) => (b.trim().to_string(), Some(format!("-{d}"))),
            None => (t.trim().to_string(), None),
        },
    };
    let base: TokenStream = if base_part == "{off}" {
        quote! { ctx.current_offset }
    } else {
        let reg = format_ident!("{base_part}");
        quote! { Reg::#reg }
    };
    let disp: TokenStream = match disp_part {
        Some(d) if d == "{off}" => quote! { ctx.current_offset },
        Some(d) if d == "{alloca}" => quote! { ctx.current_alloca_offset as i64 },
        Some(d) => {
            let v: i64 = d
                .parse()
                .map_err(|_| format!("{ctx}: mem disp '{d}' 无法解析"))?;
            quote! { #v }
        }
        None => quote! { 0i64 },
    };
    Ok(quote! { MemRef { base: #base, disp: #disp, index: None, scale: 1 } })
}

/// 从 lowering 模板收集写死的物理寄存器（RAX/RDX 等，非 {占位符}）→
/// clobber 列表（物理索引, RegClass）。regalloc 在本指令点避开。
pub(crate) fn collect_phys_clobbers(
    insts: &[String],
    infos: &[InstInfo],
    model: &V12Model,
) -> Result<Vec<TokenStream>, String> {
    // 主 GPR 组名 → 物理索引
    let gpr_names: Vec<String> = model
        .reg
        .get(&RegClass::GPR(8))
        .or_else(|| model.reg.get(&RegClass::GPR(4)))
        .map(group_names)
        .transpose()?
        .unwrap_or_default();
    let name_to_idx: std::collections::HashMap<&str, u32> = gpr_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i as u32))
        .collect();
    let mut out: Vec<TokenStream> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for t in insts {
        for part in t.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')') {
            let part = part.trim();
            if part.is_empty() || part.starts_with('{') || part.starts_with('@') {
                continue;
            }
            if let Some(&idx) = name_to_idx.get(part)
                && seen.insert(part.to_uppercase())
            {
                out.push(quote! { (#idx, forge_ir::RegClass::GPR64) });
            }
        }
    }
    let _ = infos;
    Ok(out)
}
