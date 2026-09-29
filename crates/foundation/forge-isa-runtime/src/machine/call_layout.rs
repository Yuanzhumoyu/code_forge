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
    /// **≥3 个返回寄存器**（v20 A6 的多值返回）：第 k 个返回值在 `regs[k]`。
    Group {
        regs: Vec<(RegClass, u32)>,
    },
    /// 通过隐藏指针写回（指针见 [`CallLayout::hidden_sret`]）。
    Indirect {
        size: u32,
        align: u16,
    },
}

impl RetPlace {
    /// 该落点占用的寄存器（按返回值顺序；诊断/核对用）。
    pub fn regs(&self) -> Vec<(RegClass, u32)> {
        match self {
            RetPlace::Reg { class, index, .. } => vec![(*class, *index)],
            RetPlace::Pair { lo, hi } => vec![*lo, *hi],
            RetPlace::Group { regs } => regs.clone(),
            RetPlace::Void | RetPlace::Indirect { .. } => Vec::new(),
        }
    }
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
    /// 调用点被破坏的寄存器（**约定事实**：可用池 − callee-saved；调用方需假设被破坏）。
    ///
    /// v20 A5-3 ④：发射侧据此设置 `LowerCtx::current_clobbers`。它与
    /// [`Self::callee_saved`] 互补——同一台机器换约定（x86 的 win64/sysv64），
    /// 破坏集**必须跟着换**（win64 的 RDI/RSI 是 callee-saved，sysv64 是 caller-saved）。
    pub clobbers: Vec<(RegClass, u32)>,
    /// 帧填充字节（x86 = 8 = align/2；其余约定 0）——帧布局的输入之一。
    pub frame_padding: i32,
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

/// **实参/返回值的形状**（v20 A5-3「调用点 plan」的输入）。
///
/// 为什么需要它：调用点在 lowering 时只看得见**实参的形状**（大小/对齐/族/成员），看不见
/// 被调方的 `Function`；而被调方按什么收参只有引擎算得出来。于是调用点把形状喂给宿主的
/// planner（`forge-abi` 的 `Signature` + `AbiRegistry::plan`），拿回**被调方**的
/// [`CallLayout`]，再按 `ArgPlace` 逐参数搬值——谱里因此不必再写 `[abi.arg_class]`。
///
/// 运行时 crate 不依赖 `forge-abi`，所以这份形状是**中性 POD**；把形状摊成
/// `forge_abi::TyView` 的工作由宿主（`forge-codegen`）做。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgShape {
    /// 字节大小。
    pub size: u32,
    /// 字节对齐。
    pub align: u32,
    pub kind: ShapeKind,
}

/// 形状的族（与 `forge_abi::TyKind` 同义，但**不带 forge-abi 依赖**）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeKind {
    /// 整数/布尔/字符等标量。
    Int,
    /// 指针/引用。
    Ptr,
    /// 浮点标量。
    Float,
    /// 向量（`elem_is_float` 区分 `<4 x f32>` 与 `<4 x i32>`）。
    Vector {
        elem_is_float: bool,
        lanes: u32,
        elem_bytes: u32,
    },
    /// 聚合：**成员逐个列出**（HFA/HVA 判定与"按成员拆寄存器"都要靠它——成员为浮点标量
    /// 且同宽同族时才是 HFA）。
    Aggregate { members: Vec<ArgShape> },
    /// 未建模的标量（按字节大小当整数处理）。
    Other,
}

impl ArgShape {
    /// 整数标量形状（调用点从 IR 类型投影时的常用构造）。
    pub fn int(size: u32, align: u32) -> Self {
        Self {
            size,
            align,
            kind: ShapeKind::Int,
        }
    }

    /// 浮点标量形状。
    pub fn float(size: u32) -> Self {
        Self {
            size,
            align: size,
            kind: ShapeKind::Float,
        }
    }

    /// 指针形状。
    pub fn ptr(size: u32) -> Self {
        Self {
            size,
            align: size,
            kind: ShapeKind::Ptr,
        }
    }

    /// **IR 类型 → 中性形状**（调用点投影，v20 A6）。
    ///
    /// 为什么要**递归摊开成员/lane**，而不是只报 size/align：判定"这个聚合是不是 HFA/HVA"
    /// 靠的是**成员**（AAPCS64 的 `<2 x f64>` 或 `{f32,f32,f32,f32}` 走浮点池），只报
    /// size/align 会让**调用点**算出的落点与**被调方**（函数级 plan，成员信息齐全）分叉
    /// ——一个说 `X0:X1`、一个说 `V0:V1`，实参直接搬错寄存器。
    ///
    /// 深度上限（4 层）与成员展开上限（16 个）是**防御性**的：递归类型与超大数组不需要
    /// 逐成员信息（那种大小必然走栈/间接），摊不开就退化成"没有成员"，由规则兜底。
    pub fn from_ir_type(store: &crate::ir::ir::types::TypeStore, ty: crate::ir::TypeId) -> Self {
        Self::from_ir_type_at(store, ty, 0)
    }

    fn from_ir_type_at(
        store: &crate::ir::ir::types::TypeStore,
        ty: crate::ir::TypeId,
        depth: u32,
    ) -> Self {
        use crate::ir::ir::types::TypeEntry;
        const MAX_DEPTH: u32 = 4;
        const MAX_MEMBERS: u64 = 16;

        let size = store.size_bytes(ty).max(1);
        let align = store.alignment(ty).max(1);
        let member = |t: crate::ir::TypeId| Self::from_ir_type_at(store, t, depth + 1);
        let agg = |members: Vec<ArgShape>| ArgShape {
            size,
            align,
            kind: ShapeKind::Aggregate { members },
        };
        match store.get(ty) {
            TypeEntry::Float { .. } | TypeEntry::BFloat { .. } => ArgShape::float(size),
            TypeEntry::Pointer { .. } | TypeEntry::Function { .. } => ArgShape::ptr(size),
            TypeEntry::Int { .. } => ArgShape::int(size, align),
            TypeEntry::Vector { elem, len } => ArgShape {
                size,
                align,
                kind: ShapeKind::Vector {
                    elem_is_float: store.is_float(*elem),
                    lanes: *len,
                    elem_bytes: store.size_bytes(*elem).max(1),
                },
            },
            TypeEntry::Array { elem, len } => {
                if *len <= MAX_MEMBERS && depth < MAX_DEPTH {
                    agg((0..*len).map(|_| member(*elem)).collect())
                } else {
                    agg(Vec::new())
                }
            }
            TypeEntry::Struct { fields, .. } => {
                if depth < MAX_DEPTH {
                    agg(fields.iter().map(|f| member(f.ty)).collect())
                } else {
                    agg(Vec::new())
                }
            }
            // 可扩展向量（SVE/RVV）与其它：精确形状运行时才知道 ⇒ 交给规则兜底（`Other`）。
            _ => ArgShape {
                size,
                align,
                kind: ShapeKind::Other,
            },
        }
    }
}
