//! [[pattern]] 树型多指令匹配（S6）的共享运行期类型 + 纯求值。
//!
//! DSL 谱（`isa/*.toml`）里 `[[pattern]]` 声明一棵 IR **匹配树**
//! （`match = "Fadd(Fmul(a, b), c)"`）与一段发射序列（`insts`）。编译期
//! （forge-dsl）把每棵模式树编译成 [`PatternSpec`] 静态数据（树节点 = 单元
//! Opcode，叶 = 变量，变量按树 DFS 序编号 0..），生成进 arch 模块的
//! `TargetLowering::patterns()` 表；运行期 lowering 驱动（pipeline/lowering.rs
//! 的 per-block 逆序预扫）按此数据做结构匹配（单 use 内部节点、同块、整棵
//! 子树 consumed），命中后把叶变量绑定为 emitter 的 `{N}` 输入走 `lower_pattern`。
//!
//! 本模块**只放纯数据 + 无副作用求值**——不接触 DataFlowGraph。涉及 IR 值类型
//! 的根属性推导（镜像生成器的 `__attr` 闭包）在驱动侧完成（见
//! pipeline/lowering.rs 的根属性助手），只复用这里的标量映射/宽度工具。

use forge_ir::{Opcode, TypeContext, TypeId};

// ════════════════════════════════════════════════════════════════════
// `when` 谓词（对根指令的派生属性求值）
// ════════════════════════════════════════════════════════════════════

/// 单个属性的比较运算符（TOML `eq`/`ne`/`lt`/`le`/`gt`/`ge`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatCmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl PatCmp {
    pub fn op(self, a: i64, b: i64) -> bool {
        match self {
            PatCmp::Eq => a == b,
            PatCmp::Ne => a != b,
            PatCmp::Lt => a < b,
            PatCmp::Le => a <= b,
            PatCmp::Gt => a > b,
            PatCmp::Ge => a >= b,
        }
    }
}

