//! [`AbiPlan`] — 引擎产物：**调用方与被调方共用的纯数据描述**。
//!
//! 管线不再"自己知道约定"：它按 plan 发射调用点、入口、序/尾声（A3–A4）。
//! plan 是确定性的，`to_text()` 用作**快照**（防漂移），也是 `forge-isa abi dump` 的输出。

use serde::{Deserialize, Serialize};

use crate::rules::{CalleePop, VaListKind};
use crate::ty::TyView;

/// 物理寄存器引用（带类与名字，便于诊断与快照可读）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegRef {
    pub index: u32,
    pub class: String,
    pub name: String,
}

impl RegRef {
    pub fn new(index: u32, class: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            index,
            class: class.into(),
            name: name.into(),
        }
    }
}

/// 实参/返回值的符号扩展约定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Extension {
    /// 原样（宽度已足够）。
    #[default]
    None,
    /// 高位清零（无符号/指针）。
    ZeroExt,
    /// 高位按符号填充。
    SignExt,
}

/// 槽的用途（LLVM 的 `sret`/`inreg`/… 对位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    #[default]
    Normal,
    /// 间接结果指针（`sret`）。
    StructReturn,
    /// 语言运行时上下文（Swift `self`/Go context/闭包 env）。
    Context,
    /// 变参元信息（SysV `%al`、RISC-V `LEN`）。
    VariadicMeta,
    /// 按引用传递的结构体副本指针（LLVM `byval`：指针指向**调用方**栈上的副本）。
    ByVal,
}

/// 一个参数的落点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// 单寄存器（含扩展与用途）。
    Reg {
        reg: RegRef,
        #[serde(default)]
        ext: Extension,
        #[serde(default)]
        purpose: Purpose,
    },
    /// 两个寄存器（`(int,int)`、`(float,float)`、`(int,float)` 拆分）。
    RegPair { lo: RegRef, hi: RegRef },
    /// **三个以上**连续寄存器（AAPCS64 的 4×f32 HFA → s0-s3；RVV 的 HVA）。
    ///
    /// 不并进 `RegPair` 是因为"两个"是拆分语义（`(int,float)` 各取一池），而这里是
    /// "同一个池里的 N 个连续槽"。管线按成员逐个搬运；顺序即成员顺序。
    RegGroup { regs: Vec<RegRef> },
    /// 栈上（**被调方**视角：相对 `[abi.stack].frame_base` 的字节偏移，已含
    /// `first_offset_slots`；调用方视角见 `StackLayout::caller_offset`）。
    Stack { offset: i32, size: u16, align: u16 },
    /// 间接：`ptr` = 指针所在寄存器（`None` = 指针本身在栈上，`at` 给偏移）；
    /// `on_stack` 为真表示调用方在**自己的栈上**放了副本（byval）。
    ///
    /// `at` 的坐标系是**调用方的 byval 临时区**（`StackLayout::byval_area_bytes`），
    /// 不是传出参数区：副本是调用方的临时量，跟"第几个栈参数"没有关系。
    Indirect {
        ptr: Option<RegRef>,
        at: Option<i32>,
        on_stack: bool,
    },
    /// 不传递。
    Ignore,
}

/// 一个实参/形参的完整落点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgLoc {
    /// 形参名（或 `<hidden:sret>` / `<hidden:context>` / `<hidden:va_meta>`）。
    pub what: String,
    /// 参数序号（0 基；hidden 槽用 `None`）。
    pub index: Option<usize>,
    pub size: u32,
    pub place: Placement,
}

/// 返回值落点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetLoc {
    /// 寄存器（单个或一对）。
    Reg {
        reg: RegRef,
    },
    RegPair {
        lo: RegRef,
        hi: RegRef,
    },
    /// 通过隐藏指针写回（`sret`）；指针本身在 [`HiddenSlots::sret`]。
    Indirect {
        size: u32,
        align: u16,
    },
    /// 无返回值。
    Void,
}

