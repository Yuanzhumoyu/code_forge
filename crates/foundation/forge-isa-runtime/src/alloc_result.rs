//! AllocResult — 寄存器分配的完整输出。
//!
//! 替代旧 `AllocResult`，作为分配器与管线后续阶段之间的接口。
//! VCode 在分配后不变，所有分配信息在此 struct 中。

use forge_ir::*;

/// **一次溢出编辑**（v23 M5 的 `SpillEdit`）：分配器对"这条指令前后要把值搬进/搬出栈"
/// 给出的**显式决定**，发射侧退化为"按序执行编辑"。
///
/// 为什么要有它：编辑以前是**发射期临时算出来的**（`emit_inst_with_spills` 里现查
/// `spill_slots`/`assignments`、现挑 scratch、现判 `settable`）——决策与执行混在一处，
/// 分配器真正的逐点状态（哪个值此刻在栈上）反而用不上（发射侧只能看**最终**表）。
/// 决策回到分配器之后：① 发射侧只剩"装值 / 改写字段 / 写回槽"三种动作；
/// ② 驱逐策略想升级为"驱逐活跃值"时，只要多产出一条 `Store` 即可（现状只驱逐死值）。
///
/// 顺序约定：同一条指令的编辑里，`Reload` 在前、`Bind` 居中、`Store` 在后——
/// 发射侧据此分三段执行（装值 → 改写字段 → 发指令 → 写回）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpillEdit {
    /// 指令**前**把槽里的值装进 scratch（宽度档由 `width` 选，指令族由 `class` 选）。
    Reload {
        slot: SpillSlot,
        scratch: PReg,
        class: RegClass,
        width: u16,
    },
    /// 把指令的某个寄存器字段改写成 scratch（该字段此前指向被溢出的 vreg）。
    Bind {
        field: u8,
        scratch: PReg,
        class: RegClass,
    },
    /// 指令**后**把 scratch 的值写回槽（spilled def）。
    Store {
        slot: SpillSlot,
        scratch: PReg,
        class: RegClass,
        width: u16,
    },
}

