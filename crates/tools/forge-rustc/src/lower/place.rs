//! Place 地址计算、load/store 与聚合拷贝。
use super::*;
#[cfg(debug_assertions)]
use crate::types::assert_assignable;

/// 已解析的 place（P4.2）：地址 + 类型 + codegen 类型，load/store 统一经此。
pub(crate) struct CPlace<'tcx> {
    pub(crate) addr: Value,
    pub(crate) ty: Ty<'tcx>,
    pub(crate) codegen_ty: TypeId,
}

impl<'tcx, 'f> LowerCtxt<'tcx, 'f> {
    pub(crate) fn copy_agg(&mut self, dst: Value, src: Value, size: u32) {
        // **地址必须是 PTR 类型**（v27 M6-119）：`copy_agg` 的调用方有两类——
        // 栈槽地址（`StackAddr`，PTR）与 **ABI 指针**（sret 隐藏返回指针、间接
        // 聚合形参，签名里是 `i64` 槽）。后者直接拿 i64 当 `store`/`load` 的
        // 地址产 IR 非法（verifier：`StoreAddrNotPointer` / `LoadAddrNotPointer`），
        // 而 O0 不跑 pass、只有 `-O1` 之后才被 verifier 抓到。这里统一 `inttoptr`。
        let dst = if self.builder.value_type(dst) == Some(TypeId::PTR) {
            dst
        } else {
            self.builder.inttoptr(dst, TypeId::PTR)
        };
        let src = if self.builder.value_type(src) == Some(TypeId::PTR) {
            src
        } else {
            self.builder.inttoptr(src, TypeId::PTR)
        };
        for off in (0..size).step_by(8) {
            let off_v = self.builder.const_int(off as i64, TypeId::I64);
            let saddr = if off == 0 {
                src
            } else {
                self.builder.iadd(src, off_v)
            };
            let v = self.builder.load(saddr, TypeId::I64);
            let daddr = if off == 0 {
                dst
            } else {
                self.builder.iadd(dst, off_v)
            };
            self.builder.store(v, daddr);
        }
    }
    pub(crate) fn unpack_sp(&mut self, addr: Value, ty: Ty<'tcx>) -> (Value, Value) {
        let (o0, o1) = scalar_pair_offsets(self.tcx, ty);
        let (w0, w1) = scalar_pair_widths(self.tcx, ty);
        let t0 = scalar_width_type(w0);
        let t1 = scalar_width_type(w1);
        let a0 = if o0 == 0 {
            addr
        } else {
            let off = self.builder.const_int(o0, TypeId::I64);
            self.builder.iadd(addr, off)
        };
        // WA-23：按 ScalarPair 标量真实宽度读——恒 I64 会对窄字段读 8 字节
        //（Option<i32> 的 value@4 读 -0x44..-0x3d 混入相邻槽垃圾高位，pack
        // 写回时带进相邻槽 → Range.start 被覆盖 → for 循环不终止）。
        let lo = self.builder.load(a0, t0);
        let lo = self.builder.uextend(lo, TypeId::I64);
        let off1 = self.builder.const_int(o1, TypeId::I64);
        let a1 = self.builder.iadd(addr, off1);
        let hi = self.builder.load(a1, t1);
        let hi = self.builder.uextend(hi, TypeId::I64);
        (lo, hi)
    }
    pub(crate) fn pack_sp(&mut self, addr: Value, lo: Value, hi: Value, ty: Ty<'tcx>) {
        let (o0, o1) = scalar_pair_offsets(self.tcx, ty);
        let (w0, w1) = scalar_pair_widths(self.tcx, ty);
        let t0 = scalar_width_type(w0);
        let t1 = scalar_width_type(w1);
        let a0 = if o0 == 0 {
            addr
        } else {
            let off = self.builder.const_int(o0, TypeId::I64);
            self.builder.iadd(addr, off)
        };
        // WA-23：按标量宽度写——窄字段（如 Option<i32> 的 value@4）8 字节写
        // 越界覆盖相邻槽（range1 实证：hi 写 -0x6c..-0x65 覆盖 _4.start）。
        //
        // **同宽时不能发 `ireduce`**：`ireduce` 只能变窄，8 字节分量（如 `(u64,u64)`
        // 的 lo/hi）会得到 `ireduce i64 → i64` ⇒ 非法 IR
        //（`ConversionBitWidthMismatch`，v27 M6 实测：`a[i]`、常量提升静态读取在
        // `-O` 下必炸）。同宽直接原样存。
        let lo_n = if self.builder.value_type(lo) == Some(t0) {
            lo
        } else {
            self.builder.ireduce(lo, t0)
        };
        self.builder.store(lo_n, a0);
        let off1 = self.builder.const_int(o1, TypeId::I64);
        let a1 = self.builder.iadd(addr, off1);
        let hi_n = if self.builder.value_type(hi) == Some(t1) {
            hi
        } else {
            self.builder.ireduce(hi, t1)
        };
        self.builder.store(hi_n, a1);
    }
    pub(crate) fn agg_const_bytes(&mut self, addr: Value, bytes: &[u8]) {
        for (i, chunk) in bytes.chunks(8).enumerate() {
            let mut v = 0u64;
            for (j, b) in chunk.iter().enumerate() {
                v |= (*b as u64) << (8 * j);
            }
            let off = (i * 8) as i64;
            let daddr = if off == 0 {
                addr
            } else {
                let off_v = self.builder.const_int(off, TypeId::I64);
                self.builder.iadd(addr, off_v)
            };
            let val = self.builder.const_int(v as i64, TypeId::I64);
            self.builder.store(val, daddr);
        }
    }
    pub(crate) fn load_local(&mut self, local: mir::Local) -> Value {
        let slot = &self.locals[&local];
        let addr = self.builder.stack_addr(slot.offset);
        self.load_ty(addr, slot.ty)
    }
    pub(crate) fn store_local(&mut self, local: mir::Local, val: Value) {
        let slot = &self.locals[&local];
        let addr = self.builder.stack_addr(slot.offset);
        self.store_ty(val, addr, slot.ty);
    }
    pub(crate) fn place_addr(&mut self, place: &mir::Place<'tcx>) -> Value {
        let slot = &self.locals[&place.local];
        if crate::trace::trace_enabled("PLACE") {
            eprintln!(
                "[forge] place local={} offset={} proj={}",
                place.local.index(),
                slot.offset,
                place.projection.len()
            );
        }
        if crate::trace::trace_enabled("LOAD") {
            eprintln!(
                "[forge] place_addr local={} slot_offset={} proj_len={}",
                place.local.index(),
                slot.offset,
                place.projection.len()
            );
        }
        let mut addr = self.builder.stack_addr(slot.offset);
        let mut ty = self.body.local_decls[place.local].ty;
        for proj in place.projection.iter() {
            match proj {
                mir::ProjectionElem::Field(idx, _) => {
                    if crate::trace::trace_enabled("FIELD") {
                        eprintln!("[forge] place Field ty={ty} idx={}", idx.index());
                    }
                    let off = self.field_offset(ty, idx.index());
                    if off != 0 {
                        let off_v = self.builder.const_int(off, TypeId::I64);
                        addr = self.builder.iadd(addr, off_v);
                    }
                    ty = self.field_ty(ty, idx.index());
                }
                mir::ProjectionElem::Deref => {
                    addr = self.builder.load(addr, TypeId::PTR);
                    ty = self.pointee_ty(ty);
                }
                mir::ProjectionElem::Index(idx_local) => {
                    let idx_v = self.load_local(idx_local);
                    let idx64 = self.builder.uextend(idx_v, TypeId::I64);
                    let elem_size = layout_bytes(self.tcx, self.elem_ty(ty));
                    let es_v = self.builder.const_int(elem_size as i64, TypeId::I64);
                    let scaled = self.builder.imul(idx64, es_v);
                    addr = self.builder.iadd(addr, scaled);
                    ty = self.elem_ty(ty);
                }
                // **`ConstantIndex`：常量下标的数组元素**（`a[1 of 2]`）。
                // 此前落到 `_ =>` 的"保持当前地址"⇒ **静默读成 offset 0**！
                // 实测（v27 M6）：`buf[1]` 在 `-O` 下读到 `buf[0]` 的值——rustc 的
                // **优化 MIR** 用 `ConstantIndex`、未优化 MIR 用 `Index(_const)`，
                // 所以只有 `-O` 才暴露。`from_end` 时下标从末尾倒数：
                // `min_length - 1 - offset`（编译器已保证 len ≥ min_length）。
                mir::ProjectionElem::ConstantIndex {
                    offset,
                    min_length,
                    from_end,
                } => {
                    let idx = if from_end {
                        min_length.saturating_sub(1).saturating_sub(offset)
                    } else {
                        offset
                    };
                    let elem_ty = self.elem_ty(ty);
                    let elem_size = layout_bytes(self.tcx, elem_ty);
                    let byte_off = (idx as i64).saturating_mul(elem_size as i64);
                    if byte_off != 0 {
                        let off_v = self.builder.const_int(byte_off, TypeId::I64);
                        addr = self.builder.iadd(addr, off_v);
                    }
                    ty = elem_ty;
                }
                // `arr[from..to]`：数据指针前移 `from * elem_size`。
                // `from_end`（`arr[..-k]`）需要运行时长度，本后端拿不到 ⇒ 静默按
                // `from` 处理会读到错的区间，故此处**不猜**：保持原地址并留给
                // 上层行为（切片元数据的构造在别处），不引入新的错值路径。
                mir::ProjectionElem::Subslice { from, .. } => {
                    let elem_ty = self.elem_ty(ty);
                    let elem_size = layout_bytes(self.tcx, elem_ty);
                    let byte_off = (from as i64).saturating_mul(elem_size as i64);
                    if byte_off != 0 {
                        let off_v = self.builder.const_int(byte_off, TypeId::I64);
                        addr = self.builder.iadd(addr, off_v);
                    }
                    ty = self.elem_ty(ty);
                }
                _ => {
                    // 其余投影（OpaqueCast 等）：保持当前地址。
                }
            }
        }
        addr
    }