/// 栈布局（调用点强制要求 + 被叫方取参基准）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackLayout {
    /// 调用点栈对齐。
    pub align: u32,
    /// 槽单位。
    pub slot_bytes: u32,
    /// 调用方预留的 shadow space。
    pub shadow_bytes: u32,
    /// 被叫方视角：第一个栈参数相对 frame_base 的字节偏移（`first_offset_slots × slot_bytes`）。
    pub first_arg_offset: i32,
    /// 参数区总字节（含 shadow；调用方要求的最小 OUTGOING 区）。
    pub arg_area_bytes: u32,
    /// 调用方需要预留的 **byval 临时区**字节数（`Placement::Indirect` 里
    /// `at` 的坐标系；0 = 本签名没有按引用传的实参）。
    ///
    /// 与 `arg_area_bytes` 分开记：副本是**调用方帧内**的临时量（放在传出参数区**下方**），
    /// 混进参数区会让后面的栈参数与副本抢同一段内存。
    pub byval_area_bytes: u32,
    /// 红区（调用方可用的 sp 以下字节数；`None` = 无）。
    pub red_zone: Option<u32>,
    /// 帧填充（x86 = 8）。
    pub frame_padding: i32,
}

impl StackLayout {
    /// 调用方视角：第 k 个栈槽的字节偏移（从 `sp` 起，已含 shadow）。
    pub fn caller_offset(&self, slot_index: u32) -> i32 {
        self.shadow_bytes as i32 + (slot_index * self.slot_bytes) as i32
    }
}

/// callee-saved 的保存机制（"怎么做"由 ISA 能力决定，plan 只表达"用哪种"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CalleeSaveMechanism {
    /// 完全没有（全 caller-saved）——被调方破坏即可。
    #[default]
    None,
    /// 用硬件 push/pop（x86）。
    Push,
    /// 存到帧内固定槽（riscv/arm64 的 fp-inside 顶部）。
    StoreToFrame,
}

/// callee-saved 计划。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CalleeSavedPlan {
    pub mechanism: CalleeSaveMechanism,
    /// 按保存顺序。
    pub regs: Vec<RegRef>,
    /// 帧指针是否也被保存（x86 prologue 的 `push rbp`）。
    pub includes_fp: bool,
    /// 链接寄存器是否也被保存（riscv/arm64 的 ra/lr）。
    pub includes_link: bool,
}

/// 隐藏槽（不占用户可见参数序号）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HiddenSlots {
    pub sret: Option<RegRef>,
    pub context: Option<RegRef>,
    pub va_meta: Option<RegRef>,
    pub va_len: Option<RegRef>,
}

/// 变参区域（`va_list` 的内存形态）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaArea {
    pub kind: VaListKind,
    pub size: u32,
    pub align: u32,
    /// 未命名实参是否**只能走栈**（Win64/AAPCS64/RISC-V 为真）。
    pub stack_only: bool,
}

/// **一份调用计划的完整描述**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbiPlan {
    /// 生效的约定名。
    pub conv: String,
    /// 签名是否变参。
    pub variadic: bool,
    /// 用户可见实参（含被拆成两槽的聚合，仍是一条 `ArgLoc`）。
    pub args: Vec<ArgLoc>,
    pub ret: RetLoc,
    pub stack: StackLayout,
    pub callee_saved: CalleeSavedPlan,
    pub hidden: HiddenSlots,
    /// 调用点被破坏的寄存器（callee-saved 之外的全部可用池 = 调用方需假设被破坏）。
    pub clobbers: Vec<RegRef>,
    /// 被叫方弹栈字节数（`CalleePop` 展开后的值）。
    pub callee_pop_bytes: u32,
    pub va_area: Option<VaArea>,
    /// 形参/实参至少要扩展到多少位（AArch64 = 32；`None` = 无要求）。
    pub widen_to_bits: Option<u16>,
    /// 人类可读备注（出处/已知偏差）。
    pub note: Option<String>,
}

