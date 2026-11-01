//! `liverange` 的**新增守卫**（从 `liverange.rs` 拆出：那里撞 700 行规模棘轮，
//! 见 `tests/file_size_ratchet.rs`）。原有单测仍在 `liverange.rs` 内。

use super::*;
use forge_ir::TypeId;

/// 与生产代码**同源**地取寄存器类：`no_hardcoded_widths` 守卫禁止宿主代码里写死
/// `RegClass::GPR(..)`，测试文件不在豁免之列，所以这里也走类型推导。
fn int_cls() -> RegClass {
    RegClass::from_type_id(TypeId::I64)
}

/// **`cover(point)` 必须顺序无关**：point 早于所有已有段时也要真的插入。
///
/// 真实调用点：stage-1.5 对每个形参 `cover(func_start)`（把区间起点提前到函数入口）。
/// 旧实现在 `last.end > point` 时无条件早退 ⇒ 那句话是空操作。
#[test]
fn test_cover_before_all_segments_inserts_in_order() {
    let cls = int_cls();
    let mut interval = LiveInterval::new(XReg::new(0, cls), cls);
    interval.cover(ProgPoint::new(0, 19));
    interval.cover(ProgPoint::new(0, 20));
    interval.cover(ProgPoint::new(0, 0)); // 早于所有段

    assert_eq!(
        interval.start_point(),
        ProgPoint::new(0, 0),
        "区间起点必须真的提前到 0：{:?}",
        interval.segments
    );
    assert!(interval.contains(ProgPoint::new(0, 0)));
    assert_eq!(interval.segments.len(), 2, "{:?}", interval.segments);
    assert!(
        interval.segments.windows(2).all(|w| w[0].end <= w[1].start),
        "段必须保持升序且互不重叠：{:?}",
        interval.segments
    );
}

/// **末指令是无条件跳转的块没有 fallthrough 后继**：只在下一块里活跃的值不得被
/// 扩展到本块（旧实现无条件加边，等于把所有"物理上相邻"的块都连起来）。
#[test]
fn test_unconditional_branch_has_no_fallthrough_edge() {
    /// 最小机器指令：只表达"分支去哪里、是否一定不落下"。
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct Jmp {
        uncond: bool,
        target: Option<Block>,
        binds: &'static [(u32, crate::machine::inst::OperandRole)],
    }
    impl crate::MachineInst for Jmp {
        fn operands(&self) -> smallvec::SmallVec<[crate::machine::inst::InstOperand; 4]> {
            smallvec::SmallVec::new()
        }
        fn is_branch(&self) -> bool {
            self.target.is_some()
        }
        fn branch_targets(&self) -> smallvec::SmallVec<[Block; 2]> {
            match self.target {
                Some(b) => smallvec::smallvec![b],
                None => smallvec::SmallVec::new(),
            }
        }
        fn is_unconditional_branch(&self) -> bool {
            self.uncond
        }
        fn is_call(&self) -> bool {
            false
        }
        fn is_ret(&self) -> bool {
            false
        }
    }
    use crate::machine::inst::OperandRole;

    // 三块：b0 无条件跳到 b2（**不**落到 b1）；v1 在 b1 被读、在 b2 被写。
    let mut vcode: crate::VCode<Jmp> = crate::VCode::new();
    let b0 = vcode.create_block(Block::new(0));
    vcode.create_block(Block::new(1));
    vcode.create_block(Block::new(2));
    vcode.switch_to_block(b0);
    vcode.push_inst(Jmp {
        uncond: true,
        target: Some(Block::new(2)),
        binds: &[],
    });
    let b1 = crate::VBlockId(1);
    vcode.switch_to_block(b1);
    vcode.push_inst(Jmp {
        uncond: false,
        target: None,
        binds: &[(1, OperandRole::Use)],
    });
    let b2 = crate::VBlockId(2);
    vcode.switch_to_block(b2);
    vcode.push_inst(Jmp {
        uncond: true,
        target: Some(Block::new(2)),
        binds: &[(1, OperandRole::Def)],
    });

    let cls = int_cls();
    let v1 = XReg::new(1, cls);
    let bind = |role: OperandRole| crate::machine::lowering::OperandBind {
        vreg: v1,
        field: 0,
        role,
        class: Some(cls),
    };
    let inst_ops: Vec<smallvec::SmallVec<[crate::machine::lowering::OperandBind; 2]>> = vec![
        smallvec::smallvec![],
        smallvec::smallvec![bind(OperandRole::Use)],
        smallvec::smallvec![bind(OperandRole::Def)],
    ];

    let intervals = compute_live_intervals(&vcode, &inst_ops, &[], "cfg_test");
    let iv = intervals.get(&v1).expect("v1 应有区间");
    assert_eq!(
        iv.start_point(),
        ProgPoint::new(1, 0),
        "v1 只在 b1/b2 活跃；b0 是无条件跳转 ⇒ 没有 fallthrough 边，不该被扩到 b0：{:?}",
        iv.segments
    );
}