/// 寄存器分配的完整输出。
///
/// 管线后续阶段（encoder, frame lowering, spill emit）只读取此结构，
/// 不修改 VCode。这是唯一的分配结果类型；旧的 `AllocResult` 已在 v20 删除。
#[derive(Clone, Debug)]
pub struct AllocResult {
    /// **密集**的 XReg → PReg 分配映射（下标 = `XReg::index()`；`None` = 未分配到寄存器）。
    ///
    /// v23 M5（输出密集化）：原来是 `HashMap<XReg, PReg>`，而发射/序言**每条指令每个
    /// 操作数**都要查一次（热路径）；改成按下标直取的 `Vec` 后查询退化为一次边界检查。
    /// 语义不变——读侧一律走 [`AllocResult::preg`]/[`AllocResult::has_preg`]。
    pub assignments: Vec<Option<PReg>>,
    /// **本次调用的中性布局**（v20 A3b-2b）：由管线从 `AbiPlan` 转好塞进来。
    ///
    /// 序言/尾声（`TargetFrameLowering::emit_prologue/epilogue`）只拿到本结构体、
    /// 拿不到 `LowerCtx`，所以"保存谁 / 栈区多大 / 参数落在哪"必须挂在这里才能读到。
    /// `None` = 管线没接上（或该 ISA/约定算不出计划）⇒ 走既有 `[abi]` 路径。
    pub call_layout: Option<forge_abi_model::AbiPlan>,
    /// **密集**的 XReg → 溢出槽映射（下标 = `XReg::index()`；`None` = 没溢出）。
    /// 同 `assignments`：热路径查询，走 [`AllocResult::spill_slot`]/[`AllocResult::is_spilled`]。
    pub spill_slots: Vec<Option<SpillSlot>>,
    /// **寄存器压力峰值**：分配过程中"同时占用物理寄存器"的值数上限（v27 M6-91，P4）。
    ///
    /// 判读：接近/超过可分配寄存器数 ⇒ 必然有值被溢出（配合 `SPILL-VALUE`/`SPILL-EDIT`
    /// 事件定位"是谁、搬到哪"）。它是**观测值**，不参与分配决策。
    pub peak_live: usize,
    /// **逐指令的溢出编辑序列**（下标 = 指令在 vcode 里的全局序号；空 = 该指令不需要
    /// 任何溢出搬运）。由分配器产出、发射侧按序执行——见 [`SpillEdit`]。
    pub edits: Vec<Vec<SpillEdit>>,
    /// 函数参数 XReg 列表（用于 prologue 中从 ABI 寄存器复制参数）。
    ///
    /// v23 M5（消平行数组）：原先与它**平行**的四个数组（`param_is_float`/`param_by_ref`/
    /// `param_is_32`）已删除——那三个都是**从 vreg 自身可推**的事实
    /// （`v.class().is_fp()`、`is_fp() && v.bytes() > 16`、`v.bytes() == 4`），
    /// 生成物现在直接算；其中 `param_is_32` 连读点都没有（纯死数组）。
    pub param_vregs: Vec<XReg>,
    /// 是否 sret 返回（宽向量 >16 字节）——首 int 槽被 sret 指针占用，
    /// @move_args 收参 __gi 从 1 起（Windows x64 隐藏 sret 参数约定）。
    pub sret: bool,
    /// 栈参数区字节数（shadow space + 第 5+ 参数槽，见 LowerCtx.
    /// max_stack_arg_bytes）：move_args 收栈参数时计算 spill 槽地址
    ///（sp_base = -(frame) - callee_saved + stack_arg_bytes）需用它。
    pub stack_arg_bytes: u32,
    /// **寄存器保存区**在帧内的偏移（相对帧基址；`None` = 本函数没有，v20 变参 V3）。
    /// 序言按 `call_layout.va.save` 的槽表把参数寄存器存到 `[帧基址 + 本偏移 + 槽偏移]`。
    pub va_save_off: Option<i64>,
    /// **保存区占用的帧顶字节数**（v20 V7）：形状要求"保存区与栈实参连续"时 = 保存区字节数，
    /// 保存区放在 `[入口 sp - va_top, 入口 sp)`，帧里其余东西（ra/fp、callee-saved）整体下移
    /// 这一个量——生成物按它给自己的存取偏移减掉；0 = 历史布局（逐字节不变）。
    pub va_top: u32,
    /// 每个参数的 **IR 类型字节数**（与 param_vregs 对齐）——由 `CompileState`
    /// 在分配后按值描述填充。用途：`@move_args` 的 by-ref 宽向量收参
    /// 必须按**真实字节宽**分派 32B/64B load 变体——寄存器类宽对 >128 位向量
    /// 恒为 `VEC(32)`（`reg_class_for`），无法区分 V256(32B)/V512(64B)，旧实现
    /// 因此让 64B 分支永不可达 → V512 只 load 32B（lane8..15 丢失，WA-37 D5）。
    /// 0 = 未知（保守按 32B 处理）。**这一条不可从 vreg 推**（IR 类型不在 vreg 里），
    /// 因此保留。
    pub param_bytes: Vec<u16>,
    /// 需要在序言中保存的 callee-saved 物理寄存器（按 push 顺序）
    pub callee_saved_to_save: Vec<PReg>,
    /// 帧布局信息
    pub frame_info: FrameInfo,
}

/// 栈帧上的溢出槽。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpillSlot {
    /// 相对帧基准（FP 或 SP）的字节偏移（负数表示低于帧指针）
    pub offset: i32,
    /// 槽大小（字节）— 宽度感知，不再固定 8 字节
    pub size: u8,
}

/// 帧布局信息（由分配器计算，frame lowering 阶段可使用）。
#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    /// 溢出区域总大小（已对齐，字节）
    pub spill_area_size: u32,
    /// 单条指令最多需要的同时 scratch 寄存器数
    /// 用于 frame lowering 验证 scratch 池足够
    pub max_concurrent_spills: usize,
}

impl Default for AllocResult {
    fn default() -> Self {
        Self {
            call_layout: None,
            assignments: Vec::new(),
            spill_slots: Vec::new(),
            peak_live: 0,
            edits: Vec::new(),
            param_vregs: Vec::new(),
            sret: false,
            stack_arg_bytes: 0,
            va_save_off: None,
            va_top: 0,
            param_bytes: Vec::new(),
            callee_saved_to_save: Vec::new(),
            frame_info: FrameInfo {
                spill_area_size: 0,
                max_concurrent_spills: 0,
            },
        }
    }
}

impl AllocResult {
    // ── 查询方法 ──

    /// 查询 XReg 分配的物理寄存器。
    #[inline]
    pub fn preg(&self, vreg: XReg) -> Option<PReg> {
        self.assignments
            .get(vreg.index() as usize)
            .copied()
            .flatten()
    }

    /// XReg 是否**分配到了**寄存器（`preg` 的布尔形式；生成的序言用它替代
    /// `assignments.contains_key(&pv)`）。
    #[inline]
    pub fn has_preg(&self, vreg: XReg) -> bool {
        self.preg(vreg).is_some()
    }