/// 编译后的 `when` 谓词——`&'static` 引用使其可放进生成模块的 const 静态数据。
///
/// 求值上下文 = 属性名 → i64 值（`rd`/`rs1_width`/`elem`/`imm0`…，与 lowering
/// 规则同一组 PRED_ATTRS 键）。未知属性/缺失 → 比较为 false（保守，与
/// codegen 里 `compile_pred_guard` 的 `map_or(false, …)` 语义一致）。
#[derive(Debug, Clone, Copy)]
pub enum PatPred {
    /// `attr 与 常量` 比较。
    Cmp {
        cmp: PatCmp,
        attr: &'static str,
        v: i64,
    },
    /// `attr ∈ [v1, v2, …]`。
    In {
        attr: &'static str,
        vs: &'static [i64],
    },
    /// and / or / not。
    All(&'static [PatPred]),
    Any(&'static [PatPred]),
    Not(&'static PatPred),
}

impl PatPred {
    /// 比较子（叶子）个数——特异性排序用（多者优先）。
    pub fn leaf_count(&self) -> usize {
        match self {
            PatPred::Cmp { .. } | PatPred::In { .. } => 1,
            PatPred::All(ps) | PatPred::Any(ps) => ps.iter().map(Self::leaf_count).sum(),
            PatPred::Not(p) => p.leaf_count(),
        }
    }
}

/// 求值编译后的 `when` 谓词。`attr` 提供属性名 → 值（无该属性 → None）。
///
/// 用 `&dyn Fn`（trait 对象）而非泛型 `impl Fn`：递归时若按泛型逐层包闭包，
/// 每层都生成新闭包类型，模式深度 N 就嵌套 N 层单态化（深树会击穿 rustc
/// 递归上限）。trait 对象没有单态化，递归只走一层间接调用。
pub fn eval_pat_pred(pred: &PatPred, attr: &dyn Fn(&str) -> Option<i64>) -> bool {
    match pred {
        PatPred::Cmp { cmp, attr: name, v } => attr(name).is_some_and(|g| cmp.op(g, *v)),
        PatPred::In { attr: name, vs } => attr(name).is_some_and(|g| vs.contains(&g)),
        PatPred::All(ps) => ps.iter().all(|p| eval_pat_pred(p, attr)),
        PatPred::Any(ps) => ps.iter().any(|p| eval_pat_pred(p, attr)),
        PatPred::Not(p) => !eval_pat_pred(p, attr),
    }
}

// ════════════════════════════════════════════════════════════════════
// 匹配树 + 模式规格
// ════════════════════════════════════════════════════════════════════

/// 匹配树的一个子树：IR op 节点，或绑定 IR 值的叶变量。
#[derive(Debug, Clone, Copy)]
pub enum PatTerm {
    /// IR op 节点：该位置的值必须由**同块、单 use** 的同 opcode 指令定义
    ///（驱动在预扫时校验）。`args` 长度 = 该 op 的实参个数。
    Op {
        op: Opcode,
        args: &'static [PatTerm],
    },
    /// 叶变量：把该位置的 IR 值绑定为 emitter 的 `{N}` 输入。
    /// `u8` = 变量的树 DFS 序（从 0 起连续）——codegen 把模板 `{名字}` 改写为
    /// `{N}` 后，与运行期 `lower_pattern(name, leaves, …)` 收到的叶序一致。
    Var(u8),
}

/// 一棵编译完成的模式。
#[derive(Debug, Clone, Copy)]
pub struct PatternSpec {
    /// 模式唯一名（生成 `lower_pattern` 分发的匹配键）。
    pub name: &'static str,
    /// 根指令的单元 Opcode。
    pub op: Opcode,
    /// 根 op 的操作数子树（长度 = 根 IR op 的实参个数）。
    pub args: &'static [PatTerm],
    /// 根指令派生属性上的 `when`（None = 恒真）。
    pub when: Option<&'static PatPred>,
    /// 树中出现的变量个数。
    pub var_count: u8,
    /// Op 节点总数（root + 内部）——同根多形状模式按此降序优先（大者先试）。
    pub nodes: u16,
    /// when 叶子数——同形状模式按此降序优先（特异性）。
    pub guard_leaves: u8,
}

// ════════════════════════════════════════════════════════════════════
// 根属性派生用的小工具（镜像生成器 gen_lowering_attrs 的 __attr 语义）
// ════════════════════════════════════════════════════════════════════

/// 标量类型的 `elem` 数值映射——镜像生成进 arch 模块的 `elem_id_of`
/// （F32=1/F64=2/I32=3/I64=4/I8=5/I16=6；其余 0）。向量元素类型由
/// `attr_elem` 经 `TypeContext::element_type` 取到标量后再映射。
pub fn scalar_elem_id(t: TypeId) -> i64 {
    match t {
        TypeId::F32 => 1,
        TypeId::F64 => 2,
        TypeId::I32 => 3,
        TypeId::I64 => 4,
        TypeId::I8 => 5,
        TypeId::I16 => 6,
        _ => 0,
    }
}

/// `rd`/`rs1_width`/`rs2_width` 的宽度（位）：向量 = `size_bytes × 8`，
/// 标量 = `TypeId::bits()`。与生成器 `__a_rd/__a_rs1/__a_rs2` 一致。
pub fn type_width_bits(t: TypeId, type_ctx: Option<&TypeContext>) -> i64 {
    if type_ctx.is_some_and(|tc| tc.is_vector(t)) {
        (type_ctx.map(|tc| tc.size_bytes(t)).unwrap_or(0) * 8) as i64
    } else {
        t.bits() as i64
    }
}

/// `elem` 属性：向量 → 元素类型 id；标量 → 自身 id。与生成器 `__a_elem` 一致。
pub fn type_elem_id(t: TypeId, type_ctx: Option<&TypeContext>) -> i64 {
    if type_ctx.is_some_and(|tc| tc.is_vector(t)) {
        type_ctx
            .and_then(|tc| tc.element_type(t))
            .map(scalar_elem_id)
            .unwrap_or(0)
    } else {
        scalar_elem_id(t)
    }
}

/// `rd_vec`/`rs1_vec` 向量字节数标记：类型是向量 → Some(size_bytes)，否则 None。
pub fn type_vec_bytes(t: TypeId, type_ctx: Option<&TypeContext>) -> Option<i64> {
    type_ctx.and_then(|tc| {
        if tc.is_vector(t) {
            Some(tc.size_bytes(t) as i64)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_elem_mapping_matches_generated_elem_id_of() {
        // 与 forge-dsl codegen/lowering.rs 生成的 elem_id_of 逐项对齐
        assert_eq!(scalar_elem_id(TypeId::F32), 1);
        assert_eq!(scalar_elem_id(TypeId::F64), 2);
        assert_eq!(scalar_elem_id(TypeId::I32), 3);
        assert_eq!(scalar_elem_id(TypeId::I64), 4);
        assert_eq!(scalar_elem_id(TypeId::I8), 5);
        assert_eq!(scalar_elem_id(TypeId::I16), 6);
        assert_eq!(scalar_elem_id(TypeId::BOOL), 0);
        assert_eq!(scalar_elem_id(TypeId::VOID), 0);
    }

    /// `PatPred` 携带 `&'static` 引用（生成静态数据），测试用 static 承载。
    static P_EQ1: PatPred = PatPred::Cmp {
        cmp: PatCmp::Eq,
        attr: "elem",
        v: 1,
    };
    static P_RD64: PatPred = PatPred::Cmp {
        cmp: PatCmp::Eq,
        attr: "rd",
        v: 64,
    };
    static P_IN_ELEM13: PatPred = PatPred::In {
        attr: "elem",
        vs: &[1, 3],
    };
    static P_NOT_RD: PatPred = PatPred::Not(&P_RD64);

    #[test]
    fn eval_pred_and_or_not_in() {
        let attr = |k: &str| -> Option<i64> {
            match k {
                "elem" => Some(1),
                _ => None,
            }
        };
        // eq
        assert!(eval_pat_pred(&P_EQ1, &attr));
        // 缺失属性 → false（与 compile_pred_guard 的 map_or(false) 一致）
        assert!(!eval_pat_pred(&P_RD64, &attr));
        // not 反转（缺失属性的比较为 false → not 为 true）
        assert!(eval_pat_pred(&P_NOT_RD, &attr));
        // in
        assert!(eval_pat_pred(&P_IN_ELEM13, &attr));
        // all / any
        static ALL: PatPred = PatPred::All(&[P_EQ1, P_IN_ELEM13]);
        assert!(eval_pat_pred(&ALL, &attr));
        static ANY: PatPred = PatPred::Any(&[P_RD64, P_EQ1]);
        assert!(eval_pat_pred(&ANY, &attr));
    }
}
