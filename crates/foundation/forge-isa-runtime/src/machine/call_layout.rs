//! **`va_list` 的运行时外壳**（v23 M2 收敛后的全部内容）。
//!
//! ## 这一层现在只剩什么
//!
//! 调用布局的**唯一事实源**是 `forge-abi-model` 的 `AbiPlan`（运行时经
//! [`machine::abi_model`](crate::machine::abi_model) 再导出，生成物与管线**直接读它**）。
//! 此前这里还有一份手抄镜像——`CallLayout`/`CallArg`/`ArgPlace`/`RetPlace`/`Ext` 与
//! `VaArgRule`/`VaArgRules`/`VaField`/`VaSaveArea`/`VaSaveSlot`——外加 `forge-codegen`
//! 里一份三百行的翻译：同一事实两处表示，加一个 ABI 特性要改四处。v23 M2 把它们**全部
//! 删除**（`ArgPlace`/`RetPlace` 的消费端改用 `Placement`/`RetLoc` 的**访问器**：
//! "哪个寄存器 / 指针在哪 / 栈上什么偏移"，因此不必穷举枚举）。
//!
//! 留下的三样**不是**镜像，生成物与模型都不需要它们：
//!
//! - [`VaInfo`]：被调方怎么找到未命名实参（形状 + 字段布局 + 保存区 + 取参规则 + 初值）；
//! - [`VaInit`]/[`VaInitVal`]：`va_list` 对象的**字段初值**——它取决于"本函数已用掉几个
//!   参数寄存器 / 溢出区从哪开始"，只有**编译入口**算得出来（生成物是各约定通用的），
//!   所以由宿主（forge-codegen 的 `abi_target::va_init`）算好后放进 `LowerCtx::conv.va`。
//!
//! 变参的**类型面**（`VaArgRule`/`VaArgRules`/`VaField`/`VaSaveArea`/`VaSaveSlot`）仍是
//! 模型里的那一份，这里只做再导出，宿主侧路径不变。

pub use forge_abi_model::plan::{VaArgRule, VaArgRules, VaField, VaSaveArea as VaSave, VaSaveSlot};

/// **变参信息**（v20 变参 V2/V3）：被调方怎么找到未命名实参。
///
/// **形状是数据**：没有"每份约定一个变体"，只有形状名（诊断用）+ 字段布局 + 保存区 +
/// 取参规则——生成物/管线按数据算，不认识约定名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaInfo {
    /// 形状名（引用预置形状时；显式形状表 = `None`）——**只用于诊断**。
    pub shape: Option<String>,
    /// `va_list` 对象本身的字节数。
    pub size: u32,
    pub align: u32,
    /// 未命名实参是否只走栈（约定数据）。
    pub stack_only: bool,
    /// **`va_list` 对象的字段布局**（v20 V3）：物化对象、取值时按它算偏移。
    pub fields: Vec<VaField>,
    /// **寄存器保存区**（v20 V3）：`None` = 没有（未命名实参只在栈上）。
    pub save: Option<VaSave>,
    /// **保存区是否必须紧贴入口 `sp`、与栈上实参连续**（v20 V7；形状数据）。
    ///
    /// true ⇒ 帧布局把保存区放帧顶（`[entry_sp - save_size, entry_sp)`），其余整体下移一个
    /// `save_size`——"单一线性游标"才能从寄存器那一串继续走进栈上的实参。生成物按它取
    /// `AllocResult::va_top`（0 = 历史布局）。
    pub save_contiguous: bool,
    /// **`va_list` 对象的初值**（v20 V3）：物化对象要写进去的东西，由**宿主**按本函数的 plan
    /// 预先算好（生成物是各约定通用的，算不出"已用掉几个参数寄存器"）。
    pub init: Option<VaInit>,
    /// **取参规则**（v20 V6）：`va_arg` 怎么取下一个实参（按实参类分两条）。
    pub arg_rules: VaArgRules,
}

/// `va_list` 对象某个字段的**初值**（v20 V3，与 [`VaInfo::fields`] 同序）。
///
/// 三种来源覆盖了各 psABI 的 `va_start`：常量（偏移/计数）、**帧内地址**（溢出区、保存区、
/// 游标指针）、以及"保存区基址 + 静态偏移"（管线分配的那段，偏移只有编译入口知道）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaInitVal {
    /// 常量（按字段宽度截断后写）。
    Imm(u64),
    /// **帧基址 + 偏移**（`StackAddr`）：指向溢出区/栈上实参区。
    FrameOff(i64),
    /// **寄存器保存区基址 + 静态偏移**（管线在帧里分配的那段）：AAPCS64 的 `__gr_top`/`__vr_top`
    /// 就是"保存区顶端"⇒ 偏移 = 本类区域字节数。
    SaveOff(u32),
}

/// `va_list` 对象的初值（v20 V3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaInit {
    /// 每个字段的初值，**与 `VaInfo::fields` 同序**（缺 ⇒ fail-closed）。宿主按本函数的 plan
    /// 与取参规则（`VaInfo::arg_rules`）算：计数式游标 = 零点 ± 步长 × 已用槽数、地址式游标 =
    /// 未命名区起点、基准 = 保存区（+ 区域内偏移）。
    pub fields: Vec<VaInitVal>,
}
