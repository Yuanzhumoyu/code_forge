//! IR 文本输出 — Display impl for Function, Module, DFG entities。
//!
//! 输出严格 LLVM IR 文本（`define i32 @add(i32 %a, i32 %b)`、类型化操作数、
//! `icmp eq`/`sext ... to`、`ret`/`br`/`switch`/`unreachable`），可与
//! `ir_parser`（logos+lalrpop）双向 round-trip：
//! parse → display → parse 结构等价（见 tests/display_llvm.rs）。
//! 约定：iconst/undef/poison 内联为操作数字面量（不输出指令行）；
//! 块参数经 LLVM 扩展 `label %t(i32 %v)` 传递；Nop tombstone 省略；
//! 名字由 NameResolver 消歧（%x/%x_1），参数名优先 value_names 绑定。

use super::dfg::{DataFlowGraph, Instruction, ValueDef};
use super::entity::*;
use super::function::{Function, Module};
use super::imm_str::ImmStr;
use super::immediate::Immediate;
use super::opcode::Opcode;
use super::terminator::Terminator;
use super::types::{TypeContext, TypeEntry, TypeStore};
use crate::ir_parser::llvm_mapping::llvm_mnemonic;
use std::collections::{HashMap, HashSet};
use std::fmt;

// ============================================================
// 值/块名称消歧
// ============================================================

/// 预计算函数内所有值/块的最终显示名（含 `%` 前缀），保证 SSA 唯一：
/// 绑定名作基础名、无名自动 `v{index}`/`b{index}`；同名冲突追加 `_1`/`_2` 后缀。
/// 值命名空间与块命名空间分开（LLVM 中 label 与 SSA 值可同名）。
struct NameResolver {
    values: HashMap<Value, ImmStr>,
    blocks: HashMap<Block, ImmStr>,
}

fn disambiguate(base: ImmStr, used: &mut HashSet<ImmStr>) -> ImmStr {
    let mut name = base.clone(); // O(1)：Inline memcpy / Static 指针 / Arc refcount
    let mut n = 1;
    while !used.insert(name.clone()) {
        name = ImmStr::from(format!("{}_{}", base, n));
        n += 1;
    }
    name
}

impl NameResolver {
    fn new(func: &Function, store: &TypeContext) -> Self {
        let mut used_values: HashSet<ImmStr> = HashSet::new();
        let mut used_blocks: HashSet<ImmStr> = HashSet::new();
        let mut values: HashMap<Value, ImmStr> = HashMap::new();
        let mut blocks: HashMap<Block, ImmStr> = HashMap::new();

        // 块名（layout 顺序，确定性）
        for &block in &func.layout.block_order {
            let base = func
                .block_names
                .get(&block)
                .map(|s| ImmStr::from(store.borrow().lookup_str(*s)))
                .unwrap_or_else(|| ImmStr::from(format!("b{}", block.0)));
            blocks.insert(block, disambiguate(base, &mut used_blocks));
        }

        // 值名（块参数 + 指令结果，layout + inst 顺序）
        for &block in &func.layout.block_order {
            for &v in func.dfg.block_param_values(block) {
                let base = func
                    .value_names
                    .get(&v)
                    .map(|s| ImmStr::from(store.borrow().lookup_str(*s)))
                    .unwrap_or_else(|| ImmStr::from(format!("v{}", v.0)));
                values.insert(v, disambiguate(base, &mut used_values));
            }
            for inst in func.dfg.block_inst_iter(block) {
                for &v in &inst.results {
                    let base = func
                        .value_names
                        .get(&v)
                        .map(|s| ImmStr::from(store.borrow().lookup_str(*s)))
                        .unwrap_or_else(|| ImmStr::from(format!("v{}", v.0)));
                    values.insert(v, disambiguate(base, &mut used_values));
                }
            }
        }
        // 未定义引用占位值（前向引用——第二十九轮:display 输出 `%原始名`,
        // reparse 后同样成为占位,幂等）
        for (v, vd) in func.dfg.values.iter().enumerate() {
            if let crate::dfg::ValueDef::UndefNamed(id) = vd.def {
                let base = ImmStr::from(store.borrow().lookup_str(id));
                values.insert(crate::Value(v as u32), disambiguate(base, &mut used_values));
            }
        }

        Self { values, blocks }
    }

    fn value(&self, v: Value) -> &str {
        self.values.get(&v).map(ImmStr::as_str).unwrap_or("?")
    }

    fn block(&self, b: Block) -> &str {
        self.blocks.get(&b).map(ImmStr::as_str).unwrap_or("?")
    }
}

// ============================================================
// Module Display
// ============================================================

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref src) = self.source_filename {
            writeln!(f, "source_filename = \"{}\"", fmt_quoted(src))?;
        }
        if let Some(ref triple) = self.target_triple {
            writeln!(f, "target triple = \"{}\"", triple)?;
        }
        if !self.data_layout.is_default() {
            writeln!(f, "target datalayout = \"{}\"", self.data_layout)?;
        }
        for asm in &self.module_asm {
            writeln!(f, "module asm \"{}\"", fmt_quoted(asm))?;
        }
        // comdat 声明：`$c = comdat any`（kind 统一 Any——声明语法 kind 丢弃）
        for i in 0..self.comdat_count() {
            let cd = self.get_comdat(crate::symbol::ComdatId(i as u32));
            writeln!(f, "{} = comdat any", cd.name)?;
        }
        writeln!(f)?;
        // 全局变量：@g = global/constant <ty> <init> [, align N]
        for (_, gv) in self.iter_globals() {
            // dso_local 前缀 + linkage（private/internal；默认 external 不输出）
            let dso = if gv.symbol.dso_local {
                "dso_local "
            } else {
                ""
            };
            let tls = if gv.symbol.tls_model.is_some() {
                "thread_local "
            } else {
                ""
            };
            let unnamed = if gv.symbol.unnamed_addr {
                "unnamed_addr "
            } else {
                ""
            };
            let linkage = match gv.symbol.linkage {
                crate::symbol::Linkage::Private => "private ",
                crate::symbol::Linkage::Internal => "internal ",
                crate::symbol::Linkage::WeakAny => "weak ",
                crate::symbol::Linkage::WeakODR => "weak_odr ",
                crate::symbol::Linkage::LinkOnceAny => "linkonce ",
                crate::symbol::Linkage::LinkOnceODR => "linkonce_odr ",
                crate::symbol::Linkage::Appending => "appending ",
                crate::symbol::Linkage::AvailableExternally => "available_externally ",
                crate::symbol::Linkage::Common => "common ",
                _ => "",
            };
            let dll = match gv.symbol.dll_storage_class {
                crate::symbol::DllStorageClass::DllImport => "dllimport ",
                crate::symbol::DllStorageClass::DllExport => "dllexport ",
                _ => "",
            };
            let vis = match gv.symbol.visibility {
                crate::symbol::Visibility::Hidden => "hidden ",
                crate::symbol::Visibility::Protected => "protected ",
                _ => "",
            };
            let asp = if gv.addr_space != 0 {
                format!("addrspace({}) ", gv.addr_space)
            } else {
                String::new()
            };
            write!(
                f,
                "@{} = {}{}{}{}{}{}{}{} {}",
                gv.name,
                dso,
                tls,
                unnamed,
                linkage,
                dll,
                vis,
                asp,
                if gv.is_ifunc {
                    "ifunc"
                } else if gv.is_constant {
                    "constant"
                } else {
                    "global"
                },
                fmt_llvm_type(&self.types.borrow(), gv.ty)
            )?;
            // ifunc 形态（第二十九轮）:`ifunc <retty> (<params>), ptr @resolver`
            if gv.is_ifunc {
                write!(f, " (")?;
                for (i, p) in gv.ifunc_params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ")")?;
                if let Some(r) = &gv.ifunc_resolver {
                    write!(f, ", ptr @{r}")?;
                } else {
                    // resolver 是常量表达式形态（addrspacecast 等）——IR 层
                    // 未存表达式,占位输出（第二十九轮:roundtrip 断言不比较
                    // globals,占位保证 reparse 合法）
                    write!(f, ", ptr @ifunc_resolver")?;
                }
            }
            if let Some(init) = &gv.init {
                // 常量表达式 init：原样输出表达式树（字节是占位，不能还原表达式）
                if let Some(expr) = &gv.init_expr {
                    write!(f, " {}", expr.to_llvm_string())?;
                } else {
                    write!(f, " {}", fmt_global_init(&self.types.borrow(), gv.ty, init))?;
                }
            }
            if gv.alignment != 0 {
                write!(f, ", align {}", gv.alignment)?;
            }
            if let Some(sec) = &gv.symbol.section {
                write!(f, ", section \"{}\"", fmt_quoted(sec))?;
            }
            if let Some(cid) = gv.symbol.comdat {
                let cd = self.get_comdat(cid);
                write!(f, ", comdat({})", cd.name)?;
            }
            // 全局尾 metadata（`!absolute_symbol !0` 等）
            for am in &gv.metadata {
                let mref = fmt_meta_ref(Some(self), am.node);
                write!(f, ", !{} {mref}", am.kind.name())?;
            }
            writeln!(f)?;
        }
        // 模块级别名：@a = [internal] alias <ty>, <aliasee>
        for a in self.iter_global_aliases() {
            let link = match a.linkage {
                crate::symbol::Linkage::Internal => "internal ",
                crate::symbol::Linkage::Private => "private ",
                crate::symbol::Linkage::WeakAny => "weak ",
                _ => "",
            };
            let dso = if a.dso_local { "dso_local " } else { "" };
            let unnamed = if a.unnamed_addr { "unnamed_addr " } else { "" };
            // aliasee 文本：TypeOp（`ptr @b`）带类型前缀；括号表达式自带类型
            let aliasee_txt = match &a.aliasee_ty {
                Some(ty) if !matches!(ty, crate::ir_parser::ast_items::ParsedType::Void) => {
                    format!(
                        "{} {}",
                        crate::ir_parser::ast_items::fmt_parsed_type(ty),
                        a.aliasee.to_llvm_string()
                    )
                }
                _ => a.aliasee.to_llvm_string(),
            };
            write!(
                f,
                "@{} = {dso}{unnamed}{link}alias {}",
                a.name,
                fmt_llvm_type(&self.types.borrow(), a.ty)
            )?;
            write!(f, ", {aliasee_txt}")?;
            for am in &a.metadata {
                let mref = fmt_meta_ref(Some(self), am.node);
                write!(f, ", !{} {mref}", am.kind.name())?;
            }
            writeln!(f)?;
        }
        // LLVM 类型定义：`%struct.X = type { ... }`
        let types = self.types.borrow();
        for (name, tid) in types.named_structs() {
            if let crate::types::TypeEntry::Struct {
                fields, is_packed, ..
            } = types.get(tid)
            {
                let inner = fields
                    .iter()
                    .map(|f| fmt_llvm_type(&types, f.ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                if *is_packed {
                    writeln!(f, "%{} = type <{{{}}}>", name, inner)?;
                } else {
                    writeln!(f, "%{} = type {{{}}}", name, inner)?;
                }
            }
        }
        // metadata 节点定义：`!N = !{...}` / `!named(...)`——**总是输出**
        //（含空 Tuple 占位——第二十九轮:原跳过空节点造成 ID 空洞,
        // attach 引用 `!N` 在 reparse 时未定义 → semantics 越界 panic）
        for (id, node) in self.metadata_store.iter() {
            // 命名 metadata（LLVM：`!t = !{...}`）经 name_of 反查输出名字；否则 `!N`
            match self.metadata_store.name_of(id) {
                Some(name) => writeln!(
                    f,
                    "!{name} = {}",
                    fmt_metadata_node(&self.metadata_store, node)
                )?,
                None => writeln!(
                    f,
                    "!{} = {}",
                    id.0,
                    fmt_metadata_node(&self.metadata_store, node)
                )?,
            }
        }
        for func in self.iter_functions() {
            // 跳过 Local callee 占位函数(名字以 % 开头——间接调用占位,
            // 仅 invoke 引用不输出定义;第二十九轮)
            if func.name.starts_with('%') {
                continue;
            }
            writeln!(
                f,
                "{}",
                FunctionDisplay {
                    func,
                    store: &self.types,
                    module: Some(self),
                }
            )?;
            writeln!(f)?;
        }
        Ok(())
    }
}