    /// XReg 是否被溢出到栈上。
    #[inline]
    pub fn is_spilled(&self, vreg: XReg) -> bool {
        self.spill_slots
            .get(vreg.index() as usize)
            .is_some_and(|s| s.is_some())
    }

    /// 获取 XReg 的溢出槽信息。
    #[inline]
    pub fn spill_slot(&self, vreg: XReg) -> SpillSlot {
        self.spill_slots
            .get(vreg.index() as usize)
            .copied()
            .flatten()
            .unwrap_or_else(|| {
                panic!(
                    "vreg {} 没有溢出槽——调用方必须先判 is_spilled（v23 M5 密集表：\
                     越界与空槽都是编程错误）",
                    vreg.index()
                )
            })
    }
    // ── 构造方法 ──

    // ── 核心 API ──

    /// 创建空 AllocResult。
    pub fn new() -> Self {
        Self::default()
    }

    /// 解析 XReg → PReg，已溢出/未分配时返回 IrError。
    /// 这是 DSL 生成的 emit_inst 代码使用的核心 API。
    pub fn resolve(&self, vreg: XReg) -> Result<PReg, IrError> {
        self.preg(vreg).ok_or_else(|| {
            if self.is_spilled(vreg) {
                IrError::RegAlloc(format!(
                    "vreg {} is spilled to stack (offset {}), not in a register",
                    vreg,
                    self.spill_slot(vreg).offset
                ))
            } else {
                IrError::RegAlloc(format!("unallocated vreg {}", vreg))
            }
        })
    }

    /// 插入 VReg → PReg 映射（密集表按需长大）。
    pub fn insert(&mut self, vreg: XReg, preg: PReg) {
        let i = vreg.index() as usize;
        grow_to(&mut self.assignments, i);
        grow_to(&mut self.spill_slots, i);
        self.assignments[i] = Some(preg);
    }

    /// 插入 VReg → 溢出槽（同样把**两张**表一起长大到同长：长度不一致会让越界查询
    /// 把"没有该状态"误读成另一种状态）。
    pub fn insert_spill(&mut self, vreg: XReg, slot: SpillSlot) {
        let i = vreg.index() as usize;
        grow_to(&mut self.assignments, i);
        grow_to(&mut self.spill_slots, i);
        self.spill_slots[i] = Some(slot);
    }
}

/// 把密集表长大到至少能放下下标 `i`（`None` = 该 vreg 没有对应状态）。
fn grow_to<T: Clone>(v: &mut Vec<Option<T>>, i: usize) {
    if v.len() <= i {
        v.resize(i + 1, None);
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_preg(idx: u32, class: RegClass) -> PReg {
        PReg::new(idx, class)
    }

    /// 构造第 n 号 GPR 临时寄存器（测试辅助：经 XRegAllocator 受控创建）。
    fn xgpr(n: u32) -> XReg {
        XReg::new(n, RegClass::GPR64)
    }

    /// 构造第 n 号 FPR 临时寄存器。
    fn xfpr(n: u32) -> XReg {
        let mut xa = XRegAllocator::new();
        for _ in 0..n {
            xa.alloc_default(RegClass::FPR64);
        }
        xa.alloc_default(RegClass::FPR64)
    }

    #[test]
    fn test_is_spilled_and_spill_slot() {
        let mut result = AllocResult::new();
        let v = xgpr(5);
        assert!(!result.is_spilled(v));

        result.insert_spill(
            v,
            SpillSlot {
                offset: -16,
                size: 8,
            },
        );
        assert!(result.is_spilled(v));
        assert_eq!(result.spill_slot(v).offset, -16);
        assert_eq!(result.spill_slot(v).size, 8);
    }

    #[test]
    fn test_resolve_success() {
        let mut result = AllocResult::new();
        let p = make_preg(5, RegClass::FPR64);
        result.insert(xfpr(3), p);
        assert_eq!(result.resolve(xfpr(3)).unwrap(), p);
    }

    #[test]
    fn test_resolve_spilled_error() {
        let mut result = AllocResult::new();
        result.insert_spill(
            xgpr(4),
            SpillSlot {
                offset: -8,
                size: 4,
            },
        );
        let err = result.resolve(xgpr(4)).unwrap_err();
        assert!(format!("{}", err).contains("spilled"));
    }

    #[test]
    fn test_resolve_unallocated_error() {
        let result = AllocResult::new();
        let err = result.resolve(xgpr(99)).unwrap_err();
        assert!(format!("{}", err).contains("unallocated"));
    }

    #[test]
    fn test_default() {
        let r = AllocResult::default();
        assert!(r.assignments.is_empty());
        assert_eq!(r.frame_info.spill_area_size, 0);
        assert_eq!(r.frame_info.max_concurrent_spills, 0);
    }
}
