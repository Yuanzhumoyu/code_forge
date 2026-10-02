//! **搬运族派生**（v20 V8）：ISA 只写"这条指令搬多少位"（`data_width`），
//! **方向、寄存器族、立即数/内存** 全部由操作数结构（槽 kind/class + 角色
//! `in`/`out`/`inout`）派生——不再有 `gpr_mov`/`fpr_mov`/`vec_mov`/`gpr_mov_imm`/
//! `fpr_to_gpr_mov`/`gpr_to_fpr_mov`/`wide_vec_load`/`wide_vec_store` 这些手写角色。
//!
//! # 为什么这样够
//!
//! 一条搬运指令的四个事实里，**三个已经在操作数结构里**：
//!
//! | 事实 | 来源 |
//! | --- | --- |
//! | 谁进谁出（方向） | 槽角色：`out`/`inout` = 目的，`in` = 来源 |
//! | 哪个寄存器族 | 槽的 `class`/`classes`（`RegClass` 自身带 `GPR`/`FPR`/`VEC`/`KReg`） |
//! | 立即数还是寄存器 | 槽的 `kind`（`Imm`/`Reg`/`Mem`） |
//! | **宽度** | **不在结构里——只能写出来** |
//!
//! 最后一行是这张表存在的唯一理由：x86 的 `MOVSS`/`MOVSD` 共用 `fpr16`(xmm) 槽，
//! riscv 的 `FSGNJ_S`/`FSGNJ_D` 共用 `fpr4` 槽——**同一条指令的槽分不出 32/64**，
//! 而 x86 的 `MOV r/m64, r64` 与 `mov eax, ecx` 也共用一条指令（族内宽度随操作数）。
//! 宽度必须是人写的数据（`data_width`），其余全部派生。
//!
//! # 选谁
//!
//! 请求 = **目的形状 × 来源形状 × 宽度**（形状 = `Reg(族)` / `Imm` / `Mem`）：
//!
//! - 来源是**寄存器或立即数**：值承载在寄存器里，**更宽的搬移同样正确**
//!   （x86 的 `mov rax, rcx` 搬 `i32` 实参不丢低位）⇒ 取**最窄的覆盖者**；
//! - 来源或目的是**内存**：内存操作数的宽度**必须精确**（用 512 位的 load 取 256 位的槽
//!   会越过槽界）⇒ 只取 `data_width == 请求宽度` 者。
//!
//! 候选并列（同形状同宽度多条）⇒ **生成期报错并列出候选**：这是作者该消歧的事，
//! 不设"钉选"注解（那就是第二套机制）。无候选 ⇒ 生成物里 fail-closed `Unsupported`。

use std::collections::BTreeMap;

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Ident;

use super::InstInfo;
use crate::dsl::model::{OperandKind, OperandRole, RegClass};

/// 寄存器族（`RegClass` 自带的分类，不另立注解）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bank {
    Gpr,
    Fpr,
    Vec,
    KReg,
}

impl Bank {
    fn of(class: RegClass) -> Bank {
        match class {
            RegClass::GPR(_) => Bank::Gpr,
            RegClass::FPR(_) => Bank::Fpr,
            RegClass::VEC(_) => Bank::Vec,
            RegClass::KReg(_) => Bank::KReg,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Bank::Gpr => "整数",
            Bank::Fpr => "浮点/向量",
            Bank::Vec => "向量",
            Bank::KReg => "掩码",
        }
    }
}

/// 值落点/来源的形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    /// 寄存器（带族）。
    Reg(Bank),
    /// 立即数。
    Imm,
    /// 内存。
    Mem,
}

impl Shape {
    fn show(self) -> String {
        match self {
            Shape::Reg(b) => format!("{}寄存器", b.name()),
            Shape::Imm => "立即数".to_string(),
            Shape::Mem => "内存".to_string(),
        }
    }
}

/// 一条搬运指令。
#[derive(Debug, Clone)]
pub(crate) struct MoveArm {
    /// 声明的数据宽度（位）。
    pub width: u16,
    /// 变体名（`Inst::` 后面的标识）。
    pub vn: Ident,
    /// 目的槽字段 + 槽序号。
    pub dst: Ident,
    pub dst_idx: u8,
    /// 来源槽字段 + 槽序号。
    pub src: Ident,
    pub src_idx: u8,
    /// **额外的** Reg 输入槽（riscv `FSGNJ` 的第三槽）：填同一个源寄存器。
    pub extra: Vec<(Ident, u8)>,
    pub dst_shape: Shape,
    pub src_shape: Shape,
    /// 指令名（诊断）。
    pub inst: String,
}