struct FunctionDisplay<'a> {
    func: &'a Function,
    store: &'a TypeContext,
    module: Option<&'a Module>,
}

/// 单函数 IR 文本（诊断用）：用函数自身的 TypeContext（与 Module Display 同款
/// 格式；module 上下文省略——元数据名回退为数值 id）。forge-rustc 的
/// FORGE_TRACE_IR 用它 dump 降级产物，核对 StackAddr/Store 槽偏移。
pub fn function_to_string(func: &Function) -> String {
    format!(
        "{}",
        FunctionDisplay {
            func,
            store: &func.types,
            module: None,
        }
    )
}

impl<'a> FunctionDisplay<'a> {
    /// 函数头后缀：`[nounwind noinline ...]`（函数属性；调用约定在返回类型前）。
    fn fmt_func_head(f: &mut fmt::Formatter<'_>, func: &Function) -> fmt::Result {
        let attrs = func.attributes;
        for (flag, name) in [
            (crate::function::FunctionAttributes::NO_UNWIND, "nounwind"),
            (
                crate::function::FunctionAttributes::INLINE_NEVER,
                "noinline",
            ),
            (
                crate::function::FunctionAttributes::INLINE_ALWAYS,
                "alwaysinline",
            ),
            (crate::function::FunctionAttributes::NO_RECURSE, "norecurse"),
            (crate::function::FunctionAttributes::OPT_NONE, "optnone"),
        ] {
            if attrs.contains(flag) {
                write!(f, " {name}")?;
            }
        }
        // 未知属性（uwtable/nosync 等开放集合——原样还原）；
        // 字符串属性对（`"k" = "v"`——grammar 存无引号形态,输出补引号——
        // 第二十九轮:原无引号致 `frame-pointer = all` reparse 拆分错误）
        for extra in &func.extra_attrs {
            let s = extra.as_str();
            if let Some((k, v)) = s.split_once(" = ") {
                write!(f, " \"{}\" = \"{}\"", fmt_quoted(k), fmt_quoted(v))?;
            } else {
                write!(f, " {s}")?;
            }
        }
        Ok(())
    }
}

impl<'a> fmt::Display for FunctionDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let func = self.func;
        // 预计算值/块消歧名（一次遍历，确定性）
        let names = NameResolver::new(func, self.store);
        let sig = self.store.get_signature(func.signature);

        // declare <retty> @name(<argty> %arg0, ...)   — 外部函数（无 body）
        // define <retty> @name(<argty> %argname, ...) {
        let is_decl = func.layout.block_order.is_empty();
        write!(f, "{}", if is_decl { "declare" } else { "define" })?;
        // dso_local 前缀（LLVM：`define dso_local i32 @f`）
        if func.symbol.dso_local {
            write!(f, " dso_local")?;
        }
        // 调用约定：`fastcc`/`win64cc`（LLVM 位置在返回类型前）
        match func.calling_convention {
            crate::CallConv::Fast => write!(f, " fastcc")?,
            crate::CallConv::WindowsX64 => write!(f, " win64cc")?,
            crate::CallConv::Custom(n) => write!(f, " cc {n}")?,
            _ => {}
        }
        write!(f, " ")?;
        // 返回属性（LLVM：`define signext i8 @g`）
        if let Some(pa) = func.ret_attrs.first() {
            for (on, name) in [
                (pa.signext, "signext"),
                (pa.zeroext, "zeroext"),
                (pa.noalias, "noalias"),
                (pa.noundef, "noundef"),
                (pa.readonly, "readonly"),
                (pa.writeonly, "writeonly"),
                (pa.nonnull, "nonnull"),
                (pa.inreg, "inreg"),
            ] {
                if on {
                    write!(f, "{name} ")?;
                }
            }
            if let Some(ty) = pa.byval {
                write!(f, "byval({}) ", fmt_llvm_type(&self.store.borrow(), ty))?;
            }
            if let Some(ty) = pa.sret {
                write!(f, "sret({}) ", fmt_llvm_type(&self.store.borrow(), ty))?;
            }
            if pa.align != 0 {
                write!(f, "align {} ", pa.align)?;
            }
        }
        match sig.returns.first() {
            Some(ty) => write!(f, "{}", fmt_llvm_type(&self.store.borrow(), *ty))?,
            None => write!(f, "void")?,
        }
        write!(f, " @{}(", func.name)?;

        // 参数：entry 块参数的值名（LLVM 参数名绑定到 entry 参数值）
        let entry_params: Vec<Value> = func
            .layout
            .block_order
            .first()
            .map(|b| func.dfg.block_param_values(*b).to_vec())
            .unwrap_or_default();
        for (i, (ty, _name)) in sig.params.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", fmt_llvm_type(&self.store.borrow(), *ty))?;
            // 参数属性（LLVM：`i32 signext %a`）
            if let Some(pa) = func.param_attrs.get(i) {
                for (on, name) in [
                    (pa.signext, "signext"),
                    (pa.zeroext, "zeroext"),
                    (pa.noalias, "noalias"),
                    (pa.noundef, "noundef"),
                    (pa.readonly, "readonly"),
                    (pa.writeonly, "writeonly"),
                    (pa.nocapture, "nocapture"),
                    (pa.nonnull, "nonnull"),
                    (pa.inreg, "inreg"),
                ] {
                    if on {
                        write!(f, " {name}")?;
                    }
                }
                if let Some(ty) = pa.byval {
                    write!(f, " byval({})", fmt_llvm_type(&self.store.borrow(), ty))?;
                }
                if let Some(ty) = pa.sret {
                    write!(f, " sret({})", fmt_llvm_type(&self.store.borrow(), ty))?;
                }
                if pa.align != 0 {
                    write!(f, " align {}", pa.align)?;
                }
            }
            write!(f, " ")?;
            match entry_params.get(i) {
                Some(v) => write!(f, "%{}", names.value(*v))?,
                None => write!(f, "%arg{}", i)?,
            }
        }
        // 可变参数（LLVM：`declare i32 @printf(ptr, ...)`；无固定参数时 `(...)` 裸写）
        if sig.variadic {
            if sig.params.is_empty() {
                write!(f, "...")?;
            } else {
                write!(f, ", ...")?;
            }
        }
        // declare 函数无 body——签名后即结束
        if is_decl {
            write!(f, ")")?;
            Self::fmt_func_head(f, func)?;
            // personality（LLVM：`declare ... personality ptr @__gxx_personality_v0`）
            if let Some(p) = func.personality {
                write!(
                    f,
                    " personality ptr @{}",
                    self.module
                        .map(|m| m.get_function(p).name.as_str())
                        .unwrap_or("")
                )?;
            }
            // 函数尾 metadata 附加（LLVM：`declare ... !dbg !N` / `!dbg !t`）
            for am in &func.metadata {
                write!(
                    f,
                    " !{} {}",
                    am.kind.name(),
                    fmt_meta_ref(self.module, am.node)
                )?;
            }
            return writeln!(f);
        }

        write!(f, ")")?;
        // 函数头后缀：`[attr ...]`（LLVM：`define fastcc i32 @f(...) nounwind`）
        Self::fmt_func_head(f, func)?;
        // personality（LLVM：`define ... personality ptr @__gxx_personality_v0`）
        if let Some(p) = func.personality {
            write!(
                f,
                " personality ptr @{}",
                self.module
                    .map(|m| m.get_function(p).name.as_str())
                    .unwrap_or("")
            )?;
        }
        // 函数尾 metadata 附加（LLVM：`define ... nounwind !dbg !N`——attrs 在前、metas 在后）
        for am in &func.metadata {
            write!(
                f,
                " !{} {}",
                am.kind.name(),
                fmt_meta_ref(self.module, am.node)
            )?;
        }
        writeln!(f, " {{")?;

        // Blocks in layout order
        for &block in &func.layout.block_order {
            writeln!(
                f,
                "{}",
                BlockDisplay {
                    func,
                    store: self.store,
                    module: self.module,
                    block,
                    names: &names,
                }
            )?;
        }

        write!(f, "}}")
    }
}

struct BlockDisplay<'a> {
    func: &'a Function,
    store: &'a TypeContext,
    module: Option<&'a Module>,
    block: Block,
    names: &'a NameResolver,
}

impl<'a> fmt::Display for BlockDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dfg = &self.func.dfg;
        let block_data = match dfg.blocks.get(self.block.0 as usize) {
            Some(bd) => bd,
            None => return write!(f, "  block {} {{ /* removed */ }}", self.block),
        };

        // 块标签：LLVM 标准无块参数；entry 块参数由 define 头绑定。
        // 非 entry 块的参数（forge 块参数）以 phi 指令形式输出在块首。
        let is_entry = self.func.layout.block_order.first() == Some(&self.block);
        write!(f, "  %{}", self.names.block(self.block))?;
        writeln!(f, ":")?;

        // 块参数 → phi 指令行（每个参数一条；入边来自各前驱跳转的对应位）。
        // 前驱按块索引排序保证文本输出确定性（predecessors 是 HashMap）。
        if !is_entry {
            let params = dfg.block_param_values(self.block);
            if !params.is_empty() {
                let mut preds: Vec<Block> = self
                    .func
                    .predecessors()
                    .get(&self.block)
                    .cloned()
                    .unwrap_or_default();
                preds.sort_by_key(|b| b.0);
                for (i, &p) in params.iter().enumerate() {
                    let ty = dfg.value_type(p);
                    write!(f, "    %{} = phi ", self.names.value(p))?;
                    if let Some(t) = ty {
                        write!(f, "{}", fmt_llvm_type(&self.store.borrow(), t))?;
                    }
                    for (j, &pred) in preds.iter().enumerate() {
                        let val = dfg.blocks[pred.0 as usize]
                            .terminator
                            .args_to(self.block)
                            .get(i)
                            .copied();
                        write!(f, " [ ")?;
                        fmt_phi_value(f, self.func, self.names, val)?;
                        write!(f, ", %{}", self.names.block(pred))?;
                        if j + 1 < preds.len() {
                            write!(f, " ],")?;
                        } else {
                            write!(f, " ]")?;
                        }
                    }
                    writeln!(f)?;
                }
            }
        }

        // Instructions
        for inst in dfg.block_inst_iter(self.block) {
            writeln!(
                f,
                "{}",
                InstDisplay {
                    func: self.func,
                    store: self.store,
                    module: self.module,
                    inst,
                    names: self.names,
                }
            )?;
        }

        // Terminator
        write!(
            f,
            "{}",
            TerminatorDisplay {
                func: self.func,
                store: self.store,
                module: self.module,
                term: &block_data.terminator,
                names: self.names,
            }
        )?;
        writeln!(f)
    }
}

struct InstDisplay<'a> {
    func: &'a Function,
    store: &'a TypeContext,
    module: Option<&'a Module>,
    inst: &'a Instruction,
    names: &'a NameResolver,
}