impl AbiPlan {
    /// 确定性文本渲染（快照/CLI 用；**同一 plan 必须逐字节稳定**）。
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("conv {}\n", self.conv));
        out.push_str(&format!(
            "stack align={} slot={} shadow={} first_arg_off={} arg_area={} byval_area={} red_zone={:?} frame_pad={}\n",
            self.stack.align,
            self.stack.slot_bytes,
            self.stack.shadow_bytes,
            self.stack.first_arg_offset,
            self.stack.arg_area_bytes,
            self.stack.byval_area_bytes,
            self.stack.red_zone,
            self.stack.frame_padding
        ));
        out.push_str(&format!("variadic {}\n", self.variadic));
        for a in &self.args {
            out.push_str(&format!(
                "arg {} size={} -> {}\n",
                a.what,
                a.size,
                place_text(&a.place)
            ));
        }
        out.push_str(&format!("ret {}\n", ret_text(&self.ret)));
        out.push_str(&format!(
            "hidden sret={} context={} va_meta={} va_len={}\n",
            opt_reg(&self.hidden.sret),
            opt_reg(&self.hidden.context),
            opt_reg(&self.hidden.va_meta),
            opt_reg(&self.hidden.va_len)
        ));
        out.push_str(&format!(
            "callee_saved {:?} [{}] fp={} link={}\n",
            self.callee_saved.mechanism,
            self.callee_saved
                .regs
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
                .join(" "),
            self.callee_saved.includes_fp,
            self.callee_saved.includes_link
        ));
        out.push_str(&format!(
            "clobbers [{}]\n",
            self.clobbers
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
                .join(" ")
        ));
        if self.callee_pop_bytes > 0 {
            out.push_str(&format!("callee_pop {}\n", self.callee_pop_bytes));
        }
        if let Some(va) = &self.va_area {
            out.push_str(&format!(
                "va_area {:?} size={} align={} stack_only={}\n",
                va.kind, va.size, va.align, va.stack_only
            ));
        }
        if let Some(bits) = self.widen_to_bits {
            out.push_str(&format!("widen_to_bits {bits}\n"));
        }
        out
    }

    /// 展开 callee-pop（`None`/`SumStackArgs`/`Fixed`）。
    pub(crate) fn resolve_callee_pop(pop: CalleePop, stack_args_bytes: u32) -> u32 {
        match pop {
            CalleePop::None => 0,
            CalleePop::SumStackArgs => stack_args_bytes,
            CalleePop::Fixed(n) => n,
        }
    }
}

fn place_text(p: &Placement) -> String {
    match p {
        Placement::Reg { reg, ext, purpose } => {
            let mut s = format!("reg {}", reg.name);
            if *ext != Extension::None {
                s.push_str(&format!(" ext={ext:?}"));
            }
            if *purpose != Purpose::Normal {
                s.push_str(&format!(" purpose={purpose:?}"));
            }
            s
        }
        Placement::RegPair { lo, hi } => format!("pair {}:{}", lo.name, hi.name),
        Placement::RegGroup { regs } => format!(
            "group [{}]",
            regs.iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Placement::Stack {
            offset,
            size,
            align,
        } => format!("stack off={offset} size={size} align={align}"),
        Placement::Indirect { ptr, at, on_stack } => format!(
            "indirect ptr={} at={at:?} on_stack={on_stack}",
            opt_reg(ptr)
        ),
        Placement::Ignore => "ignore".into(),
    }
}

fn ret_text(r: &RetLoc) -> String {
    match r {
        RetLoc::Reg { reg } => format!("reg {}", reg.name),
        RetLoc::RegPair { lo, hi } => format!("pair {}:{}", lo.name, hi.name),
        RetLoc::Indirect { size, align } => format!("indirect size={size} align={align}"),
        RetLoc::Void => "void".into(),
    }
}

fn opt_reg(r: &Option<RegRef>) -> String {
    r.as_ref()
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "-".into())
}

impl Default for StackLayout {
    fn default() -> Self {
        Self {
            align: 16,
            slot_bytes: 8,
            shadow_bytes: 0,
            first_arg_offset: 0,
            arg_area_bytes: 0,
            byval_area_bytes: 0,
            red_zone: None,
            frame_padding: 0,
        }
    }
}

/// 便捷构造：`TyView` 的显示名（快照可读性）。
pub fn ty_text(ty: &TyView) -> String {
    match &ty.kind {
        crate::ty::TyKind::Int => format!("i{}", ty.size * 8),
        crate::ty::TyKind::Ptr => "ptr".into(),
        crate::ty::TyKind::Float => format!("f{}", ty.size * 8),
        crate::ty::TyKind::Vector { lanes, .. } => format!("v{lanes}"),
        crate::ty::TyKind::Aggregate { members } => {
            format!("agg({})", members.len())
        }
        crate::ty::TyKind::Other => format!("other{}", ty.size),
    }
}