/// 一次搬运请求。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Want {
    pub dst: Shape,
    pub src: Shape,
}

impl Want {
    fn show(&self) -> String {
        format!("{} ← {}", self.dst.show(), self.src.show())
    }
}

/// 搬运族全表（由 `data_width` 派生）。
pub(crate) struct MoveTable {
    arms: Vec<MoveArm>,
}

impl MoveTable {
    /// 收集**声明了 `data_width`** 的指令。形状不构成纯搬运 ⇒ 生成期报错
    /// （写了 `data_width` 却不生效是最难查的一类错，宁可拒绝）。
    pub(crate) fn collect(infos: &[InstInfo]) -> Result<Self, String> {
        let mut arms = Vec::new();
        for info in infos {
            let Some(width) = info.inst.data_width else {
                continue;
            };
            arms.push(arm_of(info, width)?);
        }
        Ok(Self { arms })
    }

    /// 有没有能完成该形状的搬运（不看到宽度的**生成期门控**：没有就发 fail-closed 分支，
    /// 而不是在生成期报错——"这台机器做不了"要落在生成物里）。
    pub(crate) fn has(&self, want: &Want) -> bool {
        !self.candidates(want).is_empty()
    }

    /// 有没有能搬**这么宽**的搬运（生成期门控用：例如"本 ISA 有 128 位的整寄存器
    /// 搬移吗"决定按值向量走全宽槽还是按类型宽度）。内存形状要求精确宽度。
    pub(crate) fn covers(&self, want: &Want, width: u16) -> bool {
        self.pick(want, width).is_ok()
    }

    /// 候选（不排序、不过滤宽度）。
    fn candidates(&self, want: &Want) -> Vec<&MoveArm> {
        self.arms
            .iter()
            .filter(|a| a.dst_shape == want.dst && a.src_shape == want.src)
            .collect()
    }

    /// 生成期已知道宽度：取"最窄的覆盖者"（内存参与时要求精确）。
    pub(crate) fn pick(&self, want: &Want, width: u16) -> Result<&MoveArm, String> {
        let exact = matches!(want.dst, Shape::Mem) || matches!(want.src, Shape::Mem);
        let mut hits: Vec<&MoveArm> = self
            .candidates(want)
            .into_iter()
            .filter(|a| {
                if exact {
                    a.width == width
                } else {
                    a.width >= width
                }
            })
            .collect();
        hits.sort_by_key(|a| a.width);
        match hits.as_slice() {
            [] => Err(self.no_candidate(want, width, exact)),
            [one] => Ok(one),
            many => {
                let ties: Vec<&MoveArm> = many
                    .iter()
                    .copied()
                    .filter(|a| a.width == many[0].width)
                    .collect();
                if ties.len() > 1 {
                    Err(format!(
                        "「{}（{} 位）」有 {} 条候选（{}）——搬运派生要求唯一：请让其中一条不再声明 \
                         `data_width`（或改宽度）",
                        want.show(),
                        many[0].width,
                        ties.len(),
                        ties.iter()
                            .map(|a| a.inst.as_str())
                            .collect::<Vec<_>>()
                            .join(" / ")
                    ))
                } else {
                    Ok(many[0])
                }
            }
        }
    }

    /// 无候选时的诊断（内存操作数要求精确宽度，单独说清）。
    fn no_candidate(&self, want: &Want, width: u16, exact: bool) -> String {
        if exact {
            format!(
                "本 ISA 没有能完成「{}（{width} 位）」的搬运指令——内存操作数的宽度必须精确：\
                 请给对应指令写 `data_width = {width}`",
                want.show()
            )
        } else {
            format!(
                "本 ISA 没有能完成「{}（{width} 位）」的搬运指令——请在指令上写 \
                 `data_width = {width}`（搬运族的唯一人写数据；方向/寄存器族/立即数由操作数结构派生）",
                want.show()
            )
        }
    }

