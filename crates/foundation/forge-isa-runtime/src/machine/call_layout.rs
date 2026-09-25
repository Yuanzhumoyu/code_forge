//! **调用布局**（中性数据）：一次调用的"实参放哪、返回怎么回来、栈区多大、保存谁"。
//!
//! ## 为什么要有这一层（v20 A3b-2 的地基）
//!
//! 历史上这些事实**分成三处各写一遍**：谱的 `[abi]` 节（寄存器池/栈参数偏移/callee-saved）、
//! 生成物里的 `move_args`/序尾声模板（怎么搬）、管线的 `param_reg_count`/`by_ref_limit`
//! （怎么算）。三处靠人工对齐，于是"换了 ISA 就错"（sret 永远取首 int 槽这类）。
//!
//! v20 把它们收敛到 `forge-abi` 的 `AbiPlan`（唯一事实源）。但运行时 crate **不依赖
//! forge-abi**（这是刻意的分层），所以需要这一层**中性镜像**：
//!
//! ```text
//! forge-abi::AbiPlan ──(forge-codegen 转换)──► machine::call_layout::CallLayout
//!                                                     │
//!                       生成物（move_args/收参/序尾声）与管线都读它
//! ```
//!
//! 三条约定：
//!
//! 1. 寄存器用 **(类, 类内号)** 表示——`RegClass` 是 forge-ir 的中性类型，生成物用
//!    `Reg::from_index(i, class)` 还原成自己的物理寄存器；**不用**"A/B 空间号"这种
//!    依赖分组顺序的编号。
//! 2. `Stack` 的偏移是**被调方视角**（相对帧基址，已含首个栈参数的槽数）；给调用方看的
//!    "第 k 个栈槽"用 [`CallLayout::caller_offset`]。
//! 3. `byval` 副本走**独立的临时区**（[`CallLayout::byval_area_bytes`]），坐标从 0 起——
//!    它不是"第几个栈参数"。

use forge_ir::RegClass;

/// 符号扩展要求（与 `AbiPlan` 里的 `Extension` 同义，运行时侧不带 forge-abi 依赖）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ext {
    #[default]
    None,
    /// 高位清零。
    Zero,
    /// 高位按符号填充。
    Sign,
}

/// 一个实参/形参的落点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgPlace {
    /// 单寄存器。
    Reg {
        class: RegClass,
        index: u32,
        ext: Ext,
        /// 声明了 `sret`：这个寄存器里是**返回缓冲指针**（约定的 hidden 槽）。
        sret: bool,
    },
    /// 两个寄存器（`(int,int)` / `(float,float)` / `(int,float)`）。
    Pair {
        lo: (RegClass, u32),
        hi: (RegClass, u32),
    },
    /// ≥3 个连续寄存器（AAPCS64 的 4×f32 HFA…）。顺序即成员顺序。
    Group { regs: Vec<(RegClass, u32)> },
    /// 栈上（**被调方**视角，相对帧基址的字节偏移）。
    Stack { offset: i32, size: u16, align: u16 },
    /// 间接：指针在 `reg`（`None` = 指针本身在栈上，`at` 给按引用临时区偏移）。
    Indirect {
        reg: Option<(RegClass, u32)>,
        at: Option<i32>,
        /// 调用方在**自己的栈上**放了副本（`byval`）。
        on_stack: bool,
    },
    /// 不传递。
    Ignore,
}

impl ArgPlace {
    /// 该落点占用的寄存器（诊断/核对用）。
    pub fn regs(&self) -> Vec<(RegClass, u32)> {
        match self {
            ArgPlace::Reg { class, index, .. } => vec![(*class, *index)],
            ArgPlace::Pair { lo, hi } => vec![*lo, *hi],
            ArgPlace::Group { regs } => regs.clone(),
            ArgPlace::Indirect { reg: Some(r), .. } => vec![*r],
            _ => Vec::new(),
        }
    }
}

/// 返回值落点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetPlace {
    Void,
    Reg {
        class: RegClass,
        index: u32,
        ext: Ext,
    },
    Pair {
        lo: (RegClass, u32),
        hi: (RegClass, u32),
    },
    /// 通过隐藏指针写回（指针见 [`CallLayout::hidden_sret`]）。
    Indirect {
        size: u32,
        align: u16,
    },
}

/// 一个实参/形参的完整描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallArg {
    /// 参数序号（`None` = 引擎产生的隐藏槽）。
    pub index: Option<u32>,
    pub size: u32,
    pub place: ArgPlace,
}

/// **一次调用的布局**（调用方与被调方共用）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CallLayout {
    /// 生效的约定名（与 `forge-abi` 的 `AbiRules::name` 一致）。
    pub conv: String,
    pub variadic: bool,
    pub args: Vec<CallArg>,
    pub ret: Option<RetPlace>,
    /// 隐藏的间接结果指针寄存器（`sret`）。
    pub hidden_sret: Option<(RegClass, u32)>,
    /// 调用点栈对齐。
    pub stack_align: u32,
    /// 槽单位。
    pub slot_bytes: u32,
    /// 调用方要预留的 shadow space。
    pub shadow_bytes: u32,
    /// 第一个栈参数相对帧基址的字节偏移（被调方视角）。
    pub first_arg_offset: i32,
    /// 传出参数区总字节（含 shadow）。
    pub arg_area_bytes: u32,
    /// 调用方 byval 临时区字节数（坐标从 0 起）。
    pub byval_area_bytes: u32,
    /// 红区（`None` = 无）。
    pub red_zone: Option<u32>,
    /// 需要被调方保存的寄存器（按约定顺序）。
    pub callee_saved: Vec<(RegClass, u32)>,
    /// 被叫方在 `ret` 前自行弹掉的栈字节数（stdcall/thiscall）。
    pub callee_pop_bytes: u32,
    /// 形参/实参至少扩展到多少位（AArch64 = 32）。
    pub widen_to_bits: Option<u16>,
}

impl CallLayout {
    /// 调用方视角：第 k 个栈槽相对 `sp` 的偏移（已含 shadow）。
    pub fn caller_offset(&self, slot_index: u32) -> i32 {
        self.shadow_bytes as i32 + (slot_index * self.slot_bytes) as i32
    }

    /// 某个参数序号对应的落点。
    pub fn arg(&self, index: u32) -> Option<&CallArg> {
        self.args.iter().find(|a| a.index == Some(index))
    }
}