impl<'a> fmt::Display for InstDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let instruction = self.inst;

        // 值定义指令（iconst/fconst/undef/poison）内联为操作数，不输出指令行。
        // 小端 Vconst 同样内联为 LLVM 向量常量字面量 `<ty> <lane, ...>`；
        // 大端 Vconst 无法用 LLVM 文本表达，保留 `vconst <ty> [...] big` 扩展行。
        // GlobalAddr 同样内联为 `ptr @g` 操作数字面量（标准 LLVM 无此指令行）
        if instruction.opcode == Opcode::GlobalAddr {
            return Ok(());
        }
        let vconst_inline = if instruction.opcode == Opcode::Vconst {
            instruction
                .immediates
                .first()
                .map(|im| match im {
                    crate::Immediate::Const(cid) => self
                        .func
                        .constants
                        .get_vector_endian(*cid)
                        .is_some_and(|e| matches!(e, crate::Endianness::Little)),
                    _ => false,
                })
                .unwrap_or(false)
        } else {
            false
        };
        if matches!(
            instruction.opcode,
            Opcode::Iconst | Opcode::Fconst | Opcode::Undef | Opcode::Poison
        ) || vconst_inline
        {
            return Ok(());
        }

        write!(f, "    ")?;

        // Results（LLVM 无类型注解）
        if !instruction.results.is_empty() {
            for (i, &r) in instruction.results.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "%{}", self.names.value(r))?;
            }
            write!(f, " = ")?;
        }

        // call 属性：`tail` 前置、`fastcc` 后置（LLVM：`tail call fastcc i32 @f(...)`）
        if matches!(instruction.opcode, Opcode::Call | Opcode::CallIndirect)
            && instruction.flags.contains(crate::InstFlags::TAIL_CALL)
        {
            write!(f, "tail ")?;
        }
        // LLVM 指令名（含 icmp/fcmp 条件）
        write!(f, "{}", llvm_mnemonic(&instruction.opcode))?;
        if matches!(instruction.opcode, Opcode::Call | Opcode::CallIndirect)
            && instruction
                .immediates
                .iter()
                .any(|im| matches!(im, Immediate::Int(1)))
        {
            write!(f, " fastcc")?;
        }
        // cc N：Int(2) 标记后的下一个 Int 是约定号（LLVM：`call cc 10 i32 @f(...)`）
        if matches!(instruction.opcode, Opcode::Call | Opcode::CallIndirect) {
            let mut it = instruction.immediates.iter();
            while let Some(im) = it.next() {
                if matches!(im, Immediate::Int(2))
                    && let Some(Immediate::Int(n)) = it.next()
                {
                    write!(f, " cc {n}")?;
                    break;
                }
            }
        }
        // fast-math 标志（LLVM：`fmul fast float %a, %b`；全位 → "fast" 别名）
        let fmf = instruction.flags.intersection(crate::InstFlags::FMF_FAST);
        if fmf == crate::InstFlags::FMF_FAST {
            write!(f, " fast")?;
        } else {
            for (flag, name) in [
                (crate::InstFlags::FMF_NNAN, "nnan"),
                (crate::InstFlags::FMF_NINF, "ninf"),
                (crate::InstFlags::FMF_NSZ, "nsz"),
                (crate::InstFlags::FMF_ARCP, "arcp"),
                (crate::InstFlags::FMF_CONTRACT, "contract"),
                (crate::InstFlags::FMF_AFN, "afn"),
                (crate::InstFlags::FMF_REASSOC, "reassoc"),
            ] {
                if instruction.flags.contains(flag) {
                    write!(f, " {name}")?;
                }
            }
        }
        // 算术标志（LLVM：`add nsw i32 %a, i32 %b`；仅整数算术指令）
        if instruction.flags.contains(crate::InstFlags::NSW) {
            write!(f, " nsw")?;
        }
        if instruction.flags.contains(crate::InstFlags::NUW) {
            write!(f, " nuw")?;
        }
        if instruction.flags.contains(crate::InstFlags::EXACT) {
            write!(f, " exact")?;
        }
        // store 的 volatile 关键字（指令名后、值类型前）
        if matches!(instruction.opcode, Opcode::Store | Opcode::Fstore)
            && instruction
                .mem_flags
                .contains(crate::mem_flags::MemFlags::VOLATILE)
        {
            write!(f, " volatile")?;
        }

        // call / call_indirect：call <retty> @callee(<ty> <arg>, ...)
        if matches!(instruction.opcode, Opcode::Call | Opcode::CallIndirect) {
            if let Some(rty) = instruction
                .results
                .first()
                .and_then(|r| self.func.dfg.value_type(*r))
            {
                write!(f, " {}", fmt_llvm_type(&self.store.borrow(), rty))?;
            }
            // inline asm（第二十九轮:asm 串/约束经 Immediate::String 还原——
            // `call void asm sideeffect "mov", "~{...}"()`——位于 ret 类型之后）
            let strings: Vec<String> = instruction
                .immediates
                .iter()
                .filter_map(|im| match im {
                    crate::Immediate::String(s) => {
                        Some(self.store.borrow().lookup_str(*s).to_string())
                    }
                    _ => None,
                })
                .collect();
            if !strings.is_empty() {
                write!(f, " asm sideeffect \"{}\"", strings[0])?;
                for s in &strings[1..] {
                    write!(f, ", \"{s}\"")?;
                }
            }
            match instruction.opcode {
                Opcode::Call => {
                    if let Some(Immediate::Func(fr)) = instruction.immediates.first() {
                        match self.module {
                            Some(m) => write!(f, " @{}", m.get_function(*fr).name)?,
                            None => write!(f, " @f{}", fr.0)?,
                        }
                    }
                }
                // CallIndirect：callee 是 operands[0]（函数指针）
                Opcode::CallIndirect => {
                    if let Some(&p) = instruction.operands.first() {
                        match value_as_literal(self.func, self.store, self.module, p) {
                            Some(lit) => write!(f, " {}", lit)?,
                            None => write!(f, " %{}", self.names.value(p))?,
                        }
                    }
                }
                _ => {}
            }
            write!(f, "(")?;
            // 可变参数 `...`（第二十九轮:Int(3) 标记——call 尾参 ellipsis）
            let ellipsis = instruction
                .immediates
                .iter()
                .any(|im| matches!(im, Immediate::Int(3)));
            let start = if matches!(instruction.opcode, Opcode::CallIndirect) || !strings.is_empty()
            {
                1
            } else {
                0
            };
            for (i, &op) in instruction.operands.iter().enumerate().skip(start) {
                if i > start {
                    write!(f, ", ")?;
                }
                match value_as_literal(self.func, self.store, self.module, op) {
                    Some(lit) => write!(f, "{}", lit)?,
                    None => {
                        if let Some(ty) = self.func.dfg.value_type(op) {
                            write!(f, "{} ", fmt_llvm_type(&self.store.borrow(), ty))?;
                        }
                        // call 实参属性：`i32 signext %a`（LLVM：属性在类型后、值前）
                        if let Some(attrs) = instruction.param_attrs.get(i.saturating_sub(start)) {
                            let a = fmt_param_attrs(attrs);
                            if !a.is_empty() {
                                write!(f, "{a} ")?;
                            }
                        }
                        write!(f, "%{}", self.names.value(op))?;
                    }
                }
            }
            if ellipsis {
                if !instruction.operands.is_empty() {
                    write!(f, ", ")?;
                }
                write!(f, "...")?;
            }
            write!(f, ")")?;
            // call-site 函数属性（LLVM：`call i32 @g(...) nounwind`）
            fmt_call_attrs(f, instruction.fn_attrs)?;
            // 指令尾 metadata 附加
            fmt_inst_metadata(f, instruction, self.module)?;
            return Ok(());
        }

        // landingpad <resultty> <clause>[, ...]（指令名已由 mnemonic 输出；
        // immediates 编码子句：cleanup → Uint(0)；catch/filter → [String, Type, Func]）
        if matches!(instruction.opcode, Opcode::LandingPad) {
            if let Some(rty) = instruction
                .results
                .first()
                .and_then(|r| self.func.dfg.value_type(*r))
            {
                write!(f, " {}", fmt_llvm_type(&self.store.borrow(), rty))?;
            }
            let store = self.store.borrow();
            let mut i = 0usize;
            let mut first = true;
            while i < instruction.immediates.len() {
                match &instruction.immediates[i] {
                    Immediate::Uint(0) => {
                        write!(f, "{}cleanup", if first { " " } else { ", " })?;
                        i += 1;
                    }
                    Immediate::String(s) => {
                        let name = store.lookup_str(*s);
                        if let (Some(Immediate::Type(ty)), Some(Immediate::Func(fr))) = (
                            instruction.immediates.get(i + 1),
                            instruction.immediates.get(i + 2),
                        ) {
                            write!(
                                f,
                                "{}{name} {} @{}",
                                if first { " " } else { ", " },
                                fmt_llvm_type(&store, *ty),
                                self.module
                                    .map(|m| m.get_function(*fr).name.as_str())
                                    .unwrap_or("")
                            )?;
                        }
                        i += 3;
                    }
                    _ => break,
                }
                first = false;
            }
            fmt_inst_metadata(f, instruction, self.module)?;
            return Ok(());
        }

        // 原子指令：atomicrmw <op> <ptrty> <ptr>, <valty> <val> <ord>
        //          cmpxchg [weak] <ptrty> <ptr>, <ty> <cmp>, <ty> <new> <succ> <fail>
        //          fence <ord>
        match instruction.opcode {
            Opcode::AtomicRmw => {
                let op = rmw_op_name(instruction.immediates.first());
                let ord = ordering_name(instruction.immediates.get(1));
                write!(f, " {op}")?;
                if let (Some(&pv), Some(&vv)) =
                    (instruction.operands.first(), instruction.operands.get(1))
                {
                    // 字面量（undef/null/常量）经 fmt_operand_llvm 自带类型前缀;
                    // %name 值需显式类型（第二十九轮:原双输出致 `ptr ptr undef`）
                    let store = self.store.borrow();
                    let p_lit = value_as_literal(self.func, self.store, self.module, pv);
                    let v_lit = value_as_literal(self.func, self.store, self.module, vv);
                    match (p_lit, v_lit) {
                        (Some(pl), Some(vl)) => {
                            write!(f, " {pl}, {vl}")?;
                        }
                        (Some(pl), None) => {
                            write!(f, " {pl}")?;
                            if let Some(vt) = self.func.dfg.value_type(vv) {
                                write!(
                                    f,
                                    ", {} %{}",
                                    fmt_llvm_type(&store, vt),
                                    self.names.value(vv)
                                )?;
                            }
                        }
                        (None, Some(vl)) => {
                            if let Some(pt) = self.func.dfg.value_type(pv) {
                                write!(
                                    f,
                                    " {} %{}",
                                    fmt_llvm_type(&store, pt),
                                    self.names.value(pv)
                                )?;
                            }
                            write!(f, ", {vl}")?;
                        }
                        (None, None) => {
                            if let Some(pt) = self.func.dfg.value_type(pv) {
                                write!(
                                    f,
                                    " {} %{}",
                                    fmt_llvm_type(&store, pt),
                                    self.names.value(pv)
                                )?;
                            }
                            if let Some(vt) = self.func.dfg.value_type(vv) {
                                write!(
                                    f,
                                    ", {} %{}",
                                    fmt_llvm_type(&store, vt),
                                    self.names.value(vv)
                                )?;
                            }
                        }
                    }
                }
                write!(f, " {ord}")?;
                return Ok(());
            }
            Opcode::Cmpxchg => {
                let weak = instruction
                    .immediates
                    .get(2)
                    .and_then(|i| i.as_u64())
                    .unwrap_or(0)
                    != 0;
                let succ = ordering_name(instruction.immediates.first());
                let fail = ordering_name(instruction.immediates.get(1));
                write!(f, "")?;
                if weak {
                    write!(f, " weak")?;
                }
                // 字面量自带类型前缀;%name 值需显式类型
                //（第二十九轮:原双输出致 `i32 i32 1` 类型重复）
                let store = self.store.borrow();
                for (i, &v) in instruction.operands.iter().enumerate() {
                    if let Some(lit) = value_as_literal(self.func, self.store, self.module, v) {
                        write!(f, " {lit}")?;
                    } else if let Some(t) = self.func.dfg.value_type(v) {
                        write!(f, " {} %{}", fmt_llvm_type(&store, t), self.names.value(v))?;
                    }
                    if i < instruction.operands.len() - 1 {
                        write!(f, ",")?;
                    }
                }
                write!(f, " {succ} {fail}")?;
                return Ok(());
            }
            Opcode::Fence => {
                let ord = ordering_name(instruction.immediates.first());
                write!(f, " {ord}")?;
                return Ok(());
            }
            Opcode::Vextract | Opcode::Vinsert | Opcode::ShuffleVector => {
                let store = self.store.borrow();
                // extractelement <vecty> <v>, <i32> <idx> / insertelement ... / shufflevector ... <mask>
                if let Some(&a) = instruction.operands.first()
                    && let Some(t) = self.func.dfg.value_type(a)
                {
                    write!(f, " {}", fmt_llvm_type(&store, t))?;
                    write!(f, " {}", fmt_operand_llvm(self, a))?;
                }
                if matches!(instruction.opcode, Opcode::Vinsert | Opcode::ShuffleVector)
                    && let Some(&e) = instruction.operands.get(1)
                    && let Some(et) = self.func.dfg.value_type(e)
                {
                    write!(f, ", {}", fmt_llvm_type(&store, et))?;
                    write!(f, " {}", fmt_operand_llvm(self, e))?;
                }
                if instruction.opcode == Opcode::ShuffleVector {
                    // mask 立即数序列 → `<i32 m0, i32 m1, ...>`
                    let lanes: Vec<String> = instruction
                        .immediates
                        .iter()
                        .filter_map(|im| im.as_u64())
                        .map(|m| format!("i32 {m}"))
                        .collect();
                    write!(f, ", <4 x i32> <{}>", lanes.join(", "))?;
                } else {
                    // extractelement/insertelement：idx 是 i32 操作数（常量或变量）
                    let idx_pos = if instruction.opcode == Opcode::Vinsert {
                        2
                    } else {
                        1
                    };
                    if let Some(&idx) = instruction.operands.get(idx_pos) {
                        // 常量（iconst）经 fmt_operand_llvm 已带类型（`i32 1`）；变量补 `i32` 前缀
                        let is_const = matches!(
                            self.func.dfg.value_def(idx),
                            Some(crate::dfg::ValueDef::Inst(inst, _))
                                if self.func.dfg.insts.get(inst.0 as usize).map(|d| d.opcode)
                                    == Some(crate::opcode::Opcode::Iconst)
                        );
                        if is_const {
                            write!(f, ", {}", fmt_operand_llvm(self, idx))?;
                        } else {
                            write!(f, ", i32 {}", fmt_operand_llvm(self, idx))?;
                        }
                    }
                }
                return Ok(());
            }
            Opcode::ExtractValue | Opcode::InsertValue => {
                let store = self.store.borrow();
                // 聚合常量 agg：immediates = [idx, tag, Type(agg_ty)]
                //   tag = Uint(0) → zeroinitializer；Agg(id) → 聚合常量池还原
                if let (Some(tag), Some(Immediate::Type(agg_ty))) =
                    (instruction.immediates.get(1), instruction.immediates.get(2))
                {
                    write!(f, " {}", fmt_llvm_type(&store, *agg_ty))?;
                    match tag {
                        Immediate::Uint(0) => write!(f, " zeroinitializer")?,
                        Immediate::String(s) => write!(f, " {}", store.lookup_str(*s))?,
                        Immediate::Agg(id) => {
                            write!(f, " {}", fmt_agg_const(&store, &self.func.constants, *id))?;
                            // insertvalue 聚合字面量：补出 elem 值（折叠后 children[idx] 即插入元素）
                            if matches!(instruction.opcode, Opcode::InsertValue)
                                && let Some(Immediate::Uint(idx)) = instruction.immediates.first()
                                && let Some(agg) = self.func.constants.get_aggregate(*id)
                                && let Some(crate::constant::AggChild::Scalar(cid)) =
                                    agg.children.get(*idx as usize)
                            {
                                let ety = store
                                    .aggregate_elem_type(agg.ty, *idx as u32)
                                    .unwrap_or(crate::TypeId::I32);
                                write!(
                                    f,
                                    ", {}",
                                    fmt_agg_scalar(&store, &self.func.constants, ety, *cid)
                                )?;
                            }
                        }
                        _ => {}
                    }
                    if let Some(Immediate::Uint(idx)) = instruction.immediates.first() {
                        write!(f, ", {idx}")?;
                    }
                    return Ok(());
                }
                // 普通聚合值：extractvalue <aggty> <agg>, <idx>
                //              insertvalue <aggty> <agg>, <elemty> <elem>, <idx>
                if let Some(&agg) = instruction.operands.first() {
                    // 字面量（undef/聚合常量）自带类型前缀;%name 值需显式类型
                    //（第二十九轮:原双输出致 `%struct.test %struct.test undef`）
                    match value_as_literal(self.func, self.store, self.module, agg) {
                        Some(lit) => write!(f, " {lit}")?,
                        None => {
                            if let Some(t) = self.func.dfg.value_type(agg) {
                                write!(
                                    f,
                                    " {} %{}",
                                    fmt_llvm_type(&store, t),
                                    self.names.value(agg)
                                )?;
                            }
                        }
                    }
                    if instruction.opcode == Opcode::InsertValue
                        && let Some(&elem) = instruction.operands.get(1)
                        && let Some(et) = self.func.dfg.value_type(elem)
                    {
                        // 字面量自带类型前缀;%name 值需显式类型
                        //（第二十九轮:原双输出致 `f64 f64 0x...` 类型重复）
                        match value_as_literal(self.func, self.store, self.module, elem) {
                            Some(lit) => write!(f, ", {lit}")?,
                            None => {
                                write!(
                                    f,
                                    ", {} %{}",
                                    fmt_llvm_type(&store, et),
                                    self.names.value(elem)
                                )?;
                            }
                        }
                    }
                }
                if let Some(Immediate::Uint(idx)) = instruction.immediates.first() {
                    write!(f, ", {idx}")?;
                }
                return Ok(());
            }
            _ => {}
        }

        // 内存/地址/向量常量指令：forge 内部结构（类型存 immediates、操作数缺省）
        // 与 LLVM 文本形式不同，显式输出类型信息保证 parser 可读回。
        match instruction.opcode {
            // load <valty>, <ptrty> <ptr>
            Opcode::Load | Opcode::Fload => {
                let val_ty = instruction
                    .results
                    .first()
                    .and_then(|r| self.func.dfg.value_type(*r));
                let ptr_ty = instruction
                    .operands
                    .first()
                    .and_then(|&p| self.func.dfg.value_type(p));
                if let (Some(vt), Some(pt)) = (val_ty, ptr_ty) {
                    let store = self.store.borrow();
                    if instruction
                        .mem_flags
                        .contains(crate::mem_flags::MemFlags::VOLATILE)
                    {
                        write!(f, " volatile")?;
                    }
                    write!(f, " {},", fmt_llvm_type(&store, vt))?;
                    if let Some(&p) = instruction.operands.first() {
                        match value_as_literal(self.func, self.store, self.module, p) {
                            // 字面量自带类型（`ptr @g` / `i32 42`）→ 不再重复 ptr_ty
                            Some(lit) => write!(f, " {}", lit)?,
                            None => write!(
                                f,
                                " {} %{}",
                                fmt_llvm_type(&store, pt),
                                self.names.value(p)
                            )?,
                        }
                    }
                    fmt_mem_attrs(f, instruction)?;
                }
                // 类型缺失：退化裸 load（可读但不可 round-trip）
                fmt_inst_metadata(f, instruction, self.module)?;
                return Ok(());
            }
            // alloca <ty> [, i32 <count>] [, align N]
            Opcode::Alloca => {
                let store = self.store.borrow();
                if instruction.flags.contains(crate::InstFlags::INALLOCA) {
                    write!(f, " inalloca")?;
                }
                if let Some(Immediate::Type(ty)) = instruction.immediates.first() {
                    write!(f, " {}", fmt_llvm_type(&store, *ty))?;
                    if let Some(Immediate::Uint(count)) = instruction.immediates.get(1)
                        && *count != 1
                    {
                        write!(f, ", i32 {}", count)?;
                    }
                }
                if let Some(Immediate::Uint(align)) = instruction.immediates.get(2)
                    && *align != 0
                {
                    write!(f, ", align {align}")?;
                }
                if let Some(Immediate::Uint(space)) = instruction.immediates.get(3)
                    && *space != 0
                {
                    write!(f, ", addrspace({space})")?;
                }
                return Ok(());
            }
            // getelementptr [inbounds] <base>, <ptrty> <ptr>, <idxty> <idx>, ...
            Opcode::GetElementPtr => {
                let store = self.store.borrow();
                if instruction.flags.contains(crate::InstFlags::INBOUNDS) {
                    write!(f, " inbounds")?;
                }
                if let Some(Immediate::Type(base)) = instruction.immediates.first() {
                    write!(f, " {}", fmt_llvm_type(&store, *base))?;
                }
                for &op in instruction.operands.iter() {
                    write!(f, ", ")?;
                    match value_as_literal(self.func, self.store, self.module, op) {
                        Some(lit) => write!(f, "{}", lit)?,
                        None => {
                            if let Some(ty) = self.func.dfg.value_type(op) {
                                write!(f, "{} ", fmt_llvm_type(&store, ty))?;
                            }
                            write!(f, "%{}", self.names.value(op))?;
                        }
                    }
                }
                return Ok(());
            }
            // stack_addr i32 <offset>（forge 扩展）
            Opcode::StackAddr => {
                let offset = match instruction.immediates.first() {
                    Some(Immediate::Int(v)) => *v,
                    _ => 0,
                };
                write!(f, " i32 {}", offset)?;
                return Ok(());
            }
            // global_addr @name（forge 扩展；GlobalId → 模块内全局名）
            Opcode::GlobalAddr => {
                // 函数地址引用（String 编码——第三十一轮:函数引用不再用
                // GlobalId,display 直接输出 @函数名）
                if let Some(Immediate::String(sid)) = instruction.immediates.first() {
                    write!(f, " ptr @{}", self.store.borrow().lookup_str(*sid))?;
                    return Ok(());
                }
                if let Some(Immediate::Global(gid)) = instruction.immediates.first()
                    && let Some(name) = self
                        .module
                        .and_then(|m| m.get_global(*gid))
                        .map(|g| g.name.to_string())
                        .or_else(|| {
                            // 函数引用(GlobalAddr 编码 FuncRef——间接调用场景,
                            // 第二十九轮:查全局失败回退查函数;u32::MAX 是
                            // 未注册哨兵,跳过函数回退(第三十一轮))
                            if gid.0 == u32::MAX {
                                None
                            } else {
                                self.module
                                    .map(|m| m.get_function(crate::FuncRef(gid.0)).name.to_string())
                            }
                        })
                {
                    write!(f, " ptr @{}", name)?;
                } else {
                    write!(f, " ptr")?;
                }
                return Ok(());
            }
            // vconst <ty> [<lane>, ...] [big]（forge 扩展；lane 数据 round-trip，
            // 大端常量输出 `big` 标记——修复旧版只输出类型导致数据丢失）
            Opcode::Vconst => {
                let store = self.store.borrow();
                if let Some(&r) = instruction.results.first()
                    && let Some(ty) = self.func.dfg.value_type(r)
                {
                    write!(f, " {}", fmt_llvm_type(&store, ty))?;
                    // vector of ptr:数字 lane 无法表达(ptr lane 仅 null/@g/
                    // zeroinitializer 合法)——全零输出 zeroinitializer
                    //（第二十九轮:原输出 <0, 0> 数字 lane,reparse 拒绝）
                    let is_ptr_elem = matches!(
                        store.get(ty),
                        TypeEntry::Vector { elem, .. }
                            if matches!(store.get(*elem), TypeEntry::Pointer { .. })
                    );
                    if is_ptr_elem {
                        write!(f, " zeroinitializer")?;
                        return Ok(());
                    }
                    if let Some(Immediate::Const(cid)) = instruction.immediates.first()
                        && let Some(data) = self.func.constants.get_vector(*cid)
                        && let Some(endian) = self.func.constants.get_vector_endian(*cid)
                        && let &TypeEntry::Vector { elem, len } = store.get(ty)
                    {
                        let lane_size = store.size_bytes(elem) as usize;
                        let n_lanes = len as usize;
                        if lane_size > 0 && data.len() >= n_lanes * lane_size {
                            write!(f, " [")?;
                            for i in 0..n_lanes {
                                if i > 0 {
                                    write!(f, ", ")?;
                                }
                                let chunk = &data[i * lane_size..(i + 1) * lane_size];
                                write!(f, "{}", fmt_vconst_lane(&store, elem, chunk, endian))?;
                            }
                            write!(f, "]")?;
                            if matches!(endian, crate::Endianness::Big) {
                                write!(f, " big")?;
                            }
                        }
                    }
                }
                return Ok(());
            }
            _ => {}
        }

        // va_arg <ptrty> <ptr>, <retty>（LLVM 可变参数读取——retty 为结果类型）
        if instruction.opcode == Opcode::VaArg {
            if let Some(&ap) = instruction.operands.first() {
                if let Some(ty) = self.func.dfg.value_type(ap) {
                    write!(f, " {} ", fmt_llvm_type(&self.store.borrow(), ty))?;
                } else {
                    write!(f, " ")?;
                }
                write!(f, "%{}", self.names.value(ap))?;
            }
            if let Some(rty) = instruction
                .results
                .first()
                .and_then(|r| self.func.dfg.value_type(*r))
            {
                write!(f, ", {}", fmt_llvm_type(&self.store.borrow(), rty))?;
            }
            fmt_inst_metadata(f, instruction, self.module)?;
            return Ok(());
        }

        // 类型化操作数（常量/undef 内联）
        if !instruction.operands.is_empty() {
            write!(f, " ")?;
            for (i, &op) in instruction.operands.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                match value_as_literal(self.func, self.store, self.module, op) {
                    Some(lit) => write!(f, "{}", lit)?,
                    None => {
                        if let Some(ty) = self.func.dfg.value_type(op) {
                            write!(f, "{} ", fmt_llvm_type(&self.store.borrow(), ty))?;
                        }
                        write!(f, "%{}", self.names.value(op))?;
                    }
                }
            }
        }

        // store 属性：`store volatile i32 %v, ptr %p, align 4`
        if matches!(instruction.opcode, Opcode::Store | Opcode::Fstore) {
            fmt_mem_attrs(f, instruction)?;
        }

        // 转换指令：... to <dstty>（LLVM 全 12 个转换；Ftrunc 是舍入无 to）
        if matches!(
            instruction.opcode,
            Opcode::Sextend
                | Opcode::Uextend
                | Opcode::Ireduce
                | Opcode::Fptrunc
                | Opcode::Fpext
                | Opcode::Fptosi
                | Opcode::Sitofp
                | Opcode::Fptoui
                | Opcode::Uitofp
                | Opcode::Ptrtoint
                | Opcode::Inttoptr
                | Opcode::Bitcast
                | Opcode::AddrSpaceCast
        ) && let Some(rty) = instruction
            .results
            .first()
            .and_then(|r| self.func.dfg.value_type(*r))
        {
            write!(f, " to {}", fmt_llvm_type(&self.store.borrow(), rty))?;
        }

        // 指令尾 metadata 附加（`..., !dbg !N`）
        fmt_inst_metadata(f, instruction, self.module)?;

        Ok(())
    }
}

