//! 编译阶段（v30 M1）。
//!
//! M2 起本枚举是**阶段契约的唯一来源**：契约文档的阶段表、`COMPILE-DONE` 的
//! 计时字段、以及 `CompileError` 的失败定位都从它派生。M1 先把它引入，
//! 让错误类型有稳定的阶段标签。

/// 一次编译失败发生在哪个阶段。
///
/// `Module` 是**整模块编排**（线程 panic、产物装配）——它不是管线阶段，
/// 不出现在契约文档的阶段表与 [`Stage::ALL`] 里。
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// 合法化（含 IR 级优化与前置能力门）。
    Legalize,
    /// 指令选择（lowering）。
    ISel,
    /// 寄存器分配。
    RegAlloc,
    /// 帧布局。
    Frame,
    /// 发射。
    Emit,
    /// JIT 装载。
    Jit,
    /// 整模块编排（不属于管线阶段）。
    Module,
}

impl Stage {
    /// 管线阶段（契约文档阶段表的集合；不含 [`Stage::Module`]）。
    pub const ALL: [Stage; 6] = [
        Stage::Legalize,
        Stage::ISel,
        Stage::RegAlloc,
        Stage::Frame,
        Stage::Emit,
        Stage::Jit,
    ];

    /// 契约名（与 `docs/reference/backend-pipeline.md` 的阶段表一致）。
    pub const fn id(self) -> &'static str {
        match self {
            Stage::Legalize => "legalize",
            Stage::ISel => "isel",
            Stage::RegAlloc => "regalloc",
            Stage::Frame => "frame",
            Stage::Emit => "emit",
            Stage::Jit => "jit",
            Stage::Module => "module",
        }
    }

    /// 一句话说明（契约文档阶段表的"做什么"列；`Module` 不属于管线阶段）。
    pub const fn description(self) -> &'static str {
        match self {
            Stage::Legalize => "合法化（两相：计划相只读算计划 → 物化相按指令序单遍改写）",
            Stage::ISel => "指令选择（lowering：生成物的 `lower_inst` + 宿主的辅助发射）",
            Stage::RegAlloc => "寄存器分配（活跃区间 + 溢出与回填）",
            Stage::Frame => "帧布局（槽位/对齐/帧尺寸/ABI 槽）",
            Stage::Emit => "发射（序言 / 指令 / 尾声 / fixup 解析）",
            Stage::Jit => "装载（JIT：可执行内存 + 符号解析）",
            Stage::Module => "整模块编排（不属于管线阶段）",
        }
    }
}
