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
        for off in (0..size).step_by(8) {
            let off_v = self.builder.iconst(off as i64, TypeId::I64);
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
            let off = self.builder.iconst(o0, TypeId::I64);
            self.builder.iadd(addr, off)
        };
        // WA-23：按 ScalarPair 标量真实宽度读——恒 I64 会对窄字段读 8 字节
        //（Option<i32> 的 value@4 读 -0x44..-0x3d 混入相邻槽垃圾高位，pack
        // 写回时带进相邻槽 → Range.start 被覆盖 → for 循环不终止）。
        let lo = self.builder.load(a0, t0);
        let lo = self.builder.uextend(lo, TypeId::I64);
        let off1 = self.builder.iconst(o1, TypeId::I64);
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
            let off = self.builder.iconst(o0, TypeId::I64);
            self.builder.iadd(addr, off)
        };
        // WA-23：按标量宽度写——窄字段（如 Option<i32> 的 value@4）8 字节写
        // 越界覆盖相邻槽（range1 实证：hi 写 -0x6c..-0x65 覆盖 _4.start）。
        let lo_n = self.builder.ireduce(lo, t0);
        self.builder.store(lo_n, a0);
        let off1 = self.builder.iconst(o1, TypeId::I64);
        let a1 = self.builder.iadd(addr, off1);
        let hi_n = self.builder.ireduce(hi, t1);
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
                let off_v = self.builder.iconst(off, TypeId::I64);
                self.builder.iadd(addr, off_v)
            };
            let val = self.builder.iconst(v as i64, TypeId::I64);
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
                        let off_v = self.builder.iconst(off, TypeId::I64);
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
                    let es_v = self.builder.iconst(elem_size as i64, TypeId::I64);
                    let scaled = self.builder.imul(idx64, es_v);
                    addr = self.builder.iadd(addr, scaled);
                    ty = self.elem_ty(ty);
                }
                _ => {
                    // ConstantIndex/Subslice 等投影暂不支持：保持当前地址。
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
            return Ok(self.builder.iconst(0, TypeId::I64));
        }
        if crate::trace::trace_enabled("LOAD") {
            eprintln!(
                "[forge] load_place local={} proj={:?}",
                place.local.index(),
                place.projection,
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