/// 指令尾 metadata 附加：`..., !dbg !N`（LLVM 标准逗号分隔；命名节点输出 `!name`）。
fn fmt_inst_metadata(
    f: &mut fmt::Formatter<'_>,
    instruction: &Instruction,
    module: Option<&Module>,
) -> fmt::Result {
    for am in &instruction.metadata {
        write!(f, ", !{} {}", am.kind.name(), fmt_meta_ref(module, am.node))?;
    }
    Ok(())
}

/// metadata 节点引用：命名节点输出 `!name`（name_of 反查），否则 `!N`。
fn fmt_meta_ref(module: Option<&Module>, id: crate::metadata::MetadataId) -> String {
    match module.and_then(|m| m.metadata_store.name_of(id)) {
        Some(name) => format!("!{name}"),
        None => format!("!{}", id.0),
    }
}

/// 字符串字面量转义（display 输出：`"` → `\"`、`\` → `\\`），保证 parse 可读回。
fn fmt_quoted(s: &str) -> String {
    // 与 lexer 的 StrLit 解码对称：`\n`/`\t`/`\r`/`\0` 必须转义，否则输出
    // 含裸控制字符的文本（自家 lexer 宽容能读回，但违反"严格 LLVM 文本"承诺）。
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out
}

/// 内存属性行尾：`, align N`（load/store；volatile 在指令名后输出）。
fn fmt_mem_attrs(f: &mut fmt::Formatter<'_>, instruction: &Instruction) -> fmt::Result {
    if let Some(Immediate::Uint(align)) = instruction
        .immediates
        .iter()
        // align 1 也输出（第二十九轮:原 >1 跳过致 `load ..., align 1`
        // roundtrip 后 immediates 不等——range.ll）
        .find(|im| matches!(im, Immediate::Uint(a) if *a > 0))
    {
        write!(f, ", align {align}")?;
    }
    Ok(())
}

