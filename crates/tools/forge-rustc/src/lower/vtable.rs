//! Vtable 生成（unsize cast）与 enum 数据字段布局。
use super::*;

impl<'tcx, 'f> LowerCtxt<'tcx, 'f> {
    pub(crate) fn gen_vtable(&mut self, ty: Ty<'tcx>, dyn_ty: Ty<'tcx>) -> Result<u32, ForgeError> {
        use rustc_middle::ty::TyKind;
        let trait_ref = match dyn_ty.kind() {
            TyKind::Dynamic(tt, _) => tt
                .principal()
                .map(|p| self.tcx.instantiate_bound_regions_with_erased(p)),
            _ => None,
        }
        .ok_or_else(|| ForgeError::Message("vtable for non-trait dyn type".into()))?;

        // 与 vtable_allocation_provider 一致：self_ty 化 + erase regions
        let tr = trait_ref.with_self_ty(self.tcx, ty);
        let tr = self.tcx.erase_and_anonymize_regions(tr);
        let entries = self.tcx.vtable_entries(tr);
        let alloc_id = self.tcx.vtable_allocation((ty, Some(trait_ref)));

        let ptr = self.tcx.data_layout.pointer_size().bytes_usize();
        let mut data = Vec::with_capacity(entries.len() * ptr);
        let mut relocs: Vec<(usize, RelocKind, String, i64)> = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            let off = i * ptr;
            match *entry {
                VtblEntry::MetadataDropInPlace => {
                    if ty.needs_drop(self.tcx, ty::TypingEnv::fully_monomorphized()) {
                        let inst = Instance::resolve_drop_glue(self.tcx, ty);
                        let sym = self.mono_symbol(&inst);
                        data.extend_from_slice(&[0u8; 8]);
                        relocs.push((off, RelocKind::Absolute(8), sym, 0));
                    } else {
                        data.extend_from_slice(&[0u8; 8]); // null
                    }
                }
                VtblEntry::MetadataSize => {
                    let size = self
                        .tcx
                        .layout_of(ty::PseudoCanonicalInput {
                            typing_env: ty::TypingEnv::fully_monomorphized(),
                            value: ty,
                        })
                        .map_err(|e| ForgeError::Message(format!("vtable layout: {e}")))?
                        .size
                        .bytes();
                    data.extend_from_slice(&size.to_le_bytes());
                }
                VtblEntry::MetadataAlign => {
                    let align = self
                        .tcx
                        .layout_of(ty::PseudoCanonicalInput {
                            typing_env: ty::TypingEnv::fully_monomorphized(),
                            value: ty,
                        })
                        .map_err(|e| ForgeError::Message(format!("vtable layout: {e}")))?
                        .align
                        .abi
                        .bytes();
                    data.extend_from_slice(&align.to_le_bytes());
                }
                VtblEntry::Vacant | VtblEntry::TraitVPtr(_) => {
                    data.extend_from_slice(&[0u8; 8]);
                }
                VtblEntry::Method(inst) => {
                    let sym = self.mono_symbol(&inst);
                    data.extend_from_slice(&[0u8; 8]);
                    relocs.push((off, RelocKind::Absolute(8), sym, 0));
                }
            }
        }

        let sym = format!("__vtable_{:?}", alloc_id);
        let g = self.func_refs.intern_vtable(alloc_id, &sym, data, relocs);
        Ok(g)
    }
    pub(crate) fn enum_data_fields(
        &self,
        layout: &rustc_middle::ty::layout::TyAndLayout<'tcx>,
    ) -> Vec<i64> {
        use rustc_abi::Variants;
        // niche/标量表示枚举（Option<usize> 等）：判别嵌入 payload（值即判别），
        // 构造/读取都按"值在偏移 0"处理（不写独立判别、不做字段偏移）
        let Variants::Multiple {
            tag,
            tag_field,
            variants,
            ..
        } = &layout.layout.variants()
        else {
            return Vec::new();
        };
        if crate::trace::trace_enabled("FIELD") {
            eprintln!(
                "[forge] enum_data tag={tag:?} tag_field={tag_field:?} backend={:?} size={}",
                layout.layout.backend_repr,
                layout.layout.size().bytes(),
            );
            for (vi, v) in variants.iter().enumerate() {
                let offs: Vec<u64> = (0..v.field_offsets.len())
                    .map(|i| v.field_offsets[FieldIdx::new(i)].bytes())
                    .collect();
                eprintln!(
                    "[forge]   variant {vi} fields={} offs={offs:?} size={}",
                    v.field_offsets.len(),
                    v.size.bytes()
                );
            }
        }
        if matches!(
            layout.layout.backend_repr,
            rustc_abi::BackendRepr::Scalar(_)
        ) {
            let Some(v) = variants.iter().find(|v| v.has_fields()) else {
                return Vec::new();
            };
            return (0..v.field_offsets.len()).map(|_| 0).collect();
        }
        let v = variants
            .iter()
            .find(|v| v.has_fields())
            .or_else(|| variants.iter().next());
        let Some(v) = v else { return Vec::new() };
        (0..v.field_offsets.len())
            .map(|i| v.field_offsets[FieldIdx::new(i)].bytes() as i64)
            .collect()
    }
}
