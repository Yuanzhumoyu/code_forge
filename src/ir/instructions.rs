//! IR 指令和终止指令定义。

use super::debug_info::SourceLocation;
use super::types::*;

use smallvec::SmallVec;

/// IR 操作码。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opcode {
    // === 整数算术 ===
    Iadd,
    Isub,
    Imul,
    Udiv,
    Sdiv,
    Urem,
    Srem,

    // === 浮点算术 ===
    Fadd {
        flags: FastMathFlags,
    },
    Fsub {
        flags: FastMathFlags,
    },
    Fmul {
        flags: FastMathFlags,
    },
    Fdiv {
        flags: FastMathFlags,
    },
    /// 冻结值 — 阻止未定义行为传播（用于 MaybeUninit 等场景）。
    /// 优化 pass 不得跨 Freeze 做常量折叠或值推断。
    Freeze,
    /// 浮点取负 (IEEE 754 negate)：对符号位取反。
    /// 等价于 `-x`，但与 `fsub(-0.0, x)` 在 NaN 符号位处理上有区别。
    Fneg {
        flags: FastMathFlags,
    },
    /// 浮点绝对值。
    Fabs {
        flags: FastMathFlags,
    },
    Fsqrt {
        flags: FastMathFlags,
    },

    // === SIMD 向量算术 ===
    Vadd,
    Vsub,
    Vmul,
    /// 向量元素提取: result = vector[ lane ]
    Vextract {
        lane: u8,
    },
    /// 向量元素插入: result = vector with vector[lane] = value
    Vinsert {
        lane: u8,
    },
    /// 向量 Shuffle: result = shufflevector(v1, v2, mask)
    /// mask[i] 指定结果中第 i 个元素来自 v1 还是 v2。
    ShuffleVector {
        mask: [u8; 16],
    },

    // === 原子操作 ===
    /// 原子 Read-Modify-Write: `result = atomic_rmw(op, ptr, val, ordering)`
    /// operands: [ptr, val]，结果类型与 ptr 指向的类型相同。
    AtomicRmw {
        op: AtomicRmwOp,
        ordering: Ordering,
    },
    /// 原子 Compare-and-Exchange: `result = cmpxchg(ptr, cmp, new, ordering)`
    /// operands: [ptr, cmp, new]，返回 {old_value, success_flag} 的结构体。
    Cmpxchg {
        ordering: Ordering,
    },
    /// 内存屏障 (Fence): 阻止内存访问跨此屏障重排。
    /// 无操作数，无结果。
    Fence {
        ordering: Ordering,
    },

    // === 复合类型操作 ===
    /// 从聚合体值中提取字段: result = extractvalue(aggregate, index)
    /// operands: [aggregate]，index 指定要提取的字段索引。
    ExtractValue {
        index: u32,
    },
    /// 向聚合体中插入字段: result = insertvalue(aggregate, value, index)
    /// operands: [aggregate, value]，result 为新聚合体值。
    InsertValue {
        index: u32,
    },

    // === 位运算 ===
    Band,
    Bor,
    Bxor,
    Bnot,
    Ishl,
    Ushr,
    Sshr,

    // === 比较 ===
    Icmp {
        cond: IntCC,
    },
    Fcmp {
        cond: FloatCC,
        flags: FastMathFlags,
    },

    // === 内存 ===
    Load,
    Store,
    StackLoad {
        offset: i32,
    },
    StackStore {
        offset: i32,
    },

    // === 常量 ===
    Iconst {
        index: u32,
    },
    Fconst {
        index: u32,
    },

    // === 类型转换 ===
    Sextend,
    Uextend,
    Ireduce,
    Bitcast,

    // === 调用 ===
    Call {
        func: FuncRef,
    },
    CallIndirect,

    // === 地址 ===
    StackAddr {
        offset: i32,
    },
    GlobalAddr {
        /// 全局变量在 Module::globals 中的索引。
        global: u32,
    },

    // === 其他 ===
    Copy,
    Phi {
        /// Phi 节点的传入值配对：每个操作数与其来源基本块。
        /// 符合 SSA 的 phi 语义：`result = phi [val1, block1], [val2, block2], ...`
        incoming: smallvec::SmallVec<[(Value, BlockId); 4]>,
    },
    Select, // 条件选择: result = cond ? a : b
    Nop,    // 空操作（对齐/占位）

    // === 栈分配 ===
    /// 栈上分配。操作数：无。
    /// 结果类型：指向分配类型的指针。
    /// 由 `ty` 字段指定分配元素的类型，`count` 字段指定元素数量。
    Alloca {
        /// 分配的元素数量（默认 1）。
        count: u32,
    },

    // === 地址计算 ===
    /// GetElementPtr — 计算复合类型中嵌套元素的地址。
    /// 操作数：operands[0] = 基地址指针, operands[1..] = 各层索引（整数 Value）。
    /// 结果类型通过 Instruction.ty 设置（指向元素类型的指针）。
    /// `indexed_type` 是被索引的复合类型，用于类型推导。
    GetElementPtr {
        /// 被索引的复合类型（结构体/数组/向量）。
        indexed_ty: Type,
    },
}

/// IR 指令。
#[derive(Clone, Debug)]
pub struct Instruction {
    /// 指令产出的 SSA 值（纯指令无返回值时为 None）。
    pub result: Option<Value>,
    /// 操作码。
    pub opcode: Opcode,
    /// 操作数。
    pub operands: SmallVec<[Value; 4]>,
    /// 结果类型（用于类型检查）。
    pub ty: Type,
    /// 源码位置（可选，用于调试信息和错误报告）。
    pub source_location: Option<SourceLocation>,
}

impl Instruction {
    pub fn new(
        opcode: Opcode,
        operands: SmallVec<[Value; 4]>,
        result: Option<Value>,
        ty: Type,
    ) -> Self {
        Self {
            result,
            opcode,
            operands,
            ty,
            source_location: None,
        }
    }

    /// 设置此指令的源码位置（builder 模式）。
    pub fn with_location(mut self, loc: SourceLocation) -> Self {
        self.source_location = Some(loc);
        self
    }
}

/// 基本块终止指令。
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Terminator {
    /// 条件分支：if cond { goto true_block } else { goto false_block }
    Branch {
        cond: Value,
        true_block: BlockId,
        false_block: BlockId,
        true_args: SmallVec<[Value; 2]>,
        false_args: SmallVec<[Value; 2]>,
    },
    /// 无条件跳转。
    Jump {
        target: BlockId,
        args: SmallVec<[Value; 2]>,
    },
    /// 函数返回。
    Return { values: SmallVec<[Value; 2]> },
    /// 不可达（用于未完成的构建或死代码）。
    Unreachable,
    /// 多路分支（switch / jump table）。
    ///
    /// 根据 `discriminant` 的值跳转到对应的目标块。
    /// `targets` 提供了 case 值到目标块的映射。
    /// `default_block` 是未匹配时的默认目标。
    Switch {
        /// 分支判别值（整数）。
        discriminant: Value,
        /// 默认目标块。
        default_block: BlockId,
        /// case 值 → (目标块, 块参数) 的映射。
        cases: smallvec::SmallVec<[(i64, BlockId, smallvec::SmallVec<[Value; 2]>); 8]>,
    },
}