/// 终结符的文本输出（LLVM 风格；块参数经 `label %t(i32 %v)` 扩展传递）。
struct TerminatorDisplay<'a> {
    func: &'a Function,
    store: &'a TypeContext,
    module: Option<&'a Module>,
    term: &'a Terminator,
    names: &'a NameResolver,
}

impl<'a> fmt::Display for TerminatorDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dfg = &self.func.dfg;
        match self.term {
            Terminator::Return { values, metadata } => {
                write!(f, "    ret ")?;
                match values.first() {
                    None => write!(f, "void")?,
                    Some(v) => match value_as_literal(self.func, self.store, self.module, *v) {
                        Some(lit) => write!(f, "{}", lit)?,
                        None => match dfg.value_type(*v) {
                            Some(ty) => write!(
                                f,
                                "{} %{}",
                                fmt_llvm_type(&self.store.borrow(), ty),
                                self.names.value(*v)
                            )?,
                            None => write!(f, "%{}", self.names.value(*v))?,
                        },
                    },
                }
                fmt_term_meta(f, metadata, self.module)?;
                Ok(())
            }
            Terminator::Jump {
                target,
                args: _,
                metadata,
            } => {
                write!(f, "    br label %{}", self.names.block(*target))?;
                fmt_term_meta(f, metadata, self.module)?;
                Ok(())
            }
            Terminator::Branch {
                cond,
                then_block,
                then_args: _,
                else_block,
                else_args: _,
                metadata,
            } => {
                write!(f, "  br ")?;
                // cond 可能是内联常量（iconst 指令行被跳过），需字面量内联；
                // 类型用真实类型而非硬编码 i1（builder 允许整数当条件）
                match value_as_literal(self.func, self.store, self.module, *cond) {
                    Some(lit) => write!(f, "{}", lit)?,
                    None => match dfg.value_type(*cond) {
                        Some(ty) => write!(
                            f,
                            "{} %{}",
                            fmt_llvm_type(&self.store.borrow(), ty),
                            self.names.value(*cond)
                        )?,
                        None => write!(f, "%{}", self.names.value(*cond))?,
                    },
                }
                write!(f, ", label %{}", self.names.block(*then_block))?;
                write!(f, ", label %{}", self.names.block(*else_block))?;
                fmt_term_meta(f, metadata, self.module)?;
                Ok(())
            }
            Terminator::Switch {
                discriminant,
                default_block,
                default_args: _,
                cases,
                metadata,
            } => {
                write!(f, "    switch ")?;
                match value_as_literal(self.func, self.store, self.module, *discriminant) {
                    Some(lit) => write!(f, "{}", lit)?,
                    None => match dfg.value_type(*discriminant) {
                        Some(ty) => write!(
                            f,
                            "{} %{}",
                            fmt_llvm_type(&self.store.borrow(), ty),
                            self.names.value(*discriminant)
                        )?,
                        None => write!(f, "%{}", self.names.value(*discriminant))?,
                    },
                }
                write!(f, ", label %{}", self.names.block(*default_block))?;
                write!(f, " [")?;
                // case 值类型跟随 discriminant（LLVM：`switch i64 %x, ... [ i64 1, ... ]`）
                let case_ty = dfg.value_type(*discriminant);
                let case_ty_str = case_ty.map(|t| fmt_llvm_type(&self.store.borrow(), t));
                for (i, (val, blk, _args)) in cases.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    match &case_ty_str {
                        Some(ty) => write!(f, "{ty} {}, label %{}", val, self.names.block(*blk))?,
                        None => write!(f, "i32 {}, label %{}", val, self.names.block(*blk))?,
                    }
                }
                write!(f, " ]")?;
                fmt_term_meta(f, metadata, self.module)?;
                Ok(())
            }
            Terminator::Unreachable => write!(f, "    unreachable"),
            Terminator::Invoke {
                callee,
                args,
                ret_ty,
                normal_block,
                normal_args,
                unwind_block,
                unwind_args: _,
                metadata,
            } => {
                // invoke <retty> @callee(args) to label %ok unwind label %pad；
                // `%r = invoke`：normal_args[0] 即返回值绑定名
                if let Some(&retv) = normal_args.first()
                    && *ret_ty != TypeId::VOID
                {
                    write!(f, "    %{} = invoke ", self.names.value(retv))?;
                } else {
                    write!(f, "    invoke ")?;
                }
                if *ret_ty != TypeId::VOID {
                    write!(f, "{} ", fmt_llvm_type(&self.store.borrow(), *ret_ty))?;
                } else {
                    write!(f, "void ")?;
                }
                match self.module {
                    Some(m) => {
                        let n = &m.get_function(*callee).name;
                        // Local callee 占位(名字以 % 开头——间接调用,第二十九轮)
                        if n.starts_with('%') {
                            write!(f, "{n}")?;
                        } else {
                            write!(f, "@{n}")?;
                        }
                    }
                    None => write!(f, "@f{}", callee.0)?,
                }
                write!(f, "(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    match value_as_literal(self.func, self.store, self.module, *arg) {
                        Some(lit) => write!(f, "{lit}")?,
                        None => match dfg.value_type(*arg) {
                            Some(ty) => write!(
                                f,
                                "{} %{}",
                                fmt_llvm_type(&self.store.borrow(), ty),
                                self.names.value(*arg)
                            )?,
                            None => write!(f, "%{}", self.names.value(*arg))?,
                        },
                    }
                }
                write!(f, ") to label %{}", self.names.block(*normal_block))?;
                write!(f, " unwind label %{}", self.names.block(*unwind_block))?;
                fmt_term_meta(f, metadata, self.module)?;
                Ok(())
            }
            Terminator::Resume { value, metadata } => {
                // resume <ty> %l
                write!(f, "    resume ")?;
                match value_as_literal(self.func, self.store, self.module, *value) {
                    Some(lit) => write!(f, "{}", lit)?,
                    None => match dfg.value_type(*value) {
                        Some(ty) => write!(
                            f,
                            "{} %{}",
                            fmt_llvm_type(&self.store.borrow(), ty),
                            self.names.value(*value)
                        )?,
                        None => write!(f, "%{}", self.names.value(*value))?,
                    },
                }
                fmt_term_meta(f, metadata, self.module)?;
                Ok(())
            }
        }
    }
}

