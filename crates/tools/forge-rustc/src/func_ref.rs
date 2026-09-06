//! 符号 ↔ FuncRef/GlobalId 序号 双向映射表。
//!
//! forge-ir 的 Call 指令把重定位符号记为 "@{FuncRef 序号}"（@call_reloc32）、
//! 全局数据为 "G{N}"，写对象文件前必须替换为真实符号名。
//!
//! # M3/M4（并行 CGU Stage A）编号作用域
//! 序号**不需要跨函数全局唯一**：@N/G{N} 只以重定位符号留在各函数的机器码里，
//! 每函数编译完经 [`FuncRefTable::resolve_relocs`]/[`resolve_global_relocs`]
//! 就地换成真实符号名（本表只读查询），编号不跨函数逃逸。因此并行场景下
//! **每个函数任务持有一张私有 FuncRefTable**（零加锁——M4 起任务粒度 =
//! 单函数，任务即 par_map 的一个元素），任务结束时登记条目
//! （vtable/promoted/line/var/enum）经 [`FuncRefTable::merge_task_table`]
//! 按任务序归并回主表：vtable 按 alloc_id 去重、promoted/slice **按
//! alloc_id 或内容**（bytes+align）去重（M5 稳定键命名）、line/var 按任务序
//! 追加（= 全局函数序，与旧串行首见序一致）、enum 按 desc 去重保留先见序。
//!
//! # M5（WA-38 残余关闭）：内部数据符号 = 内容稳定键
//! 内部 slice/vtable 常量符号**不再含 rustc AllocId 数值**（`__slice_alloc{N}`
//! 时代的 N 是 rustc 全局 `AtomicU64` 首次请求序——-Z threads 真并行下随
//! 调度序跨运行漂移）：改为记录内容的 64 位 FNV-1a 哈希
//! （`__slice_{hash}` / `__vtable_{hash}`，见 lower/statement.rs
//! `slice_sym` / lower/vtable.rs `vtable_sym`）。跨运行/串并行稳定，同名必
//! 同内容；promoted 因内容去重而共享记录（[`FuncRefTable::intern_promoted`]
//! 与 [`FuncRefTable::merge_task_table`] 双路去重，debug_assert 碰撞防护）。

use crate::prelude::*;

/// vtable 数据段记录：`(alloc_id, 符号名, 8 字节指针表字节,
/// (数据内偏移, reloc, 符号, addend) 列表, 对齐)`——alloc_id 为任务间
/// 归并去重键（见 [`FuncRefTable::merge_task_table`]）；符号名 = 内容
/// 稳定键（M5，`__vtable_{hash}`，vtable.rs）。
pub type VtableRecord = (
    rustc_middle::mir::interpret::AllocId,
    String,
    Vec<u8>,
    Vec<(usize, RelocKind, String, i64)>,
    u64,
);
/// promoted/slice 常量数据段记录：`(alloc_id, 符号名, 字节, 对齐)`。
/// 符号名 = 内容稳定键（M5，`__slice_{hash}`）——同内容跨 alloc_id 共享
/// 同一记录（intern/merge 双路内容去重）。
pub type PromotedRecord = (rustc_middle::mir::interpret::AllocId, String, Vec<u8>, u64);

