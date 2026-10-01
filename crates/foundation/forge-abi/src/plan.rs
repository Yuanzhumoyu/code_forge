//! [`AbiPlan`] — 引擎产物：**调用方与被调方共用的纯数据描述**。
//!
//! 管线不再"自己知道约定"：它按 plan 发射调用点、入口、序/尾声（A3–A4）。
//! plan 是确定性的，`to_text()` 用作**快照**（防漂移），也是 `forge-isa abi dump` 的输出。

use serde::{Deserialize, Serialize};

use crate::rules::{CalleePop, PositionRule};
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
    /// **≥3 个返回寄存器**（v20 A6 的多值返回：`(i64, i64, i64)` 这类）。
    /// 顺序即返回值的顺序（第 k 个值在 `regs[k]`）。
    RegGroup {
        regs: Vec<RegRef>,
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

/// **声明属性**：前端在形参/返回值上写下的"按什么传"（LLVM 的 `byval`/`sret`/`inreg`/
/// `zeroext`/`signext`/`align`）。
///
/// 与 [`Extension`] 的分工：`DeclAttrs` 是**输入**（前端声明），`Extension` 是**产物**
/// （plan 里那条落点带的符号扩展要求）。引擎把前者折进分类与落点，后者出现在 `AbiPlan` 里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclAttrs {
    /// `byval(N)`：按值传递——调用方在**自己的栈上**做 N 字节副本、传指针（LLVM `byval`）。
    pub byval: Option<u32>,
    /// `sret`：这个指针是**返回缓冲**（间接结果指针），走约定声明的 hidden sret 槽。
    pub sret: bool,
    /// `inreg`：尽量走寄存器（分类说要走栈时也先试一次池）。
    pub inreg: bool,
    /// 高位清零（无符号/指针）。
    pub zeroext: bool,
    /// 高位按符号填充。
    pub signext: bool,
    /// 声明的对齐（`None` = 用类型自然对齐）。
    pub align: Option<u32>,
}

impl DeclAttrs {
    /// 没有任何声明属性（引擎走纯分类路径）。
    pub fn is_empty(&self) -> bool {
        self.byval.is_none()
            && !self.sret
            && !self.inreg
            && !self.zeroext
            && !self.signext
            && self.align.is_none()
    }

    /// 折成 plan 里的符号扩展要求（`signext` 优先于 `zeroext`，与 LLVM 一致）。
    pub fn extension(&self) -> Extension {
        if self.signext {
            Extension::SignExt
        } else if self.zeroext {
            Extension::ZeroExt
        } else {
            Extension::None
        }
    }

    /// 声明的对齐（`None`/`Some(0)` → `None`）。
    pub fn declared_align(&self) -> Option<u32> {
        self.align.filter(|a| *a > 0)
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
    /// **形状名**（引用预置形状时；显式形状表 = `None`）——**只用于诊断与快照**，
    /// 语义全在下面几项数据里（引擎与管线都不按名字分支）。
    pub shape: Option<String>,
    pub size: u32,
    pub align: u32,
    /// 未命名实参是否**只能走栈**（Win64/AAPCS64/RISC-V 为真）。
    pub stack_only: bool,
    /// **`va_list` 对象的字段布局**（v20 变参 V3）：由形状定的 psABI 事实——
    /// 物化对象（`va_start`）与取值（`va_arg`）时按这些偏移算。
    ///
    /// 例（sysv64）：`gp_offset@0:u32` / `fp_offset@4:u32` /
    /// `overflow_arg_area@8:ptr` / `reg_save_area@16:ptr`。
    pub fields: Vec<VaField>,
    /// **寄存器保存区**（v20 变参 V3）：形状声明了才有；槽表由绑定的寄存器池给（ISA 数据）。
    pub save: Option<VaSaveArea>,
    /// **取参规则**（v20 V6）：`va_arg` 怎么从对象里取一个实参——按**实参类**（整数/浮点）各一条。
    /// 引擎从形状数据解析出来（字段名→下标、上限/步长由保存区槽表推），管线只按它跑同一套算法。
    pub arg_rules: VaArgRules,
}

/// 一类实参（整数/浮点）的**取参规则**：从对象里取一个实参的全部信息。
///
/// 算法（管线里的唯一实现，见 `forge-codegen/src/pipeline/va_expand.rs`）：
///
/// ```text
/// cur    = load <字段宽> [ap + cursor.offset]          ; 游标
/// addr   = base ? load [ap + base.offset] + cur : cur  ; base 缺省 ⇒ 游标本身就是地址
/// in_reg = icmp <cc> cur, limit                        ; 未超上限 ⇒ 实参在保存区里
/// p      = select in_reg, addr, overflow               ; 否则取溢出区指针
/// val    = load/fload <ty> [p]
/// custom : cur += in_reg ? step : 0 ; overflow += in_reg ? 0 : overflow_step
/// ```
///
/// 四份内置约定都落在这套数据上：`sysv64` = 无符号偏移游标（`gp_offset`/`fp_offset`，上限 =
/// 本类保存区字节数，基址 = `reg_save_area`，溢出 = `overflow_arg_area`）；`win64` = 没有基址
/// 也没有溢出（游标就是"下一个实参槽的地址"）；`aapcs64` = **有符号**偏移游标
/// （`__gr_offs`/`__vr_offs` 从负值数到 0，基址 = `__gr_top`/`__vr_top`，溢出 = `__stack`）；
/// `lp64d` = 单指针栈游标 + 保存区。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaArgRule {
    /// 游标字段在 `VaArea::fields` 里的下标。
    pub cursor: usize,
    /// 偏移式游标的基准字段（地址 = `base` + 游标）；`None` = 游标本身就是要取的地址。
    pub base: Option<usize>,
    /// 溢出/栈区字段；`None` = 没有溢出支（游标就是地址）。
    pub overflow: Option<usize>,
    /// 上限：游标（未超上限 ⇒ 实参在保存区里）与之比较。
    pub limit: u64,
    /// 上限比较是否**按有符号**看（aapcs64 的 `__gr_offs`/`__vr_offs` 是负数计数）。
    pub signed_limit: bool,
    /// 游标步长（字节）：实参在保存区里时推进多少。
    pub step: u32,
    /// 溢出指针步长（字节）：实参溢出到栈上时推进多少。
    pub overflow_step: u32,
    /// 无符号式游标的**区域起点**（字节）：初值 = 这里 + 步长 × 已用槽数
    /// （SysV 的浮点游标从 GP 区之后起算 ⇒ 48；整数类 = 0）。
    pub cursor_origin: u32,
    /// 游标是否"**从区域顶端往下数**"（AAPCS64 式）：初值 = −(步长 × 已用槽数)，
    /// 基准字段的值 = 本类保存区的**顶端**；否则（SysV/Win64 式）基准字段 = 保存区**起点**、
    /// 初值从 `cursor_origin` 正着数。
    pub cursor_counts_down: bool,
}