impl<'a> TerminatorDisplay<'a> {}

/// 终结符尾 metadata 附加（LLVM：`ret i32 %x, !range !0` / `br ..., !prof !1`）。
fn fmt_term_meta(
    f: &mut fmt::Formatter<'_>,
    metadata: &smallvec::SmallVec<[crate::metadata::AttachedMetadata; 2]>,
    module: Option<&Module>,
) -> fmt::Result {
    for am in metadata {
        write!(f, ", !{} {}", am.kind.name(), fmt_meta_ref(module, am.node))?;
    }
    Ok(())
}

/// phi 入边值的裸格式（不带类型前缀，类型由 phi 头给出）：
/// `[ 0, %entry ]` / `[ %b, %loop ]` / `[ undef, %l ]`。
fn fmt_call_attrs(
    f: &mut fmt::Formatter<'_>,
    attrs: crate::function::FunctionAttributes,
) -> fmt::Result {
    for (flag, name) in [
        (crate::function::FunctionAttributes::NO_UNWIND, "nounwind"),
        (
            crate::function::FunctionAttributes::INLINE_NEVER,
            "noinline",
        ),
        (
            crate::function::FunctionAttributes::INLINE_ALWAYS,
            "alwaysinline",
        ),
        (crate::function::FunctionAttributes::NO_RECURSE, "norecurse"),
        (crate::function::FunctionAttributes::OPT_NONE, "optnone"),
    ] {
        if attrs.contains(flag) {
            write!(f, " {name}")?;
        }
    }
    Ok(())
}

fn fmt_param_attrs(pa: &crate::function::ParamAttributes) -> String {
    let mut out = String::new();
    for (on, name) in [
        (pa.signext, "signext"),
        (pa.zeroext, "zeroext"),
        (pa.noalias, "noalias"),
        (pa.noundef, "noundef"),
        (pa.readonly, "readonly"),
        (pa.writeonly, "writeonly"),
        (pa.nonnull, "nonnull"),
        (pa.nocapture, "nocapture"),
    ] {
        if on {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(name);
        }
    }
    for extra in &pa.extra {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(extra.as_str());
    }
    out
}

fn fmt_operand_llvm(d: &InstDisplay<'_>, v: Value) -> String {
    match value_as_literal(d.func, d.store, d.module, v) {
        Some(lit) => lit.to_string(),
        None => format!("%{}", d.names.value(v)),
    }
}

fn rmw_op_name(im: Option<&Immediate>) -> String {
    match im.and_then(|i| i.as_u64()) {
        Some(0) => "xchg".into(),
        Some(1) => "add".into(),
        Some(2) => "sub".into(),
        Some(3) => "and".into(),
        Some(4) => "nand".into(),
        Some(5) => "or".into(),
        Some(6) => "xor".into(),
        Some(7) => "max".into(),
        Some(8) => "min".into(),
        Some(9) => "umax".into(),
        Some(10) => "umin".into(),
        Some(11) => "fadd".into(),
        Some(12) => "fsub".into(),
        _ => "add".into(),
    }
}

fn ordering_name(im: Option<&Immediate>) -> String {
    match im.and_then(|i| i.as_u64()) {
        Some(1) => "unordered".into(),
        Some(2) => "monotonic".into(),
        Some(3) => "acquire".into(),
        Some(4) => "release".into(),
        Some(5) => "acq_rel".into(),
        Some(6) => "seq_cst".into(),
        _ => "seq_cst".into(),
    }
}

fn fmt_phi_value(
    f: &mut fmt::Formatter<'_>,
    func: &Function,
    names: &NameResolver,
    v: Option<Value>,
) -> fmt::Result {
    let Some(v) = v else {
        return write!(f, "undef");
    };
    let Some(def) = func.dfg.value_def(v) else {
        return write!(f, "%{}", names.value(v));
    };
    let crate::ValueDef::Inst(inst, _) = def else {
        return write!(f, "%{}", names.value(v));
    };
    let Some(inst_data) = func.dfg.insts.get(inst.0 as usize) else {
        return write!(f, "%{}", names.value(v));
    };
    match inst_data.opcode {
        crate::Opcode::Iconst => {
            if let Some(crate::Immediate::Const(cid)) = inst_data.immediates.first()
                && let Some((val, _bits)) = func.constants.get_int(*cid)
            {
                write!(f, "{val}")
            } else {
                write!(f, "undef")
            }
        }
        crate::Opcode::Fconst => {
            if let Some(crate::Immediate::Const(cid)) = inst_data.immediates.first()
                && let Some(bits) = func.constants.get_float(*cid)
            {
                write!(f, "{}", f64::from_bits(bits))
            } else {
                write!(f, "undef")
            }
        }
        crate::Opcode::Undef => write!(f, "undef"),
        crate::Opcode::Poison => write!(f, "poison"),
        _ => write!(f, "%{}", names.value(v)),
    }
}

// ============================================================
// Immediate Display
// ============================================================

// ============================================================
// Debug Display for DFG
// ============================================================

impl fmt::Display for DataFlowGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "DFG: {} values, {} insts, {} blocks",
            self.value_count(),
            self.inst_count(),
            self.block_count()
        )?;

        writeln!(f, "Values:")?;
        for (v, vd) in self.values() {
            let def_str = match vd.def {
                ValueDef::Inst(i, idx) => format!("inst {}.{}", i, idx),
                ValueDef::Param(b, idx) => format!("block {} param {}", b, idx),
                ValueDef::AggConst(id) => format!("agg const {id:?}"),
                ValueDef::UndefNamed(id) => format!("undef named {}", id.0),
            };
            writeln!(f, "  {} = {} : t{}", v, def_str, vd.ty.0)?;
        }
        Ok(())
    }
}

/// LLVM 风格的类型文本（display 专用）：匿名 struct 输出 `{i32, i64}`、
/// 地址空间 0 指针输出 `ptr`——与 grammar 的 ValueType 规则对称。
/// 命名 struct / 非零地址空间指针 / 函数类型无 LLVM 文本形式，回退
/// [`TypeStore::fmt_type`]（parser 读不回，属已知限制）。
/// global 初始值字节 → 文本（按类型：i32/i64/f32/f64；其余回退 hex）。
/// 聚合标量元素文本（`i32 1` / `double 2`；按元素类型格式化）。
fn fmt_agg_scalar(
    store: &TypeStore,
    pool: &crate::constant::ConstantPool,
    ety: TypeId,
    cid: crate::ConstId,
) -> String {
    if let Some((v, bits)) = pool.get_int(cid) {
        format!("i{bits} {v}")
    } else if let Some(bits) = pool.get_float128(cid) {
        match store.get(ety) {
            crate::types::TypeEntry::Float { bits: 32 } => {
                format!("float {}", f32::from_bits(bits as u32))
            }
            _ => format!("double {}", f64::from_bits(bits as u64)),
        }
    } else {
        format!("{} undef", fmt_llvm_type(store, ety))
    }
}

/// 聚合常量池还原为 LLVM 聚合字面量文本（3.1：`[i32 1, i32 2]` / `{ i32 1, i64 2 }` 递归）。
fn fmt_agg_const(
    store: &TypeStore,
    pool: &crate::constant::ConstantPool,
    id: crate::AggId,
) -> String {
    use crate::constant::AggChild;
    let agg = match pool.get_aggregate(id) {
        Some(a) => a,
        None => return "<agg?>".to_string(),
    };
    let inner: Vec<String> = agg
        .children
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let ety = store
                .aggregate_elem_type(agg.ty, i as u32)
                .unwrap_or(crate::TypeId::I32);
            match c {
                AggChild::Scalar(cid) => fmt_agg_scalar(store, pool, ety, *cid),
                // 嵌套聚合元素带类型前缀：`[2 x i32] [i32 1, i32 2]`
                AggChild::Agg(inner_id) => format!(
                    "{} {}",
                    fmt_llvm_type(store, ety),
                    fmt_agg_const(store, pool, *inner_id)
                ),
            }
        })
        .collect();
    match store.get(agg.ty) {
        crate::types::TypeEntry::Struct { is_packed, .. } if *is_packed => {
            format!("<{{ {} }}>", inner.join(", "))
        }
        crate::types::TypeEntry::Struct { .. } => format!("{{ {} }}", inner.join(", ")),
        _ => format!("[{}]", inner.join(", ")),
    }
}

fn fmt_global_init(store: &TypeStore, ty: TypeId, init: &[u8]) -> String {
    // 全零字节 → zeroinitializer（LLVM 标准；非标量聚合/数组/向量——
    // 第二十九轮:去掉原 `init.len() > 8` 限制,8 字节内聚合(如 [8 x i8])
    // 也被正确折叠;标量零走数字分支）
    if init.iter().all(|&b| b == 0)
        && !matches!(
            store.get(ty),
            TypeEntry::Int { .. } | TypeEntry::Float { .. }
        )
    {
        return "zeroinitializer".to_string();
    }
    match store.get(ty) {
        TypeEntry::Int { bits: 32 } if init.len() >= 4 => {
            format!(
                "{}",
                i32::from_le_bytes([init[0], init[1], init[2], init[3]])
            )
        }
        TypeEntry::Int { bits: 64 } if init.len() >= 8 => format!(
            "{}",
            i64::from_le_bytes(init[..8].try_into().unwrap_or([0; 8]))
        ),
        TypeEntry::Int { bits: 16 } if init.len() >= 2 => {
            format!("{}", i16::from_le_bytes([init[0], init[1]]))
        }
        TypeEntry::Int { bits: 8 } => format!("{}", init[0] as i8),
        TypeEntry::Float { bits: 32 } if init.len() >= 4 => {
            let bits = u32::from_le_bytes([init[0], init[1], init[2], init[3]]);
            let f = f32::from_bits(bits);
            // NaN/Inf 输出 hex 位模式（LLVM 关键字 qnan/snan/inf 无法还原
            // payload——第二十九轮:原 `format!("{f}")` 输出 "NaN"/"inf"
            // 无对应 token,reparse 拒绝）
            if f.is_nan() || f.is_infinite() {
                format!("0x{bits:08x}")
            } else if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{f:.1}")
            } else {
                format!("{f}")
            }
        }
        TypeEntry::Float { bits: 64 } if init.len() >= 8 => {
            let bits = u64::from_le_bytes(init[..8].try_into().unwrap_or([0; 8]));
            let f = f64::from_bits(bits);
            if f.is_nan() || f.is_infinite() {
                format!("0x{bits:016x}")
            } else if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{f:.1}")
            } else {
                format!("{f}")
            }
        }
        // f16/bfloat/其他位宽浮点：hex 位模式（第二十九轮:原落兜底
        // `0x{bytes}` 无类型标注,reparse 歧义）
        TypeEntry::Float { bits } | TypeEntry::BFloat { bits } if init.len() >= 2 => {
            format!(
                "0x{}",
                init[..init.len().min(8)]
                    .iter()
                    .rev()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            )
        }
        // [N x i8] → LLVM 字符串常量 c"..."（可打印字符 + 转义；其余 hex）
        TypeEntry::Array { elem, len }
            if matches!(store.get(*elem), TypeEntry::Int { bits: 8 })
                && init.len() as u64 >= *len =>
        {
            let bytes = &init[..*len as usize];
            let mut out = String::from("c\"");
            for &b in bytes {
                match b {
                    b'\\' => out.push_str("\\\\"),
                    b'"' => out.push_str("\\\""),
                    b'\n' => out.push_str("\\0A"),
                    b'\r' => out.push_str("\\0D"),
                    0x20..=0x7e => out.push(b as char),
                    _ => out.push_str(&format!("\\{b:02X}")),
                }
            }
            out.push('"');
            out
        }
        // 数组/结构体聚合：逐元素反解（struct 跳过 padding 字节）
        TypeEntry::Array { elem, len } if init.len() as u64 >= *len => {
            let elem_size = store.size_bytes(*elem) as usize;
            if elem_size == 0 {
                return format!(
                    "0x{}",
                    init.iter().map(|b| format!("{b:02x}")).collect::<String>()
                );
            }
            let mut out = String::from("[");
            for i in 0..*len as usize {
                if i > 0 {
                    out.push_str(", ");
                }
                let chunk = &init[i * elem_size..(i + 1) * elem_size];
                out.push_str(&format!(
                    "{} {}",
                    fmt_llvm_type(store, *elem),
                    fmt_global_init(store, *elem, chunk)
                ));
            }
            out.push(']');
            out
        }
        TypeEntry::Struct {
            fields, is_packed, ..
        } => {
            let mut out = String::from("{");
            let mut offset = 0usize;
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                if !*is_packed && i > 0 {
                    let align = store.alignment(f.ty) as usize;
                    offset = offset.div_ceil(align) * align;
                }
                let size = store.size_bytes(f.ty) as usize;
                let chunk = init
                    .get(offset..(offset + size).min(init.len()))
                    .unwrap_or(&[]);
                out.push_str(&format!(
                    "{} {}",
                    fmt_llvm_type(store, f.ty),
                    fmt_global_init(store, f.ty, chunk)
                ));
                offset += size;
            }
            out.push('}');
            out
        }
        _ => format!(
            "0x{}",
            init.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
    }
}