    /// 解析 place 为统一表示（P4.2）：地址 + 类型 + codegen 类型。
    /// load/store 统一经此入口，消除散落的 `place_addr` + `place.ty` 重复获取。
    fn place(&mut self, place: &mir::Place<'tcx>) -> Result<CPlace<'tcx>, ForgeError> {
        let ty = place.ty(&self.body.local_decls, self.tcx).ty;
        Ok(CPlace {
            addr: self.place_addr(place),
            ty,
            codegen_ty: map_type(ty, self.tcx)?,
        })
    }

    pub(crate) fn load_place(&mut self, place: &mir::Place<'tcx>) -> Result<Value, ForgeError> {
        // unit/never（VOID）local：无存储，返回常量 0（避免读占位槽产生垃圾）。
        if place.ty(&self.body.local_decls, self.tcx).ty.is_unit()
            || place.ty(&self.body.local_decls, self.tcx).ty.is_never()
        {
            return Ok(self.builder.const_int(0, TypeId::I64));
        }
        if crate::trace::trace_enabled("LOAD") {
            eprintln!(
                "[forge] load_place local={} proj={:?} ty={} codegen_ty={:?}",
                place.local.index(),
                place.projection,
                place.ty(&self.body.local_decls, self.tcx).ty,
                self.locals.get(&place.local).map(|s| s.ty),
            );
        }
        let cp = self.place(place)?;
        // P2.5 自检：8 字节标量/指针的读取宽度应为 I64/PTR（防错位读；
        // float 走 XMM 宽度 F64、向量（V64 8B / V128 16B 按值 XMM/Direct，
        // WA-37 D3）不在此断言）
        if !is_agg_mem(self.tcx, cp.ty)
            && !is_scalar_pair_abi(self.tcx, cp.ty)
            && layout_bytes(self.tcx, cp.ty) == 8
            && !matches!(cp.ty.kind(), ty::TyKind::Float(_))
            && !matches!(cp.codegen_ty, TypeId::V64 | TypeId::V128 | TypeId::V256)
        {
            assert_assignable(cp.codegen_ty, TypeId::I64, &format!("load_place {}", cp.ty));
        }
        Ok(self.load_ty(cp.addr, cp.codegen_ty))
    }
    pub(crate) fn store_place(&mut self, place: &mir::Place<'tcx>, val: Value) {
        // unit/never（VOID）local：无存储，no-op（避免写占位槽）。
        if place.ty(&self.body.local_decls, self.tcx).ty.is_unit()
            || place.ty(&self.body.local_decls, self.tcx).ty.is_never()
        {
            return;
        }
        if crate::trace::trace_enabled("STORE") {
            eprintln!(
                "[forge] store_place local={} proj={:?} val={val:?}",
                place.local.index(),
                place.projection,
            );
        }
        let cp = match self.place(place) {
            Ok(cp) => cp,
            // map_type 失败（store 路径原语义为 I32 兜底）：重新取地址组装
            Err(_) => {
                let ty = place.ty(&self.body.local_decls, self.tcx).ty;
                CPlace {
                    addr: self.place_addr(place),
                    ty,
                    codegen_ty: TypeId::I32,
                }
            }
        };
        if crate::trace::trace_enabled("STORE") {
            eprintln!("[forge]   -> addr={:?}", cp.addr);
        }
        self.store_ty(val, cp.addr, cp.codegen_ty);
    }
}