/// 取参规则：整数类与浮点类各一条（按实参类型选）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaArgRules {
    pub int: VaArgRule,
    pub float: VaArgRule,
}

/// `va_list` 对象里的一个字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaField {
    /// 字段名（psABI 里的叫法，只用于诊断与快照）。
    pub name: String,
    /// 相对对象起点的字节偏移。
    pub offset: u32,
    /// 字段字节数。
    pub size: u32,
}

/// 寄存器保存区（v20 变参 V3）：被调方在序言里把**参数寄存器**存进帧内的一段区，
/// `va_list` 的保存区指针字段指向它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaSaveArea {
    /// 区字节数（GP 块 + FP 块）。
    pub size: u32,
    pub align: u32,
    /// 槽：寄存器 + 区内偏移 + 字节数（GP 块在前、FP 块在后，各自按池序）。
    pub slots: Vec<VaSaveSlot>,
}

/// 保存区里的一个槽。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaSaveSlot {
    pub reg: RegRef,
    pub offset: u32,
    pub size: u32,
}

/// **一份调用计划的完整描述**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbiPlan {
    /// 生效的约定名。
    pub conv: String,
    /// 签名是否变参。
    pub variadic: bool,
    /// **寄存器位置的计数规则**（约定级事实，v20 A5-3）：`by_class` = int/float 各自独立
    /// 推进（riscv SysV），`by_position` = int/float 共用位置游标（Windows x64）。
    ///
    /// 放在 plan 里是因为调用方要按**被调方**的落点搬实参：这条规则决定"第 i 个实参落在
    /// 哪"，而它随约定而变。生成器把它镜像进 `CallLayout`，从此不必在谱里写 `arg_slot`。
    pub position: PositionRule,
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
        out.push_str(&format!(
            "variadic {} position {}\n",
            self.variadic,
            match self.position {
                PositionRule::ByClass => "by_class",
                PositionRule::ByPosition => "by_position",
            }
        ));
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
                "va_area {} size={} align={} stack_only={}\n",
                va.shape.as_deref().unwrap_or("<explicit>"),
                va.size,
                va.align,
                va.stack_only
            ));
            // 字段布局（v20 V3）：对象怎么物化由它定——逐字段列出（顺序即声明序）。
            for f in &va.fields {
                out.push_str(&format!("va_field {} @{} +{}\n", f.name, f.offset, f.size));
            }
            // 寄存器保存区：槽表是"序言要存谁、存到哪"的权威。
            if let Some(save) = &va.save {
                out.push_str(&format!(
                    "va_save size={} align={}\n",
                    save.size, save.align
                ));
                for s in &save.slots {
                    out.push_str(&format!(
                        "va_save_slot {} @{} +{}\n",
                        s.reg.name, s.offset, s.size
                    ));
                }
            }
            // 取参规则（v20 V6）：`va_arg` 的唯一算法按它跑（类 → 游标/基址/溢出/上限/步长），
            // 因此它也进黄金快照（改了约定数据就必须显式改快照，不会悄悄漂）。
            {
                let r = &va.arg_rules;
                let idx = |i: Option<usize>| i.map(|v| v.to_string()).unwrap_or_else(|| "-".into());
                for (cls, rule) in [("int", &r.int), ("float", &r.float)] {
                    out.push_str(&format!(
                        "va_rule {cls} cursor={} base={} overflow={} limit={} signed={} \
                         step={} ov_step={} origin={} counts_down={}\n",
                        rule.cursor,
                        idx(rule.base),
                        idx(rule.overflow),
                        rule.limit,
                        rule.signed_limit,
                        rule.step,
                        rule.overflow_step,
                        rule.cursor_origin,
                        rule.cursor_counts_down,
                    ));
                }
            }
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
        RetLoc::RegGroup { regs } => format!(
            "group {}",
            regs.iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
                .join(":")
        ),
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