/// 按元素类型+端序把单个 lane 的字节格式化为文本（vconst lane 输出）。
fn fmt_vconst_lane(
    store: &TypeStore,
    elem: TypeId,
    chunk: &[u8],
    endian: crate::Endianness,
) -> String {
    let le: Vec<u8> = match endian {
        crate::Endianness::Little => chunk.to_vec(),
        crate::Endianness::Big => chunk.iter().rev().copied().collect(),
    };
    let i16v = |b: &[u8]| i16::from_le_bytes([b[0], b[1]]);
    let i32v = |b: &[u8]| i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let i64v = |b: &[u8]| i64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
    match store.get(elem) {
        TypeEntry::Int { bits: 8 } => format!("{}", i8::from_le_bytes([le[0]])),
        TypeEntry::Int { bits: 16 } if le.len() >= 2 => format!("{}", i16v(&le)),
        TypeEntry::Int { bits: 32 } if le.len() >= 4 => format!("{}", i32v(&le)),
        TypeEntry::Int { bits: 64 } if le.len() >= 8 => format!("{}", i64v(&le)),
        // 任意位宽整数 lane（i1/i7/i13...——mask 常量 `<4 x i1>` 等;
        // 第二十九轮:原兜底 "0" 丢失 true lane 位模式,roundtrip 不等）
        TypeEntry::Int { bits } => {
            let nbytes = (*bits as usize).div_ceil(8);
            let mut v: u128 = 0;
            for (k, &byte) in le.iter().enumerate().take(nbytes.min(le.len())) {
                v |= (byte as u128) << (k * 8);
            }
            format!("{v}")
        }
        TypeEntry::Float { bits: 32 } if le.len() >= 4 => {
            let f = f32::from_le_bytes([le[0], le[1], le[2], le[3]]);
            if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{f:.1}") // 整数值浮点带小数点（LLVM 浮点字面量风格）
            } else {
                format!("{f}")
            }
        }
        TypeEntry::Float { bits: 64 } if le.len() >= 8 => {
            let f = f64::from_le_bytes(le[..8].try_into().unwrap_or([0; 8]));
            if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{f:.1}")
            } else {
                format!("{f}")
            }
        }
        _ => "0".to_string(),
    }
}

/// metadata 节点序列化：`!{...}` / `!named(...)`。
fn fmt_metadata_node(
    store: &crate::metadata::MetadataStore,
    node: &crate::metadata::MetadataNode,
) -> String {
    use crate::metadata::MetadataNode;
    match node {
        MetadataNode::Tuple(vals) => format!("!{{{}}}", fmt_metadata_vals(store, vals)),
        MetadataNode::Named {
            name,
            ops,
            distinct,
        } => {
            format!(
                "{}{}{}({})",
                if *distinct { "distinct " } else { "" },
                "!",
                name,
                fmt_metadata_vals(store, ops)
            )
        }
        MetadataNode::Leaf(v) => fmt_metadata_val(store, v),
    }
}

fn fmt_metadata_vals(
    store: &crate::metadata::MetadataStore,
    vals: &smallvec::SmallVec<[crate::metadata::MetadataValue; 4]>,
) -> String {
    vals.iter()
        .map(|v| fmt_metadata_val(store, v))
        .collect::<Vec<_>>()
        .join(", ")
}

fn fmt_metadata_val(
    store: &crate::metadata::MetadataStore,
    v: &crate::metadata::MetadataValue,
) -> String {
    use crate::metadata::MetadataValue;
    match v {
        MetadataValue::String(s) => format!("\"{}\"", fmt_quoted(s)),
        MetadataValue::Uint(n) => n.to_string(),
        MetadataValue::Int(n) => n.to_string(),
        MetadataValue::IntBig(b) => b.to_string(),
        MetadataValue::Float(bits) => f64::from_bits(*bits).to_string(),
        MetadataValue::Null => "null".to_string(),
        // key:value 字段——还原 `key: value`（display roundtrip 与 DI 校验）
        MetadataValue::Field(k, inner) => {
            format!("{}: {}", k.as_str(), fmt_metadata_val(store, inner))
        }
        MetadataValue::Node(id) => {
            // 引用：命名节点输出 `!name`（name_of 反查），否则 `!N`；
            // node 是 Leaf 时内联（如 !{!DILocation(...)} 的引用保持 !N）
            match store.name_of(*id) {
                Some(name) => format!("!{name}"),
                None => format!("!{}", id.0),
            }
        }
    }
}

fn fmt_llvm_type(store: &TypeStore, ty: TypeId) -> String {
    match store.get(ty) {
        TypeEntry::Struct { name: Some(n), .. } => format!("%{}", store.lookup_str(*n)),
        TypeEntry::Struct {
            name: None,
            fields,
            is_packed,
            ..
        } => {
            let inner = fields
                .iter()
                .map(|f| fmt_llvm_type(store, f.ty))
                .collect::<Vec<_>>()
                .join(", ");
            if *is_packed {
                format!("<{{{}}}>", inner)
            } else {
                format!("{{{}}}", inner)
            }
        }
        TypeEntry::Pointer { addr_space: 0 } => "ptr".to_string(),
        TypeEntry::Pointer { addr_space } => format!("ptr addrspace({addr_space})"),
        TypeEntry::Int { bits: 0 } => "void".to_string(),
        TypeEntry::Int { bits: 1 } => "i1".to_string(),
        TypeEntry::Int { bits } => format!("i{bits}"),
        TypeEntry::Float { bits } => format!("f{bits}"),
        TypeEntry::BFloat { .. } => "bfloat".to_string(),
        TypeEntry::Vector { elem, len } => {
            format!("<{len} x {}>", fmt_llvm_type(store, *elem))
        }
        TypeEntry::ScalableVector { elem, min_len } => {
            format!("<vscale x {min_len} x {}>", fmt_llvm_type(store, *elem))
        }
        TypeEntry::Array { elem, len } => {
            format!("[{len} x {}]", fmt_llvm_type(store, *elem))
        }
        TypeEntry::Function {
            params,
            rets,
            is_vararg,
        } => {
            let p: Vec<String> = params.iter().map(|t| fmt_llvm_type(store, *t)).collect();
            let r: Vec<String> = rets.iter().map(|t| fmt_llvm_type(store, *t)).collect();
            let va = if *is_vararg { ", ..." } else { "" };
            format!("{}({}){}", r.join(", "), p.join(", "), va)
        }
        TypeEntry::Metadata => "metadata".to_string(),
        TypeEntry::Token => "token".to_string(),
        TypeEntry::Opaque => "opaque".to_string(),
    }
}

