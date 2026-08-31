//! 符号 ↔ FuncRef/GlobalId 序号 双向映射表。
//!
//! forge-ir 的 Call 指令把重定位符号记为 "@{FuncRef 序号}"（@call_reloc32）、
//! 全局数据为 "G{N}"，写对象文件前必须替换为真实符号名；序号跨函数全局
//! 唯一，由 codegen_crate 持有并在编译各实例时共享。

use crate::prelude::*;

/// 模块级函数符号表：符号名 ↔ FuncRef 序号 双向映射。
#[derive(Default)]
pub struct FuncRefTable {
    next: u32,
    by_sym: HashMap<String, u32>,
    by_idx: HashMap<u32, String>,
    /// 全局数据符号（static）：alloc_id → GlobalId 序号（对象文件重定位
    /// 符号为 "G{N}"，与函数重定位 "@N" 前缀区分）。
    global_next: u32,
    global_by_alloc: HashMap<rustc_middle::mir::interpret::AllocId, u32>,
    global_by_idx: HashMap<u32, String>,
    /// vtable 数据段（unsize cast 生成）：(符号名, 8 字节指针表字节,
    /// (数据内偏移, reloc, 符号, addend) 列表, 对齐)——主流程统一写 .data
    /// （[WA-01] MSVC 链接器不应用 .rodata 的 ADDR64 重定位，见 backend.rs）。
    vtables: Vec<(String, Vec<u8>, Vec<(usize, RelocKind, String, i64)>, u64)>,
    /// promoted/slice 常量数据段（`&[1,2,3]`、`&"str"` 字面量 rodata 副本）：
    /// (符号名, 字节, 对齐)——backend.rs 统一写 .rodata。
    promoted: Vec<(String, Vec<u8>, u64)>,
}

impl FuncRefTable {
    /// 取得符号对应的 FuncRef 序号，不存在则分配新序号。
    pub fn intern(&mut self, sym: &str) -> u32 {
        if let Some(&id) = self.by_sym.get(sym) {
            return id;
        }
        let id = self.next;
        self.next += 1;
        self.by_sym.insert(sym.to_string(), id);
        self.by_idx.insert(id, sym.to_string());
        id
    }

    /// 把 CompiledFunction 中 "@N" 形式的重定位符号替换为真实符号名。
    pub fn resolve_relocs(&self, func: &mut CompiledFunction) {
        for reloc in &mut func.relocations {
            if let Some(idx_str) = reloc.symbol.strip_prefix('@') {
                if let Ok(idx) = idx_str.parse::<u32>() {
                    if let Some(sym) = self.by_idx.get(&idx) {
                        reloc.symbol = sym.clone().into();
                    }
                }
            }
        }
    }

    /// 取得符号对应的 GlobalId（static 数据符号）。
    pub fn intern_global(
        &mut self,
        alloc_id: rustc_middle::mir::interpret::AllocId,
        sym: &str,
    ) -> u32 {
        if let Some(&id) = self.global_by_alloc.get(&alloc_id) {
            return id;
        }
        let id = self.global_next;
        self.global_next += 1;
        self.global_by_alloc.insert(alloc_id, id);
        self.global_by_idx.insert(id, sym.to_string());
        id
    }

    /// 把 CompiledFunction 中 "G{N}" 形式的重定位符号替换为真实符号名。
    pub fn resolve_global_relocs(&self, func: &mut CompiledFunction) {
        for reloc in &mut func.relocations {
            if let Some(idx_str) = reloc.symbol.strip_prefix('G') {
                if let Ok(idx) = idx_str.parse::<u32>() {
                    if let Some(sym) = self.global_by_idx.get(&idx) {
                        reloc.symbol = sym.clone().into();
                    }
                }
            }
        }
    }

    /// 记录 vtable 数据段（unsize cast 生成）并返回 GlobalId。
    /// 幂等：同一 alloc_id（同 ty + trait）复用同一 GlobalId/符号。
    pub fn intern_vtable(
        &mut self,
        alloc_id: rustc_middle::mir::interpret::AllocId,
        sym: &str,
        bytes: Vec<u8>,
        relocs: Vec<(usize, RelocKind, String, i64)>,
    ) -> u32 {
        if let Some(&id) = self.global_by_alloc.get(&alloc_id) {
            return id;
        }
        let id = self.global_next;
        self.global_next += 1;
        self.global_by_alloc.insert(alloc_id, id);
        self.global_by_idx.insert(id, sym.to_string());
        self.vtables.push((sym.to_string(), bytes, relocs, 8));
        id
    }

    /// 已注册的 vtable 数据段（供 codegen_crate 写对象文件时统一落盘）。
    pub fn vtables(&self) -> &[(String, Vec<u8>, Vec<(usize, RelocKind, String, i64)>, u64)] {
        &self.vtables
    }

    /// 登记 promoted/slice 常量数据段（`&"str"`、`&[1,2,3]` 字面量的
    /// rodata 副本）并返回 GlobalId。幂等：同一 alloc_id 复用。
    pub fn intern_promoted(
        &mut self,
        alloc_id: rustc_middle::mir::interpret::AllocId,
        sym: &str,
        bytes: Vec<u8>,
        align: u64,
    ) -> u32 {
        if let Some(&id) = self.global_by_alloc.get(&alloc_id) {
            return id;
        }
        let id = self.global_next;
        self.global_next += 1;
        self.global_by_alloc.insert(alloc_id, id);
        self.global_by_idx.insert(id, sym.to_string());
        self.promoted.push((sym.to_string(), bytes, align));
        id
    }

    /// 已注册的 promoted/slice 常量数据段（供 codegen_crate 写 .rodata）。
    pub fn promoted(&self) -> &[(String, Vec<u8>, u64)] {
        &self.promoted
    }
}
