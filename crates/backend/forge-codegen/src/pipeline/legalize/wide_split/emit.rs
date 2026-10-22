//! 肢发射器：**没有槽**，只负责"在锚点之前插入窄指令"与"就地改写"。
//!
//! 两条纪律都是实测换来的：
//!
//! 1. `make_inst` 把新指令追加到**块尾**（在终结符之后）⇒ 必须搬回锚点前，否则 isel
//!    根本看不到它们（症状：vcode 里插入的 `load/store` 全部消失）；
//! 2. 就地清空/改写操作数后**必须** `refresh_inst_uses`，否则驱动的 use-list 一致性检查
//!    会报 `inst i3 operand 0 (value v22) not found in use-lists`。

use super::*;

/// 肢/指令发射器。
pub(super) struct LimbEmitter<'a> {
    pub(super) func: &'a mut Function,
    /// 新增指令数（`ChangeSet::new_insts`）。
    pub(super) created: usize,
    /// 新增常量数（`ChangeSet::new_consts`）。
    pub(super) consts: usize,
}

impl<'a> LimbEmitter<'a> {
    pub(super) fn new(func: &'a mut Function) -> Self {
        Self {
            func,
            created: 0,
            consts: 0,
        }
    }

    /// 肢类型（`i<W>`）。
    pub(super) fn limb_ty(&self, w: u32) -> TypeId {
        self.func.types.int_ty(w)
    }

    /// 在 `anchor` **之前**插入一条单结果指令，返回结果值。
    pub(super) fn emit_before(
        &mut self,
        anchor: Inst,
        op: Opcode,
        ops: &[Value],
        imms: &[Immediate],
        ty: TypeId,
    ) -> Value {
        let block = self.func.dfg.inst_data(anchor).block;
        let inst = self.func.make_inst(
            op,
            block,
            smallvec::SmallVec::from_slice(ops),
            smallvec::SmallVec::from_slice(imms),
            &[ty],
            InstFlags::NONE,
        );
        let v = self.func.dfg.inst_data(inst).results[0];
        self.created += 1;
        self.place_before(anchor, inst);
        v
    }

    /// 把刚追加到块尾的指令搬到 `anchor` 之前（保持块内顺序）。
    fn place_before(&mut self, anchor: Inst, inst: Inst) {
        let block = self.func.dfg.inst_data(anchor).block;
        let pos = self
            .func
            .dfg
            .block(block)
            .inst_order
            .iter()
            .position(|x| *x == anchor)
            .expect("锚点必须是非终结符且在块内");
        let last = {
            let order = &mut self.func.dfg.block_mut(block).inst_order;
            order.pop().expect("刚插入的指令必在块尾")
        };
        debug_assert_eq!(last, inst, "块尾必须就是刚插入的那条");
        self.func.dfg.block_mut(block).inst_order.insert(pos, last);
    }

    /// 在 `anchor` 之前插入一条 `store`（无结果）。
    pub(super) fn emit_before_store(&mut self, anchor: Inst, v: Value, addr: Value) -> Inst {
        let block = self.func.dfg.inst_data(anchor).block;
        let inst = self.func.make_inst(
            Opcode::Store,
            block,
            smallvec::smallvec![v, addr],
            smallvec::smallvec![],
            &[],
            InstFlags::NONE,
        );
        self.created += 1;
        self.place_before(anchor, inst);
        inst
    }

    /// 就地改写一条指令（保留结果值身份），并刷新使用边。
    pub(super) fn rewrite_in_place(&mut self, inst: Inst, op: Opcode, ops: &[Value]) {
        {
            let d = self.func.dfg.inst_mut(inst);
            d.opcode = op;
            d.operands = smallvec::SmallVec::from_slice(ops);
            d.immediates = smallvec::SmallVec::new();
        }
        self.func.refresh_inst_uses(inst);
    }

    /// 把指令变成 `Nop` 并**清空结果**（调用方保证此时已无使用者）。
    pub(super) fn kill(&mut self, inst: Inst) {
        {
            let d = self.func.dfg.inst_mut(inst);
            d.opcode = Opcode::Nop;
            d.operands = smallvec::SmallVec::new();
            d.immediates = smallvec::SmallVec::new();
            d.results.clear();
        }
        self.func.refresh_inst_uses(inst);
    }

    /// `i<w>` 常量（走整数常量池，值的规范化由 `canonical_int` 负责）。
    pub(super) fn iconst_in(&mut self, anchor: Inst, value: u64, w: u32) -> Value {
        let bits = self.limb_bits(w);
        let cid = self
            .func
            .constants
            .intern_int(canonical_int(u128::from(value), bits) as i128, bits);
        self.consts += 1;
        let ty = self.limb_ty(w);
        self.emit_before(anchor, Opcode::Iconst, &[], &[Immediate::Const(cid)], ty)
    }

    /// `PTR` 类型的偏移常量。
    ///
    /// **必须**是 PTR 类型：用 `i64` 常量去和 `ptr` 相加会生成 `add ptr, i64`（类型混用），
    /// 实测会让地址算错（症状是"高半读到 0"）。
    pub(super) fn iconst_ptr_in(&mut self, anchor: Inst, value: u64, w: u32) -> Value {
        let bits = self.limb_bits(w);
        let cid = self.func.constants.intern_int(value as i128, bits);
        self.consts += 1;
        self.emit_before(
            anchor,
            Opcode::Iconst,
            &[],
            &[Immediate::Const(cid)],
            TypeId::PTR,
        )
    }

    /// 两值相加（PTR 语义）：`a + b`，结果类型 `PTR`。
    pub(super) fn ptr_add(&mut self, anchor: Inst, a: Value, b: Value) -> Value {
        self.emit_before(anchor, Opcode::Iadd, &[a, b], &[], TypeId::PTR)
    }

    /// 把 `i1` 值零扩展成 `i<W>`（比较标志走 i64 通道用）。
    pub(super) fn zext_bool(&mut self, anchor: Inst, v: Value, w: u32) -> Value {
        let ty = self.limb_ty(w);
        self.emit_before(anchor, Opcode::Uextend, &[v], &[], ty)
    }

    /// 三目（窄值）。
    pub(super) fn select(
        &mut self,
        anchor: Inst,
        c: Value,
        a: Value,
        b: Value,
        ty: TypeId,
    ) -> Value {
        self.emit_before(anchor, Opcode::Select, &[c, a, b], &[], ty)
    }

    fn limb_bits(&self, w: u32) -> u32 {
        let _ = self;
        w
    }
}

/// `W` 位的全 1 掩码（`W ≤ 64`）。
pub(super) fn mask_of(w: u32) -> u64 {
    if w >= 64 { u64::MAX } else { (1u64 << w) - 1 }
}