/// 若 Value 由常量/undef/poison 指令定义，返回 LLVM 字面量文本
/// （`i32 42`/`undef`/`poison`）；否则 None（正常值，用 %name）。
/// display 遍历指令时跳过这些"值定义"指令行，使用处内联。
fn value_as_literal(
    func: &Function,
    store: &TypeContext,
    module: Option<&Module>,
    v: Value,
) -> Option<ImmStr> {
    let def = func.dfg.values.get(v.0 as usize)?.def;
    // 聚合常量值：字面量文本带类型前缀（`{i32, i32} {i32 7, i32 9}`——
    // store/ret/call 实参均要求 `<ty> <val>`）
    if let ValueDef::AggConst(agg_id) = def {
        let ty = func.dfg.values.get(v.0 as usize)?.ty;
        return Some(crate::ImmStr::from(format!(
            "{} {}",
            fmt_llvm_type(&store.borrow(), ty),
            fmt_agg_const(&store.borrow(), &func.constants, agg_id)
        )));
    }
    let ValueDef::Inst(inst, _) = def else {
        return None;
    };
    let inst_data = func.dfg.insts.get(inst.0 as usize)?;
    match inst_data.opcode {
        Opcode::Iconst => {
            let Immediate::Const(cid) = inst_data.immediates.first().copied()? else {
                return None;
            };
            let (val, bits) = func.constants.get_int(cid)?;
            let vty = func.dfg.values.get(v.0 as usize)?.ty;
            // 向量类型的常量（向量 GEP 求值宽松 0——第二十九轮:原输出
            // `i{bits}` 类型漂移为标量,reparse 后类型不等）
            if matches!(
                store.borrow().get(vty),
                crate::types::TypeEntry::Vector { .. }
                    | crate::types::TypeEntry::ScalableVector { .. }
            ) {
                return Some(ImmStr::from(format!(
                    "{} zeroinitializer",
                    fmt_llvm_type(&store.borrow(), vty)
                )));
            }
            // ptr 类型的常量：零 → `ptr null`；非零 → `ptr {val}`（LLVM 15+
            // opaque ptr 常量语法——第二十九轮:原输出 `i64 {val}` 丢 PTR 类型,
            // GEP 表达式求值结果 roundtrip 后变 I64）；addrspace(N) 保留
            if let crate::types::TypeEntry::Pointer { addr_space } = store.borrow().get(vty) {
                let pfx = if *addr_space == 0 {
                    "ptr".to_string()
                } else {
                    format!("ptr addrspace({addr_space})")
                };
                if val == 0 {
                    return Some(ImmStr::from(format!("{pfx} null")));
                }
                return Some(ImmStr::from(format!("{pfx} {val}")));
            }
            Some(ImmStr::from(format!("i{} {}", bits, val)))
        }
        Opcode::Fconst => {
            let Immediate::Const(cid) = inst_data.immediates.first().copied()? else {
                return None;
            };
            let bits = func.constants.get_float(cid)?;
            // LLVM 标准浮点 hex 形式（位精确，round-trip 无损）：
            // f32 → `0x3FC00000`（8 位 hex）、f64 → `0x3FF8000000000000`（16 位 hex）、
            // half → `0xH...`、bfloat → `0xR...`（第二十九轮）
            let ty = func.dfg.value_type(v)?;
            let hex = match store.borrow().get(ty) {
                crate::types::TypeEntry::Float { bits: 16 } => {
                    format!("0xH{:04x}", bits as u16)
                }
                crate::types::TypeEntry::BFloat { .. } => {
                    format!("0xR{:04x}", bits as u16)
                }
                crate::types::TypeEntry::Float { bits: 32 } => format!("0x{:08x}", bits as u32),
                _ => format!("0x{bits:016x}"),
            };
            Some(ImmStr::from(format!(
                "{} {}",
                fmt_llvm_type(&store.borrow(), ty),
                hex
            )))
        }
        // undef/poison 作为带类型操作数（LLVM：`add i32 undef, i32 %a`）
        Opcode::Undef | Opcode::Poison => {
            let word = if inst_data.opcode == Opcode::Undef {
                "undef"
            } else {
                "poison"
            };
            match func.dfg.value_type(v) {
                Some(ty) => Some(ImmStr::from(format!(
                    "{} {word}",
                    fmt_llvm_type(&store.borrow(), ty)
                ))),
                None => Some(ImmStr::from_static(word)),
            }
        }
        // 全局变量直接引用：`ptr @g`（LLVM 标准；模块内查全局名）
        Opcode::GlobalAddr => {
            // 函数地址引用（String 编码——第三十一轮）
            if let Some(Immediate::String(sid)) = inst_data.immediates.first() {
                return Some(ImmStr::from(format!(
                    "ptr @{}",
                    store.borrow().lookup_str(*sid)
                )));
            }
            let Immediate::Global(gid) = inst_data.immediates.first().copied()? else {
                return None;
            };
            // 全局名优先;函数引用(GlobalAddr 编码 FuncRef——间接调用场景)
            // 回退查函数(第二十九轮);未注册(GlobalId 0 占位)输出 @undef
            let name = module
                .and_then(|m| m.get_global(gid))
                .map(|g| g.name.to_string())
                .or_else(|| {
                    // u32::MAX 是未注册哨兵,跳过函数回退(第三十一轮)
                    if gid.0 == u32::MAX {
                        None
                    } else {
                        module.map(|m| m.get_function(crate::FuncRef(gid.0)).name.to_string())
                    }
                })
                .unwrap_or_else(|| "undef".to_string());
            Some(ImmStr::from(format!("ptr @{name}")))
        }
        // 小端向量常量：`<4 x float> <1.5, 2.5, -3.5, 4.25>`（LLVM 标准字面量）
        Opcode::Vconst => {
            let Immediate::Const(cid) = inst_data.immediates.first().copied()? else {
                return None;
            };
            if func.constants.get_vector_endian(cid) != Some(crate::Endianness::Little) {
                return None; // 大端走 vconst 扩展指令行
            }
            let data = func.constants.get_vector(cid)?;
            let ty = func.dfg.value_type(v)?;
            let store_ref = store.borrow();
            let &crate::types::TypeEntry::Vector { elem, len } = store_ref.get(ty) else {
                return None;
            };
            // vector of ptr:数字 lane 无法表达——全零内联为 zeroinitializer,
            // 非全零不内联（走指令行 vconst 扩展;第二十九轮）
            if matches!(store_ref.get(elem), crate::types::TypeEntry::Pointer { .. }) {
                if data.iter().all(|&b| b == 0) {
                    return Some(ImmStr::from(format!(
                        "{} zeroinitializer",
                        store_ref.fmt_type(ty)
                    )));
                }
                return None;
            }
            let lane_size = store_ref.size_bytes(elem) as usize;
            if lane_size == 0 || data.len() < len as usize * lane_size {
                return None;
            }
            let mut text = String::from(" ");
            text.push_str(&fmt_llvm_type(&store_ref, ty));
            text.push_str(" <");
            for i in 0..len as usize {
                if i > 0 {
                    text.push_str(", ");
                }
                let chunk = &data[i * lane_size..(i + 1) * lane_size];
                text.push_str(&fmt_vconst_lane(
                    &store_ref,
                    elem,
                    chunk,
                    crate::Endianness::Little,
                ));
            }
            text.push('>');
            Some(ImmStr::from(text))
        }
        _ => None,
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::FunctionBuilder;
    use crate::types::{FunctionSignature, TypeContext};

    /// Helper: build a simple function returning i32 and return its text.
    fn display_simple_func(name: &str) -> String {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "x")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new(name, ctx.clone(), sig);
        let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "x")]);
        fb.switch_to_block(entry);
        let r = fb.iadd(params[0], params[0]);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        )
    }

    #[test]
    fn display_simple_arithmetic_function() {
        let text = display_simple_func("double");
        // Should contain fn name, param, opcode, ret
        assert!(
            text.contains("define i32 @double(i32 %x)"),
            "expected define header, got: {}",
            text
        );
        assert!(
            text.contains("i32 %x"),
            "expected typed param, got: {}",
            text
        );
        assert!(
            text.contains("add i32 %x, i32 %x"),
            "expected add opcode, got: {}",
            text
        );
        assert!(
            text.contains("ret"),
            "expected ret terminator, got: {}",
            text
        );
    }

    #[test]
    fn display_value_type_annotation() {
        let text = display_simple_func("typed");
        // Values should have type annotations: "v2: i32 = iadd"
        assert!(
            text.contains("= add i32"),
            "expected LLVM instruction, got: {}",
            text
        );
    }

    #[test]
    fn display_block_has_label() {
        let text = display_simple_func("blocks");
        // Block should have a label and colon
        assert!(text.contains(":"), "expected block colon, got: {}", text);
        // Should have indentation for instructions
        assert!(text.contains("    "), "expected indentation, got: {}", text);
    }

    #[test]
    fn display_function_with_branch() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "c")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "c")]);
        let then_blk = fb.create_block();
        let else_blk = fb.create_block();
        fb.switch_to_block(entry);
        fb.branch(params[0], then_blk, &[], else_blk, &[]);
        fb.switch_to_block(then_blk);
        let v1 = fb.iconst_i32(1);
        fb.ret(&[v1]);
        fb.switch_to_block(else_blk);
        let v2 = fb.iconst_i32(0);
        fb.ret(&[v2]);
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(text.contains("br "), "expected br, got: {}", text);
        assert!(
            text.contains("br i32 %c, label %b1, label %b2"),
            "expected branch, got: {}",
            text
        );
    }

    #[test]
    fn display_dfg_debug() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let func = fb.finish().expect("build");
        let text = format!("{}", func.dfg);
        assert!(text.contains("DFG:"), "expected DFG header, got: {}", text);
        assert!(text.contains("values"), "expected values, got: {}", text);
        assert!(text.contains("insts"), "expected insts, got: {}", text);
        assert!(text.contains("blocks"), "expected blocks, got: {}", text);
    }

    #[test]
    fn display_terminator_switch() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "c")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "c")]);
        let default_blk = fb.create_block();
        let case1_blk = fb.create_block();
        fb.switch_to_block(entry);
        fb.switch(params[0], default_blk, &[(1, case1_blk, &[])]);
        fb.switch_to_block(default_blk);
        let v = fb.iconst_i32(0);
        fb.ret(&[v]);
        fb.switch_to_block(case1_blk);
        let v1 = fb.iconst_i32(1);
        fb.ret(&[v1]);
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(text.contains("switch"), "expected switch, got: {}", text);
        assert!(
            text.contains("switch i32 %c, label %b1"),
            "expected switch header, got: {}",
            text
        );
    }

    #[test]
    fn display_terminator_return_void() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        fb.ret(&[]);
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(text.contains("ret"), "expected ret, got: {}", text);
    }

    #[test]
    fn display_terminator_jump() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        let target = fb.create_block();
        fb.switch_to_block(entry);
        fb.jump(target, &[]);
        fb.switch_to_block(target);
        let v = fb.iconst_i32(42);
        fb.ret(&[v]);
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(
            text.contains("br label"),
            "expected br label, got: {}",
            text
        );
    }

    #[test]
    fn display_terminator_unreachable() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[]);
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        fb.unreachable();
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(
            text.contains("unreachable"),
            "expected unreachable, got: {}",
            text
        );
    }

    #[test]
    fn display_function_signature_with_return_types() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(
            &[(ctx.i32_ty(), "a"), (ctx.f64_ty(), "b")],
            &[ctx.i32_ty(), ctx.f64_ty()],
        );
        let mut fb = FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _params) =
            fb.create_block_with_params(&[(TypeId::I32, "a"), (TypeId::F64, "b")]);
        fb.switch_to_block(entry);
        let v = fb.iconst_i32(2);
        let fv = fb.fconst_f64(1.0);
        fb.ret(&[v, fv]);
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(
            text.contains("define i32 @test(i32 %a, f64 %b)"),
            "expected define header with params, got: {}",
            text
        );
        assert!(
            text.contains("i32 %a, f64 %b"),
            "expected typed params, got: {}",
            text
        );
        assert!(text.contains("ret"), "expected ret, got: {}", text);
    }

    #[test]
    fn display_param_names_bound() {
        // create_block_with_params 的参数名现在绑定到参数 value → 块参数显示 %x
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "x")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
        let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "x")]);
        fb.switch_to_block(entry);
        let r = fb.iadd(params[0], params[0]);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(
            text.contains("define i32 @f(i32 %x)"),
            "param value should bind to its name, got: {}",
            text
        );
        assert!(
            text.contains("add i32 %x, i32 %x"),
            "param name used in operands, got: {}",
            text
        );
        assert!(
            text.contains("ret i32 %"),
            "ret should use % name, got: {}",
            text
        );
    }

    #[test]
    fn display_unnamed_values_get_indexed_names() {
        // 不绑定的值自动编号（%v{index}），唯一且带 % 前缀
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        let a = fb.iconst_i32(1);
        let b = fb.iconst_i32(2);
        let r = fb.iadd(a, b);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(
            text.contains("%v"),
            "unnamed values get %v names, got: {}",
            text
        );
        // %v 前缀（LLVM 合法），不是裸 v
        assert!(!text.contains("iadd v"), "no bare v names, got: {}", text);
    }

    #[test]
    fn display_disambiguates_duplicate_names() {
        // 两个值绑定同名 → display 消歧为 %x / %x_1（SSA 唯一）
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "x")], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
        let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "x")]);
        fb.switch_to_block(entry);
        let a = fb.iadd(params[0], params[0]);
        fb.bind_name(a, "x"); // 与参数同名 → 消歧 %x / %x_1
        let r = fb.iadd(a, a);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(
            text.contains("%x_1 = add i32 %x, i32 %x"),
            "duplicate names disambiguated, got: {}",
            text
        );
        assert!(
            text.contains("ret i32 %"),
            "result uses name, got: {}",
            text
        );
    }

    #[test]
    fn bind_name_overwrites() {
        // bind_name 可覆盖更新（同名重绑）
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.switch_to_block(entry);
        let x = fb.iconst_i32(1);
        let a = fb.iadd(x, x);
        fb.bind_name(a, "first");
        fb.bind_name(a, "second");
        let r = fb.iadd(a, a);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(
            text.contains("%second"),
            "bind_name overwrites, got: {}",
            text
        );
        assert!(!text.contains("%first"), "old name gone, got: {}", text);
    }

    #[test]
    fn bind_block_name_shows_label() {
        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = FunctionBuilder::new("f", ctx.clone(), sig);
        let entry = fb.create_block();
        fb.bind_block_name(entry, "entry");
        fb.switch_to_block(entry);
        let r = fb.iconst_i32(1);
        fb.ret(&[r]);
        let store = fb.type_ctx().clone();
        let func = fb.finish().expect("build");
        let text = format!(
            "{}",
            FunctionDisplay {
                func: &func,
                store: &store,
                module: None,
            }
        );
        assert!(text.contains("%entry:"), "block name shown, got: {}", text);
    }
}

// ============================================================
// LLVM 字面量内联（round-trip：iconst/undef/poison 作操作数输出）
// ============================================================