/// 模块级函数符号表：符号名 ↔ FuncRef 序号 双向映射。
///
/// 串行路径由 codegen_crate 持单例；并行路径每 worker 任务一个实例
/// （编号任务内分配、任务内 resolve，见模块注释）。
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
    /// vtable 数据段（unsize cast 生成）：记录见 [`VtableRecord`]——主流程
    /// 统一写 .data（[WA-01] MSVC 链接器不应用 .rodata 的 ADDR64 重定位，
    /// 见 backend.rs）。带 alloc_id 以支持任务间按 alloc_id 归并去重。
    vtables: Vec<VtableRecord>,
    /// promoted/slice 常量数据段（`&[1,2,3]`、`&"str"` 字面量 rodata 副本）：
    /// 记录见 [`PromotedRecord`]——backend.rs 统一写 .rodata。M5 起按
    /// **内容**去重（同 bytes+align 共享记录/符号，见 intern_promoted）。
    promoted: Vec<PromotedRecord>,
    /// 行号表（C1 DebugInfo line-tables-only）：(符号名, 源码行号 1-based)。
    /// 每函数一个条目（函数起始地址 → 函数定义行）——`-C debuginfo=1`
    /// 的最小语义：调试器可定位当前函数/行（粗粒度）。
    line_entries: Vec<(String, u32)>,
    /// C2 DebugInfo 变量表（-C debuginfo=2 full）：(函数符号, 变量列表)。
    /// 见 `crate::dwarf::VarEntry`（名/槽偏移/类型/参数标志/声明行）。
    var_entries: Vec<crate::dwarf::FnVarEntries>,
    /// C2 聚合类型注册表（-C debuginfo=2）：C-like 枚举（unit 变体）。
    /// 见 `crate::dwarf::EnumTypeEntry`。
    enum_types: Vec<crate::dwarf::EnumTypeEntry>,
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
        self.vtables
            .push((alloc_id, sym.to_string(), bytes, relocs, 8));
        id
    }

    /// 已注册的 vtable 数据段（M6 起数据归属归并在 backend.rs 按符号名
    /// owner 判定——本表不再作为落盘源；此访问器保留供表内断言/调试）。
    #[allow(dead_code)]
    pub fn vtables(&self) -> &[VtableRecord] {
        &self.vtables
    }

    /// 取出全部 vtable 记录（并行任务归并用；serial 主表不必要）。
    pub fn drain_vtables(&mut self) -> Vec<VtableRecord> {
        std::mem::take(&mut self.vtables)
    }

    /// 登记 promoted/slice 常量数据段（`&"str"`、`&[1,2,3]` 字面量的
    /// rodata 副本）并返回 GlobalId。幂等且**内容去重**（M5 稳定键）：
    /// - 同一 alloc_id 直接复用；
    /// - 不同 alloc_id 但同内容（bytes+align 相等）也复用同一
    ///   GlobalId/符号/数据记录——M5 起符号名 = 内容稳定键
    ///   （`__slice_{内容哈希}`，见 lower/statement.rs `slice_sym`），
    ///   同名记录只允许一份（重复登记会产生重复同名对象符号）；
    ///   promoted 常量每次求值各得独立 alloc_id，同内容跨 alloc 是常态。
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
        // 内容去重：已存在同 (bytes, align) 记录 → 复用其符号/GlobalId
        // （内容稳定键命名下符号名必同；debug_assert 碰撞防护，release
        // 以内容相等为准——名字冲突但内容不同 = 哈希碰撞，拒绝合并）。
        if let Some((first_alloc, _, _, _)) = self
            .promoted
            .iter()
            .find(|(_, _, b, a)| *b == bytes && *a == align)
        {
            let gid = self.global_by_alloc[first_alloc];
            self.global_by_alloc.insert(alloc_id, gid);
            return gid;
        }
        let id = self.global_next;
        self.global_next += 1;
        self.global_by_alloc.insert(alloc_id, id);
        self.global_by_idx.insert(id, sym.to_string());
        self.promoted
            .push((alloc_id, sym.to_string(), bytes, align));
        id
    }

    /// 已注册的 promoted/slice 常量数据段（M6 起数据归属归并在 backend.rs
    /// 按符号名 owner 判定——本表不再作为落盘源；此访问器保留供表内断言）。
    #[allow(dead_code)]
    pub fn promoted(&self) -> &[PromotedRecord] {
        &self.promoted
    }

    /// 取出全部 promoted 记录（并行任务归并用）。
    pub fn drain_promoted(&mut self) -> Vec<PromotedRecord> {
        std::mem::take(&mut self.promoted)
    }

    /// 登记函数行号条目（C1：符号名 → 源码行号 1-based）。
    pub fn add_line_entry(&mut self, sym: &str, line: u32) {
        self.line_entries.push((sym.to_string(), line));
    }

    /// 已登记的 (符号, 行号) 列表（供 codegen_crate 生成 .debug_line）。
    pub fn line_entries(&self) -> &[(String, u32)] {
        &self.line_entries
    }

    /// 取出全部行号条目（并行任务归并用）。
    pub fn drain_line_entries(&mut self) -> Vec<(String, u32)> {
        std::mem::take(&mut self.line_entries)
    }

    /// C2 debuginfo：登记函数的源变量表（函数符号 + 变量列表）。
    pub fn add_var_entries(&mut self, sym: &str, vars: Vec<crate::dwarf::VarEntry>) {
        self.var_entries.push(crate::dwarf::FnVarEntries {
            sym: sym.to_string(),
            vars,
        });
    }

    /// 已登记的 (符号, 变量表) 列表（供 codegen_crate 生成 .debug_info）。
    pub fn var_entries(&self) -> &[crate::dwarf::FnVarEntries] {
        &self.var_entries
    }

    /// 取出全部变量表（并行任务归并用）。
    pub fn drain_var_entries(&mut self) -> Vec<crate::dwarf::FnVarEntries> {
        std::mem::take(&mut self.var_entries)
    }

    /// C2 debuginfo：登记 C-like 枚举类型（desc 去重）。
    pub fn add_enum_type(&mut self, e: crate::dwarf::EnumTypeEntry) {
        if !self.enum_types.iter().any(|x| x.desc == e.desc) {
            self.enum_types.push(e);
        }
    }

    /// 已登记的枚举类型表（供 codegen_crate 生成 .debug_info）。
    pub fn enum_types(&self) -> &[crate::dwarf::EnumTypeEntry] {
        &self.enum_types
    }

    /// 取出全部枚举类型（并行任务归并用）。
    pub fn drain_enum_types(&mut self) -> Vec<crate::dwarf::EnumTypeEntry> {
        std::mem::take(&mut self.enum_types)
    }

    /// M3/M4（并行 CGU Stage A）：把一张任务私有表（编译完一个函数任务的
    /// 产出）的全部登记条目归并入本表。任务按原实例序（= 全局函数首见序，
    /// M4 par_map 保输入序、串行 map 天然保序）调用本方法 → 归并后序 =
    /// **全局函数首见序**，与旧串行路径（单表边编译边登记）完全一致：
    /// - line/var：直接追加（每函数至多一条，符号不跨任务重复）；
    /// - enum：按 desc 去重保留先见序；
    /// - vtables：按 alloc_id 去重——同 alloc_id（同 (ty, principal)，
    ///   rustc `vtable_allocation` 查询缓存）必然同名同字节同 relocs，
    ///   debug_assert 守护（release 下以先见者为准）；
    /// - promoted/slice：**按 alloc_id 或内容**（bytes+align）去重——M5
    ///   稳定键命名（`__slice_{内容哈希}`）后同名记录只允许一份：同一
    ///   内容可能经不同 alloc_id 到达（promoted 每次求值各得独立
    ///   AllocId），先见者保留、内容/对齐 debug_assert 相等。
    /// 不合并 by_sym/by_idx/global_* 编号映射：编号只用于任务内就地
    /// resolve（函数已在任务内 resolve 完），主线程不再需要。
    pub fn merge_task_table(&mut self, task: &mut FuncRefTable) {
        self.line_entries.extend(task.drain_line_entries());
        self.var_entries.extend(task.drain_var_entries());
        for e in task.drain_enum_types() {
            self.add_enum_type(e);
        }
        for rec in task.drain_vtables() {
            let alloc_id = rec.0;
            if let Some((_, old_sym, old_bytes, old_relocs, old_align)) =
                self.vtables.iter().find(|(a, ..)| *a == alloc_id)
            {
                debug_assert_eq!(old_sym, &rec.1, "same alloc_id must have same vtable sym");
                debug_assert_eq!(
                    old_bytes, &rec.2,
                    "same alloc_id must have same vtable bytes"
                );
                debug_assert_eq!(
                    old_relocs, &rec.3,
                    "same alloc_id must have same vtable relocs"
                );
                debug_assert_eq!(
                    old_align, &rec.4,
                    "same alloc_id must have same vtable align"
                );
            } else {
                self.vtables.push(rec);
            }
        }
        for rec in task.drain_promoted() {
            let (alloc_id, sym, bytes, align) = &rec;
            // alloc_id 命中或同内容（bytes+align）命中都视为重复
            // （同内容在内容稳定键命名下符号名必同）。
            let dup_idx = self
                .promoted
                .iter()
                .enumerate()
                .find_map(|(i, (a, _, b, al))| {
                    (a == alloc_id || (b == bytes && al == align)).then_some(i)
                });
            match dup_idx {
                Some(i) => {
                    let (_, old_sym, old_bytes, old_align) = &self.promoted[i];
                    debug_assert_eq!(old_sym, sym, "same promoted content must have same sym");
                    debug_assert_eq!(
                        old_bytes, bytes,
                        "same promoted content must have same bytes"
                    );
                    debug_assert_eq!(
                        old_align, align,
                        "same promoted content must have same align"
                    );
                }
                None => self.promoted.push(rec),
            }
        }
    }
}
