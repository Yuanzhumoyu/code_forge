//! 常量求值（eval_const_bytes）。
use super::*;

impl<'tcx, 'f> LowerCtxt<'tcx, 'f> {
    pub(crate) fn eval_const_bytes(&self, ct: rustc_middle::mir::Const<'tcx>) -> Option<Vec<u8>> {
        let ty = ct.ty();
        let size = layout_bytes(self.tcx, ty);
        if crate::trace::trace_enabled("CONST") {
            eprintln!("[forge] eval_const_bytes ty={ty} size={size}");
        }
        let r = self.eval_const_bytes_inner(ct, ty, size);
        if crate::trace::trace_enabled("CONST") {
            eprintln!(
                "[forge]   -> {:?}",
                r.as_ref()
                    .map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>())
            );
        }
        r
    }
    pub(crate) fn eval_const_bytes_inner(
        &self,
        ct: rustc_middle::mir::Const<'tcx>,
        _ty: Ty<'tcx>,
        size: u32,
    ) -> Option<Vec<u8>> {
        use rustc_middle::mir::ConstValue;
        use rustc_middle::mir::interpret::AllocRange;
        if size == 0 {
            return Some(Vec::new());
        }
        let cv = ct
            .eval(
                self.tcx,
                ty::TypingEnv::fully_monomorphized(),
                rustc_span::DUMMY_SP,
            )
            .ok()?;
        match cv {
            ConstValue::Scalar(s) => {
                let bits = s
                    .to_bits(rustc_abi::Size::from_bytes(size))
                    .discard_err()
                    .unwrap_or(0);
                Some(
                    (0..size)
                        .map(|i| ((bits >> (8 * i)) & 0xff) as u8)
                        .collect(),
                )
            }
            ConstValue::Indirect { alloc_id, offset } => {
                if let rustc_middle::mir::interpret::GlobalAlloc::Memory(alloc) =
                    self.tcx.global_alloc(alloc_id)
                {
                    let range = AllocRange {
                        start: offset,
                        size: rustc_abi::Size::from_bytes(size),
                    };
                    Some(alloc.0.get_bytes_unchecked(range).to_vec())
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
