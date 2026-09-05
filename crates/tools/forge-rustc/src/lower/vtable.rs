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
        let trait_def_id = trait_ref.def_id;
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

        let sym = self.vtable_sym(&data, &relocs, trait_def_id);
        let g = self.func_refs.intern_vtable(alloc_id, &sym, data, relocs);
        Ok(g)
    }

    /// vtable 数据段符号名：**内容稳定键**（WA-38 残余 M5）。
    ///
    /// M4 及以前用 `__vtable_{alloc_id:?}`——alloc_id 由 rustc 全局
    /// `AtomicU64` 按首次请求序分配，-Z threads 真并行下各函数任务的调度序
    /// 不定 → 同一 vtable 内容跨运行符号名数值漂移（内容↔符号恒一致，仅
    /// 名称非字节确定）。M5：64 位 FNV-1a 哈希规范化流 = 指针表 data +
    /// 逐 reloc（offset / RelocKind / 目标符号 / addend）+ align（恒 8）+
    /// **trait principal def_path**——跨运行与调度序无关；def_path 是
    /// 语义鉴别键：两个不同 trait 的 vtable 即使指针表内容完全相同
    /// （如零方法空 trait + 无 drop ZST）也保持不同名、各自独立数据段
    /// （与 alloc_id 命名时代语义一致——rustc 按 (ty, principal) 各给独立
    /// AllocId，不按内容去重）。同名必同内容（intern/merge debug_assert
    /// 碰撞防护）。
    pub(crate) fn vtable_sym(
        &self,
        data: &[u8],
        relocs: &[(usize, RelocKind, String, i64)],
        trait_def_id: rustc_hir::def_id::DefId,
    ) -> String {
        let mut canonical: Vec<u8> = Vec::with_capacity(data.len() + 64);
        canonical.extend_from_slice(data);
        canonical.extend_from_slice(&8u64.to_le_bytes()); // align（vtable 恒 8）
        for (off, kind, sym, addend) in relocs {
            canonical.extend_from_slice(&(*off as u64).to_le_bytes());
            canonical.extend_from_slice(format!("{kind:?}").as_bytes());
            canonical.push(0);
            canonical.extend_from_slice(sym.as_bytes());
            canonical.push(0);
            canonical.extend_from_slice(&addend.to_le_bytes());
        }
        canonical.extend_from_slice(self.tcx.def_path_str(trait_def_id).as_bytes());
        format!("__vtable_{:016x}", super::fnv1a64(&canonical))
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