    /// 宽度是**运行期表达式**（值的位宽来自 IR 类型）：发射"最窄覆盖"的 if-链，
    /// 全不覆盖 ⇒ fail-closed。`build` 为该臂生成语句体（含 `push_inst`/绑寄存器）。
    pub(crate) fn dispatch<F>(
        &self,
        want: &Want,
        bits: &TokenStream2,
        build: F,
    ) -> Result<TokenStream2, String>
    where
        F: Fn(&MoveArm) -> TokenStream2,
    {
        let exact = matches!(want.dst, Shape::Mem) || matches!(want.src, Shape::Mem);
        let mut hits = self.candidates(want);
        hits.sort_by_key(|a| a.width);
        if hits.is_empty() {
            return Err(self.no_candidate(want, 0, exact));
        }
        // 同一形状 + 同一宽度有多条 ⇒ **生成期报错并列出候选**（与 `pick` 同一条纪律：
        // 歧义是作者该消歧的事，不设"钉选"注解——那就是第二套机制）。
        for pair in hits.windows(2) {
            if pair[0].width == pair[1].width {
                let ties: Vec<&MoveArm> = hits
                    .iter()
                    .copied()
                    .filter(|a| a.width == pair[0].width)
                    .collect();
                return Err(format!(
                    "「{}（{} 位）」有 {} 条候选（{}）——搬运派生要求唯一：请让其中一条不再声明 \
                     `data_width`（或改宽度）",
                    want.show(),
                    pair[0].width,
                    ties.len(),
                    ties.iter()
                        .map(|a| a.inst.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ")
                ));
            }
        }
        let msg = format!(
            "ISA-DSL: 搬运「{}」的宽度本 ISA 未申报（该宽度没有声明 `data_width` 的指令{}）",
            want.show(),
            if exact {
                "，内存操作数要求精确宽度"
            } else {
                ""
            }
        );
        let unsupported = quote! {
            return Err(crate::prelude::IrError::Unsupported(#msg.into()));
        };
        // 升序链：先命中最窄的覆盖者（内存参与时是精确相等）。
        let mut chain: Option<TokenStream2> = None;
        for a in hits.iter().rev() {
            let w = a.width as u32;
            let body = build(a);
            let cond = if exact {
                quote! { (#bits as u32) == #w }
            } else {
                quote! { (#bits as u32) <= #w }
            };
            chain = Some(match chain {
                Some(tail) => quote! { if #cond { #body } else { #tail } },
                None => quote! { if #cond { #body } else { #unsupported } },
            });
        }
        Ok(chain.unwrap_or(unsupported))
    }

    /// **能力视图**（ABI 侧词汇）：把派生结果折算成 `forge-abi::Capability` 的
    /// 能力名 + 该能力的最大位宽 + 提供它的指令名。ISA **不再申报**这些名字，
    /// 视图只是翻译给引擎看（`abi_view` 的静态体检与生成物的 `role_bits` 同源）。
    ///
    /// `fpr_mov` / `vec_mov` 的分界 = **按值向量的 ABI 阈值 128 位**（16 字节）：
    /// 更窄 = 标量浮点搬移，达到整寄存器宽 = 按值向量搬移（x86 的 `VEC(16)`）。
    pub(crate) fn capabilities(&self) -> Vec<(&'static str, u16, Vec<String>)> {
        let mut out: BTreeMap<&'static str, (u16, Vec<String>)> = BTreeMap::new();
        for a in &self.arms {
            let name =
                match (a.dst_shape, a.src_shape) {
                    (Shape::Reg(Bank::Gpr), Shape::Reg(Bank::Gpr)) => "gpr_mov",
                    (Shape::Reg(Bank::Gpr), Shape::Imm) => "gpr_mov_imm",
                    (Shape::Reg(Bank::Gpr), Shape::Reg(_)) => "fpr_to_gpr_mov",
                    (Shape::Reg(_), Shape::Reg(Bank::Gpr)) => "gpr_to_fpr_mov",
                    (Shape::Reg(_), Shape::Reg(_)) | (Shape::Reg(_), Shape::Imm) => {
                        if a.width >= 128 { "vec_mov" } else { "fpr_mov" }
                    }
                    (Shape::Reg(_), Shape::Mem) | (Shape::Mem, Shape::Reg(_)) => "wide_vec_move",
                    _ => continue,
                };
            let e = out.entry(name).or_insert((0, Vec::new()));
            e.0 = e.0.max(a.width);
            e.1.push(a.inst.clone());
        }
        out.into_iter()
            .map(|(name, (bits, mut insts))| {
                insts.sort();
                insts.dedup();
                (name, bits, insts)
            })
            .collect()
    }
}

/// 从指令的操作数结构派生一条搬运臂。
fn arm_of(info: &InstInfo, width: u16) -> Result<MoveArm, String> {
    let bad = |why: &str| {
        Err(format!(
            "[[instructions]] \"{}\": `data_width = {width}` 只能写在**纯搬运**指令上（{}）——\
             方向/寄存器族/立即数由操作数结构派生，所以结构必须干净：一个目的槽 + 一个来源槽\
             （Reg 输入槽可以多一个，riscv `fsgnj` 那种自动填同一个源）",
            info.inst.name, why
        ))
    };
    // 索引一律是 **Reg 字段序号**（= `MachineInst::reg_field(i)` 的域：生成物里
    // `__reg_slot` 在**只含 Reg 字段**的数组上取值），不是操作数绝对序号。
    let mut dst: Option<(Ident, u8, Shape)> = None;
    let mut regs: Vec<(Ident, u8, Bank)> = Vec::new();
    let mut imm: Option<(Ident, u8)> = None;
    let mut mem: Option<(Ident, u8)> = None;
    let mut reg_seen = 0u8;
    for (_field, fid, slot, role) in info.operands.iter() {
        let kinds_ok = matches!(
            slot.kind,
            OperandKind::Reg | OperandKind::Imm | OperandKind::Mem
        );
        if !kinds_ok {
            return bad("有寄存器/立即数/内存之外的槽（标签、条件码…）");
        }
        // 目的 = 那个 `out`/`inout` 槽（**寄存器或内存**：store 的内存槽就是目的）。
        if matches!(role, OperandRole::Out | OperandRole::InOut) {
            if dst.is_some() {
                return bad("有多个目的槽");
            }
            match slot.kind {
                OperandKind::Reg => {
                    let Some(bank) = bank_of(slot) else {
                        return bad("寄存器槽没有确定的寄存器族（`class`/`classes` 跨族或缺失）");
                    };
                    dst = Some((fid.clone(), reg_seen, Shape::Reg(bank)));
                    reg_seen += 1;
                }
                OperandKind::Mem => dst = Some((fid.clone(), 0, Shape::Mem)),
                OperandKind::Imm => return bad("目的槽是立即数（立即数不能是搬移的目的）"),
                _ => unreachable!("上面已排除"),
            }
            continue;
        }
        match slot.kind {
            OperandKind::Reg => {
                let Some(bank) = bank_of(slot) else {
                    return bad("寄存器槽没有确定的寄存器族（`class`/`classes` 跨族或缺失）");
                };
                regs.push((fid.clone(), reg_seen, bank));
                reg_seen += 1;
            }
            OperandKind::Imm => {
                if imm.is_some() {
                    return bad("有多个立即数槽");
                }
                imm = Some((fid.clone(), 0));
            }
            OperandKind::Mem => {
                if mem.is_some() {
                    return bad("有多个内存槽");
                }
                mem = Some((fid.clone(), 0));
            }
            _ => unreachable!("上面已排除"),
        }
    }
    let (dst_fid, dst_idx, dst_shape) = dst.ok_or_else(|| {
        format!(
            "[[instructions]] \"{}\": `data_width` 需要恰好一个目的槽（`out`/`inout`），一条都没有",
            info.inst.name
        )
    })?;
    // 来源：寄存器 > 立即数 > 内存；三者互斥（混用说明这条指令不是纯搬运）。
    let kinds = [!regs.is_empty(), imm.is_some(), mem.is_some()]
        .iter()
        .filter(|b| **b)
        .count();
    if kinds == 0 {
        return bad("没有来源槽（`in`）");
    }
    if kinds > 1 {
        return bad("来源槽不止一类（寄存器/立即数/内存混用）");
    }
    if matches!(dst_shape, Shape::Mem) && regs.is_empty() {
        return bad("目的槽是内存，但来源不是寄存器（内存→内存之类）");
    }
    let (src_fid, src_idx, src_shape); // 声明后立即赋值，避免未初始化误用
    let mut extra = Vec::new();
    if !regs.is_empty() {
        let (f, i, b) = regs[0].clone();
        src_fid = f;
        src_idx = i;
        src_shape = Shape::Reg(b);
        for (f, i, _) in regs.iter().skip(1) {
            extra.push((f.clone(), *i));
        }
    } else if let Some((f, i)) = imm {
        src_fid = f;
        src_idx = i;
        src_shape = Shape::Imm;
    } else if let Some((f, i)) = mem {
        src_fid = f;
        src_idx = i;
        src_shape = Shape::Mem;
    } else {
        unreachable!("kinds 已保证恰好一类来源");
    }
    Ok(MoveArm {
        width,
        vn: info.vn.clone(),
        dst: dst_fid,
        dst_idx,
        src: src_fid,
        src_idx,
        extra,
        dst_shape,
        src_shape,
        inst: info.inst.name.clone(),
    })
}

/// 槽的寄存器族（跨族 ⇒ None：这样一条指令不能算搬运）。
fn bank_of(slot: &crate::dsl::model::OperandSlot) -> Option<Bank> {
    let cs = slot.classes()?;
    let mut it = cs.iter().map(|c| Bank::of(*c));
    let first = it.next()?;
    if it.all(|b| b == first) {
        Some(first)
    } else {
        None
    }
}
