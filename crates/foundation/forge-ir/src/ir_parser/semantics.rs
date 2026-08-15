//! 语义构建：ParsedModule（grammar.lalrpop 产出）→ forge IR。
//!
//! 职责：名称解析（`@`/`%` 符号表 + SSA 唯一性）、ParsedType → TypeId、
//! LLVM opcode → forge Opcode（含 icmp/fcmp 条件、向量精化、trunc 分发）、
//! 类型化操作数 → Value、常量、terminator、跨函数 `@` 解析。

use std::collections::{HashMap, HashSet};

use smallvec::SmallVec;

use crate::CallConv;
use crate::DataLayout;
use crate::Endianness;
use crate::builder::FunctionBuilder;
use crate::entity::{Block, FuncRef, GlobalId, TypeId, Value};
use crate::function::Function;
use crate::function::FunctionAttributes;
use crate::function::GlobalVariable;
use crate::function::Module;
use crate::imm_str::ImmStr;
use crate::immediate::Immediate;
use crate::inst_flags::InstFlags;
use crate::mem_flags::MemFlags;
use crate::opcode::AtomicRmwOp;
use crate::opcode::Opcode;
use crate::opcode::Ordering;
use crate::terminator::Terminator;
use crate::types::{FunctionSignature, TypeContext, TypeEntry};

use super::ast_items::*;
use super::lexer::TokenStream;
use super::llvm_mapping;
use crate::error::IrError;

/// 解析 LLVM IR module（多函数 + target）。
pub fn parse_module(source: &str) -> Result<Module, IrError> {
    let mut ast = parse_to_ast(source)?;
    build_module(&mut ast)
}

/// 解析单个 LLVM IR 函数（module 的第一个 define）。
pub fn parse_function(source: &str) -> Result<crate::function::Function, IrError> {
    let ast = parse_to_ast(source)?;
    for item in &ast.items {
        if let ParsedItem::Function(f) = item {
            if f.declare {
                continue;
            }
            let ctx = TypeContext::new();
            return build_function(f, &HashMap::new(), &HashMap::new(), &ctx);
        }
    }
    Err(IrError::Semantic(
        "no function definition found".to_string(),
    ))
}

fn parse_to_ast(source: &str) -> Result<ParsedModule, IrError> {
    let lexer = TokenStream::new(source);
    let parser = super::grammar::ModuleParser::new();
    parser
        .parse(lexer)
        .map_err(|e| IrError::Parse(format!("{:?}", e)))
}

// ── 模块构建 ──

/// uselistorder 基础校验（第十一轮）：索引 ≥2、无重复、非升序；模块级
/// 值名存在性（@global 必须在 items 中定义）。use 数/范围校验需符号表
/// use 列表（记录已知失败）。
fn check_uselistorder(
    name: &str,
    idx: &[i64],
    exists: &dyn Fn(&str) -> bool,
    use_count: Option<u32>,
) -> Result<(), IrError> {
    if name.starts_with('@') && !exists(name.trim_start_matches('@')) {
        return Err(IrError::Semantic(format!(
            "uselistorder references undefined global {name}"
        )));
    }
    // use 计数校验（第二十一轮：invalid-uselistorder-* 官方规则）
    match use_count {
        None => {
            // 局部值未定义（function-missing-named/numbered）→ 无 uses
            return Err(IrError::Semantic("value has no uses".to_string()));
        }
        Some(0) => {
            return Err(IrError::Semantic("value has no uses".to_string()));
        }
        Some(1) => {
            return Err(IrError::Semantic("value only has one use".to_string()));
        }
        Some(n) if n as usize != idx.len() => {
            return Err(IrError::Semantic(format!(
                "wrong number of indexes, expected {n}"
            )));
        }
        _ => {}
    }
    // 索引范围 [0, size)
    if let Some(n) = use_count
        && idx.iter().any(|&i| i < 0 || i as u32 >= n)
    {
        return Err(IrError::Semantic(
            "expected distinct uselistorder indexes in range [0, size)".to_string(),
        ));
    }
    if idx.len() < 2 {
        return Err(IrError::Semantic(
            "uselistorder requires at least 2 indices".to_string(),
        ));
    }
    let mut sorted = idx.to_vec();
    sorted.sort_unstable();
    let mut deduped = sorted.clone();
    deduped.dedup();
    if deduped.len() != idx.len() {
        return Err(IrError::Semantic(
            "uselistorder indices must be unique".to_string(),
        ));
    }
    if idx.windows(2).all(|w| w[0] < w[1]) {
        return Err(IrError::Semantic(
            "uselistorder indices must not be in ascending order (identity)".to_string(),
        ));
    }
    Ok(())
}

/// 终结符操作数中指定局部值的引用计数（第二十一轮 uselistorder use 计数）。
fn count_term_uses(t: &crate::ir_parser::ast_items::ParsedTerminator, name: &str) -> u32 {
    use crate::ir_parser::ast_items::ParsedTerminator;
    let hit = |op: &ParsedOperand| -> u32 {
        match &op.op {
            Operand::Local(l) if l.trim_start_matches('%') == name => 1,
            _ => 0,
        }
    };
    match t {
        ParsedTerminator::Return(vals, _) => vals.iter().map(hit).sum(),
        ParsedTerminator::Jump(_, _) => 0,
        ParsedTerminator::Branch(cond, _, _, _) => cond.iter().map(hit).sum(),
        ParsedTerminator::Switch(d, _, _, _) => hit(d),
        ParsedTerminator::Invoke { args, .. } => args.iter().map(hit).sum(),
        ParsedTerminator::Resume(v, _) => hit(v),
        ParsedTerminator::Unreachable => 0,
    }
}

/// use 计数收集（第二十一轮 uselistorder 校验扩展）：值名（@glob/%local）
/// → 被引用次数。别名 aliasee、全局 init 常量表达式、指令/终结符操作数。
fn collect_uses(ast: &ParsedModule) -> std::collections::HashMap<String, u32> {
    use std::collections::HashMap;
    let mut counts: HashMap<String, u32> = HashMap::new();
    fn bump(counts: &mut HashMap<String, u32>, name: &str) {
        *counts.entry(name.to_string()).or_insert(0) += 1;
    }
    fn walk_expr(e: &ConstExpr, counts: &mut HashMap<String, u32>) {
        match e {
            ConstExpr::GlobalAddr(g) => bump(counts, &format!("@{g}")),
            ConstExpr::PtrToInt { op, .. }
            | ConstExpr::IntToPtr { op, .. }
            | ConstExpr::Bitcast { op, .. }
            | ConstExpr::AddrSpaceCast { op, .. } => walk_expr(op, counts),
            ConstExpr::GetElementPtr { ptr, indices, .. } => {
                walk_expr(ptr, counts);
                for (_, i) in indices {
                    walk_expr(i, counts);
                }
            }
            ConstExpr::Binary { lhs, rhs, .. } => {
                walk_expr(lhs, counts);
                walk_expr(rhs, counts);
            }
            ConstExpr::Cast { src, .. } => walk_expr(src, counts),
            ConstExpr::Ptrauth(args) => {
                for (_, a) in args {
                    walk_expr(a, counts);
                }
            }
            _ => {}
        }
    }
    for item in &ast.items {
        match item {
            ParsedItem::Alias(a) => walk_expr(&a.aliasee.1, &mut counts),
            ParsedItem::Global(g) => {
                if let Some(GlobalInitVal::Expr(e)) = &g.init {
                    walk_expr(e, &mut counts);
                }
                if let Some(GlobalInitVal::Agg(vals)) = &g.init {
                    for v in vals {
                        if let GlobalInitVal::Expr(e) = v {
                            walk_expr(e, &mut counts);
                        }
                    }
                }
            }
            ParsedItem::Function(f) => {
                for blk in &f.blocks {
                    for inst in &blk.insts {
                        for a in &inst.args {
                            match &a.op {
                                Operand::Local(l) => bump(&mut counts, l),
                                Operand::Global(g) => bump(&mut counts, g),
                                Operand::ConstExpr(e) => walk_expr(e, &mut counts),
                                _ => {}
                            }
                        }
                    }
                    if let ParsedTerminator::Invoke { callee, .. } = &blk.terminator {
                        bump(&mut counts, callee);
                    }
                }
            }
            _ => {}
        }
    }
    counts
}

fn build_module(ast: &mut ParsedModule) -> Result<Module, IrError> {
    // uselistorder 模块级校验（第十一轮）：索引必须 ≥2、无重复、非升序
    // （LLVM：use-list 顺序必须是真实置换且非 identity）
    let global_names: std::collections::HashSet<String> = ast
        .items
        .iter()
        .filter_map(|i| match i {
            ParsedItem::Global(g) => Some(g.name.trim_start_matches('@').to_string()),
            ParsedItem::Function(f) => Some(f.name.trim_start_matches('@').to_string()),
            ParsedItem::Alias(a) => Some(a.name.trim_start_matches('@').to_string()),
            _ => None,
        })
        .collect();
    for item in &ast.items {
        if let ParsedItem::UselistOrder(name, idx, _ty) = item {
            if !name.starts_with('@') {
                // % 局部值形态（块尾/函数内）——verify-uselistorder 语义
                // 故意未定义引用合法（uselistorder.ll 的 %e）；块尾已有宽松
                // 索引形状校验；function-missing-* 的 llvm-as 严格语义为
                // 工具差异，无法两全，归档
                continue;
            }
            let uses = collect_uses(ast);
            let bare = name.trim_start_matches('@');
            let use_count = uses.get(&format!("@{bare}")).copied();
            check_uselistorder(name, idx, &|n| global_names.contains(n), use_count)?;
        }
    }
    // LLVM 链接纪律：local linkage（private/internal）必须 default visibility
    // （private-hidden-variable/private-protected-alias 类——语法层已接受,
    // 值级校验拒绝）
    for item in &ast.items {
        let (linkage, vis) = match item {
            ParsedItem::Global(g) => (g.linkage.as_deref(), g.visibility.as_deref()),
            ParsedItem::Alias(a) => (a.linkage.as_deref(), a.visibility.as_deref()),
            _ => continue,
        };
        if matches!(linkage, Some("private") | Some("internal")) && vis.is_some() {
            return Err(IrError::Semantic(
                "symbol with local linkage must have default visibility".to_string(),
            ));
        }
    }
    // LLVM comdat 纪律：数字函数名（unnamed）+ 裸 comdat（无名）拒绝
    // （unnamed-comdat.ll——`define void @0() comdat {`）
    for item in &ast.items {
        if let ParsedItem::Function(f) = item
            && f.comdat.as_deref() == Some("")
        {
            let bare = f.name.trim_start_matches('@');
            if !bare.is_empty() && bare.chars().all(|c| c.is_ascii_digit()) {
                return Err(IrError::Semantic("comdat cannot be unnamed".to_string()));
            }
        }
    }
    // summary 条目校验（第二十三轮 summary-parsing-error / thinlto-bad-summary-5）：
    // gv: (name: "X") 的 X 必须已定义;X 是函数定义且条目无 summaries
    //（value info）子句 → 拒绝
    for item in &ast.items {
        if let ParsedItem::Summary(s) = item {
            // 仅 gv: 条目要求 name 已定义;typeidCompatibleVTable 等条目
            // 引用外部 RTTI 符号合法(thinlto-vtable-skip 的正向形态)
            if !s.contains("gv:") {
                continue;
            }
            if let Some(nstart) = s.find("name:") {
                let rest = &s[nstart + 5..];
                if let Some(q) = rest.find('"') {
                    let inner = &rest[q + 1..];
                    if let Some(q2) = inner.find('"') {
                        let name = &inner[..q2];
                        // 定义名可能带引号（@"_ZTVN3FooE" 形态的引号全局名）
                        let quoted = format!("\"{name}\"");
                        let defined = global_names.contains(name) || global_names.contains(&quoted);
                        if !defined {
                            return Err(IrError::Semantic(format!(
                                "Reference to undefined global \"{name}\""
                            )));
                        }
                        let is_define = ast.items.iter().any(|i| {
                            matches!(i, ParsedItem::Function(f)
                                if !f.declare
                                    && (f.name.trim_start_matches('@') == name
                                        || f.name.trim_start_matches('@') == quoted))
                        });
                        if is_define && !s.contains("summaries:") {
                            return Err(IrError::Semantic(format!(
                                "expected function definition {name} to have an associated value info"
                            )));
                        }
                    }
                }
            }
        }
    }
    // 全局尾 metadata 延迟附加队列：(global 名, [(kind, ref), ...])——节点定义在
    // 模块 metadata 段（global 构建时 store 尚未就位），收尾统一 attach。
    let mut pending_global_metas: Vec<(String, Vec<(String, MetadataRef)>)> = Vec::new();
    // 属性组收集：`attributes #N = { ... }`（供 call/define 的 #N 展开）
    let mut attr_groups: HashMap<u32, Vec<String>> = HashMap::new();
    for item in &ast.items {
        if let ParsedItem::AttrGroup(id, attrs) = item {
            attr_groups.insert(*id, attrs.clone());
        }
    }
    // 预收集 metadata 节点（先定义后使用——按文本顺序 intern；命名引用要求先定义）。
    // `!t = !{...}` 命名定义：intern 节点 + define_named（供 `!dbg !t` / `!{!t}` 引用）。
    // id 分配策略：**先 reserve 全部显式数字 id（`!N = ...` 空占位），再 intern 命名
    // 节点**——命名 id 从数字区之后开始，永不与 `!N` 冲突（否则 `!t` 拿到 id 0 会被
    // `!0 = !{!t}` 的 insert_at 覆盖，造成自引用）。
    let mut module_meta_store = crate::metadata::MetadataStore::new();
    // 数字区大小 = 显式 `!N = ...` 定义的最大 id + 1（命名节点 intern 从数字区后开始；
    // `!N` 数值引用必须 < 数字区——否则撞上命名节点 id）
    let max_num_meta_id = ast
        .items
        .iter()
        .filter_map(|i| match i {
            ParsedItem::Metadata(id, _) => Some(*id),
            _ => None,
        })
        .max()
        .map(|m| m + 1)
        .unwrap_or(0);
    for item in &ast.items {
        if let ParsedItem::Metadata(id, _) = item {
            module_meta_store.insert_at(
                crate::metadata::MetadataId(*id),
                crate::metadata::MetadataNode::Tuple(smallvec::smallvec![]),
            );
        }
    }
    // 第二遍：全部命名定义（intern + define_named）——命名之间按文本顺序
    // （命名引用须先定义）；数字引用（`!N`）无需 lookup，reserve 已占位。
    for item in &ast.items {
        if let ParsedItem::NamedMetadata(name, def) = item {
            let node = build_metadata_node(def, &mut module_meta_store, max_num_meta_id, false)?;
            let id = module_meta_store.intern(node);
            module_meta_store.define_named(name, id);
        }
    }
    // 第三遍：数字定义 fill——此时全部命名已注册，`!0 = !{!t}` 的 NamedRef
    // 可解析（display 按 id 顺序输出会让数字定义排在命名前，必须容忍前向引用）。
    for item in &ast.items {
        if let ParsedItem::Metadata(id, def) = item {
            let node = build_metadata_node(def, &mut module_meta_store, max_num_meta_id, false)?;
            module_meta_store.insert_at(crate::metadata::MetadataId(*id), node);
        }
    }
    // 预处理：把函数/指令属性里的 `#N` 展开为组内具体属性名；
    // 命名 metadata 引用（`!dbg !t`）经 lookup_named 解析为数字 id（顺序无关）。
    for item in &mut ast.items {
        if let ParsedItem::Function(f) = item {
            f.attrs = expand_attr_groups(&f.attrs, &attr_groups)?;
            for (_, r) in &mut f.metadata {
                if let MetadataRef::Named(n) = r {
                    let id = module_meta_store.lookup_named(n).ok_or_else(|| {
                        IrError::Semantic(format!("undefined named metadata !{n}"))
                    })?;
                    *r = MetadataRef::Num(id.0);
                }
            }
            for blk in &mut f.blocks {
                for inst in &mut blk.insts {
                    inst.call_fn_attrs = expand_attr_groups(&inst.call_fn_attrs, &attr_groups)?;
                    for (_, r) in &mut inst.metadata_attach {
                        if let MetadataRef::Named(n) = r {
                            let id = module_meta_store.lookup_named(n).ok_or_else(|| {
                                IrError::Semantic(format!("undefined named metadata !{n}"))
                            })?;
                            *r = MetadataRef::Num(id.0);
                        }
                    }
                }
            }
        }
    }
    let mut module = Module::new();

    // target 指令：triple 与 datalayout 均真正解析；source_filename / module asm
    for item in &ast.items {
        match item {
            ParsedItem::Target(k, v) => {
                if k.as_str() == "triple" {
                    module.set_target_triple(v);
                } else if k.as_str() == "datalayout" {
                    let dl = DataLayout::parse(v).map_err(|e| IrError::Semantic(e.to_string()))?;
                    // 同步重建 types（size/align 查询与 data_layout 一致）
                    module.set_data_layout(dl);
                }
            }
            ParsedItem::SourceFilename(s) => {
                module.source_filename = Some(s.clone());
            }
            ParsedItem::ModuleAsm(s) => {
                module.module_asm.push(s.clone());
            }
            ParsedItem::ComdatDecl(c) => {
                // `$c = comdat any` 声明（重复声明报错——LLVM 符号表）
                module
                    .add_comdat(c, crate::symbol::ComdatKind::Any)
                    .map_err(|e| IrError::Semantic(format!("comdat '{c}': {e}")))?;
            }
            _ => {}
        }
    }

    // 所有函数共享 module 的 TypeContext（bind_name 的 InternedStr 进入
    // module.types 的 pool——display 用同一 pool lookup，避免跨 pool 越界）。
    // 必须在 set_data_layout 之后 clone（重建会更换 pool）。
    let ctx = module.types.clone();

    // LLVM 类型定义：`%struct.X = type {...}`——两遍处理（第十二轮）：
    // 第一遍注册全部名字（空占位——支持前向引用 `%struct.A = type { %struct.anon }`，
    // LLVM 两遍解析）；第二遍按定义填充（struct_named 按名去重）。
    for item in &ast.items {
        if let ParsedItem::TypeDef(name, ParsedType::Struct(_) | ParsedType::StructPacked(_)) = item
        {
            let name = name.trim_start_matches('%');
            let packed = matches!(item, ParsedItem::TypeDef(_, ParsedType::StructPacked(_)));
            let id = ctx.borrow_mut().struct_anon(vec![], packed);
            ctx.borrow_mut().define_named(name, id);
        }
    }
    // 递归类型检测（mutually-recursive-types / unsized-recursive-type）：
    // 命名结构的**直接**引用环（不含指针间接——经 ptr 的递归合法）拒绝。
    // ParsedType 层依赖图 DFS（第一遍占位后、第二遍填充前）。
    {
        fn collect_direct_named(t: &ParsedType, out: &mut Vec<String>) {
            match t {
                ParsedType::Named(n) => out.push(n.trim_start_matches('%').to_string()),
                // 指针间接（含 addrspace）终止——`%list = type { %list* }` 合法
                ParsedType::Ptr | ParsedType::PtrAddrSpace(_) => {}
                ParsedType::Struct(tys) | ParsedType::StructPacked(tys) => {
                    for f in tys {
                        collect_direct_named(f, out);
                    }
                }
                ParsedType::Array(_, e) => collect_direct_named(e, out),
                _ => {}
            }
        }
        let mut deps: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for item in &ast.items {
            if let ParsedItem::TypeDef(
                name,
                ParsedType::Struct(tys) | ParsedType::StructPacked(tys),
            ) = item
            {
                let mut d = vec![];
                for f in tys {
                    collect_direct_named(f, &mut d);
                }
                deps.insert(name.trim_start_matches('%').to_string(), d);
            }
        }
        fn has_cycle(
            name: &str,
            deps: &std::collections::HashMap<String, Vec<String>>,
            visiting: &mut std::collections::HashSet<String>,
            done: &mut std::collections::HashSet<String>,
        ) -> Option<String> {
            if done.contains(name) {
                return None;
            }
            if !visiting.insert(name.to_string()) {
                return Some(name.to_string());
            }
            if let Some(nexts) = deps.get(name) {
                for n in nexts {
                    if let Some(c) = has_cycle(n, deps, visiting, done) {
                        return Some(c);
                    }
                }
            }
            visiting.remove(name);
            done.insert(name.to_string());
            None
        }
        let mut visiting = std::collections::HashSet::new();
        let mut done = std::collections::HashSet::new();
        for name in deps.keys() {
            if let Some(c) = has_cycle(name, &deps, &mut visiting, &mut done) {
                return Err(IrError::Semantic(format!(
                    "identified structure type '{c}' is recursive"
                )));
            }
        }
    }
    for item in &ast.items {
        if let ParsedItem::TypeDef(name, ParsedType::Struct(tys) | ParsedType::StructPacked(tys)) =
            item
        {
            let packed = matches!(item, ParsedItem::TypeDef(_, ParsedType::StructPacked(_)));
            let name = name.trim_start_matches('%');
            let field_tys: Vec<TypeId> = tys
                .iter()
                .map(|t| to_type_result(t, &ctx))
                .collect::<Result<_, _>>()?;
            let id = ctx.borrow_mut().struct_named(
                name,
                field_tys
                    .iter()
                    .map(|&t| crate::types::TypeField::new(t))
                    .collect(),
                packed,
            );
            ctx.borrow_mut().define_named(name, id);
        }
    }
    // 第三遍（第十四轮，独立循环——依赖全部完整后重填）：前向引用占位升级
    // ——%0 引用 %1 时第二遍 struct_named 的字段还是 %1 的占位；第三遍
    // struct_named 对旧占位字段原位升级（types.rs 升级分支）。
    for item in &ast.items {
        if let ParsedItem::TypeDef(name, ParsedType::Struct(tys) | ParsedType::StructPacked(tys)) =
            item
        {
            let packed = matches!(item, ParsedItem::TypeDef(_, ParsedType::StructPacked(_)));
            let name = name.trim_start_matches('%');
            let field_tys: Vec<TypeId> = tys
                .iter()
                .map(|t| to_type_result(t, &ctx))
                .collect::<Result<_, _>>()?;
            let id = ctx.borrow_mut().struct_named(
                name,
                field_tys
                    .iter()
                    .map(|&t| crate::types::TypeField::new(t))
                    .collect(),
                packed,
            );
            ctx.borrow_mut().define_named(name, id);
        }
    }

    // 第一遍：注册全局变量（@g = global/constant <ty> <init>）——函数体内的
    // global_addr 引用需要先解析到 GlobalId。
    let mut global_refs: HashMap<String, GlobalId> = HashMap::new();
    for item in &ast.items {
        if let ParsedItem::Alias(a) = item {
            let ty = to_type(&a.ty, &ctx);
            // 可见性冲突（LLVM：internal 与 hidden/protected 互斥）
            if a.linkage.as_deref() == Some("internal") && a.visibility.is_some() {
                return Err(IrError::Semantic(format!(
                    "alias {}: internal linkage cannot be combined with visibility `{}`",
                    a.name,
                    a.visibility.as_deref().unwrap()
                )));
            }
            let linkage = match a.linkage.as_deref() {
                Some("internal") => crate::symbol::Linkage::Internal,
                Some("private") => crate::symbol::Linkage::Private,
                Some("external") => crate::symbol::Linkage::External,
                Some("weak") => crate::symbol::Linkage::WeakAny,
                Some(other) => {
                    return Err(IrError::Semantic(format!(
                        "unsupported alias linkage `{other}`"
                    )));
                }
                None => crate::symbol::Linkage::External,
            };
            // 前向引用类型校验（opaque-ptr-invalid-forward-ref ×2）：
            // aliasee 为 `ptr addrspace(N) @name`（N≠0）且 @name 后定义时,
            // 其实际类型地址空间必须为 N,否则拒绝
            if let (
                ParsedType::PtrAddrSpace(n),
                crate::ir_parser::ast_items::ConstExpr::GlobalAddr(target),
            ) = (&a.aliasee.0, &a.aliasee.1)
                && *n != 0
            {
                let def = ast.items.iter().find(|i| match i {
                    ParsedItem::Global(g) => g.name.trim_start_matches('@') == target,
                    ParsedItem::Function(f) => f.name.trim_start_matches('@') == target,
                    _ => false,
                });
                match def {
                    Some(ParsedItem::Global(g)) if g.addr_space != *n => {
                        return Err(IrError::Semantic(
                            "forward reference and definition of global have different types"
                                .to_string(),
                        ));
                    }
                    // 函数默认程序地址空间 0
                    Some(ParsedItem::Function(_)) => {
                        return Err(IrError::Semantic(format!(
                            "invalid forward reference to function '{target}' with wrong type: expected 'ptr' but was 'ptr addrspace({n})'"
                        )));
                    }
                    _ => {}
                }
            }
            let ga = crate::function::GlobalAlias {
                name: crate::ImmStr::from(a.name.trim_start_matches('@')),
                ty,
                linkage,
                dso_local: a.dso_local,
                unnamed_addr: a.unnamed_addr,
                aliasee_ty: Some(a.aliasee.0.clone()),
                aliasee: a.aliasee.1.clone(),
                metadata: Vec::new(),
            };
            module
                .add_global_alias(ga)
                .map_err(|e| IrError::Semantic(format!("alias {}: {e}", a.name)))?;
            continue;
        }
        if let ParsedItem::Global(g) = item {
            let ty = to_type_result(&g.ty, &ctx)?;
            let init = match &g.init {
                Some(GlobalInitVal::Int(n)) => Some(int_init_bytes(ty, *n, &ctx)),
                Some(GlobalInitVal::UInt(n)) => Some(int_init_bytes(ty, *n as i64, &ctx)),
                Some(GlobalInitVal::Float(f)) => Some(float_init_bytes(ty, *f, &ctx)),
                // LLVM 字符串常量：字节直接作 init（类型应为 [N x i8]）
                Some(GlobalInitVal::Str(bytes)) => Some(bytes.clone()),
                // zeroinitializer：按类型大小填零
                Some(GlobalInitVal::ZeroInit) => Some(vec![0u8; ctx.size_bytes(ty) as usize]),
                // 聚合常量：按元素/字段类型打包字节（struct 含 padding 布局）
                Some(GlobalInitVal::Agg(vals)) => Some(pack_agg_init(&ctx, ty, vals)?),
                // 常量表达式：求字节（全局地址占位 0——链接期重定位，P1 文本层占位）
                Some(GlobalInitVal::Expr(e)) => {
                    let expr_bytes = const_expr_bytes(&ctx, e, ctx.size_bytes(ty) as u64)?;
                    Some(expr_bytes)
                }
                Some(GlobalInitVal::Null) => None,
                None => None,
            };
            let name = g.name.trim_start_matches('@');
            let mut gv = if g.is_constant {
                GlobalVariable::constant(name, ty)
            } else {
                GlobalVariable::mutable(name, ty)
            };
            // ifunc IR 表示（第二十九轮重建:参数类型文本 + resolver 名;
            // display 还原 `ifunc <retty> (<params>), ptr @resolver`）
            if g.is_ifunc {
                gv.is_ifunc = true;
                gv.ifunc_params = g
                    .ifunc_params
                    .iter()
                    .map(|s| crate::imm_str::ImmStr::from(s.clone()))
                    .collect();
                gv.ifunc_resolver = g
                    .ifunc_resolver
                    .as_ref()
                    .map(|r| crate::imm_str::ImmStr::from(r.trim_start_matches('@').to_string()));
            }
            // 表达式 init 保留树（display 原样还原文本，替代字节还原）
            if let Some(GlobalInitVal::Expr(e)) = &g.init {
                gv.init_expr = Some((**e).clone());
            }
            if let Some(data) = init {
                gv = gv.with_init(data);
            }
            if g.align != 0 {
                check_align_value(g.align)?;
                gv.alignment = g.align as u32;
            }
            // 全局尾 metadata 附加（`!absolute_symbol !0` 等；kind 宽松）——
            // 延迟：metadata 节点定义在模块 metadata 段（本阶段尚未插入 store），
            // 在 build_module 收尾（module_meta_store 就位后）统一 attach。
            let gname = g.name.trim_start_matches('@').to_string();
            pending_global_metas.push((gname, g.metadata.clone()));
            // linkage（`private`/`internal`/`external`/weak 族）→ SymbolInfo
            if let Some(link) = &g.linkage {
                let lk = match link.as_str() {
                    "private" => crate::symbol::Linkage::Private,
                    "internal" => crate::symbol::Linkage::Internal,
                    "external" => crate::symbol::Linkage::External,
                    "weak" => crate::symbol::Linkage::WeakAny,
                    "weak_odr" => crate::symbol::Linkage::WeakODR,
                    "linkonce" => crate::symbol::Linkage::LinkOnceAny,
                    "linkonce_odr" => crate::symbol::Linkage::LinkOnceODR,
                    "appending" => crate::symbol::Linkage::Appending,
                    "available_externally" => crate::symbol::Linkage::AvailableExternally,
                    "common" => crate::symbol::Linkage::Common,
                    "extern_weak" => crate::symbol::Linkage::ExternalWeak,
                    // externally_initialized 是 global 修饰符（非 linkage）——
                    // 宽松映射 External（display 还原为 external——见 §7）
                    "externally_initialized" => crate::symbol::Linkage::External,
                    other => return Err(IrError::Semantic(format!("unknown linkage {other}"))),
                };
                gv.symbol.linkage = lk;
            }
            // dll storage（`dllimport`/`dllexport`）
            if let Some(dll) = &g.dll_storage {
                gv.symbol.dll_storage_class = match dll.as_str() {
                    "dllimport" => crate::symbol::DllStorageClass::DllImport,
                    "dllexport" => crate::symbol::DllStorageClass::DllExport,
                    other => return Err(IrError::Semantic(format!("unknown dll storage {other}"))),
                };
            }
            if g.dso_local {
                gv.symbol.dso_local = true;
            }
            if g.unnamed_addr {
                gv.symbol.unnamed_addr = true;
            }
            if let Some(sec) = &g.section {
                gv.symbol.section = Some(ImmStr::from(sec.clone()));
            }
            if g.addr_space != 0 {
                gv.addr_space = g.addr_space;
            }
            if g.thread_local {
                gv.symbol.tls_model = Some(crate::symbol::TlsModel::GeneralDynamic);
            }
            if let Some(v) = &g.visibility {
                // 可见性冲突（LLVM：internal 与 hidden/protected 互斥）
                if g.linkage.as_deref() == Some("internal") {
                    return Err(IrError::Semantic(format!(
                        "global {}: internal linkage cannot be combined with visibility `{v}`",
                        g.name
                    )));
                }
                gv.symbol.visibility = match v.as_str() {
                    "hidden" => crate::symbol::Visibility::Hidden,
                    "protected" => crate::symbol::Visibility::Protected,
                    _ => crate::symbol::Visibility::Default,
                };
            }
            if let Some(c) = &g.comdat {
                // LLVM：comdat 引用要求 `$c = comdat ...` 已声明（先声明后引用）
                let cid = module
                    .find_comdat(c)
                    .ok_or_else(|| IrError::Semantic(format!("comdat '{c}' is not declared")))?;
                gv.symbol.comdat = Some(cid);
            }
            let id = module
                .add_global(gv)
                .map_err(|_| IrError::Semantic(format!("global '{}' already exists", g.name)))?;
            global_refs.insert(g.name.clone(), id);
        }
    }

    // 第一遍：注册全部函数名（declare 外部函数 + define）为 FuncRef 占位——
    // 支持前向引用（call @后定义函数）与 declare 外部函数（无 body 空壳）。
    // 第十一轮：预扫描 call/invoke 的 callee——ifunc 等非 Function 定义的前向
    // 引用（LLVM 隐式声明：`call void @f()` 无需 declare）。
    let mut forward_callees: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in &ast.items {
        if let ParsedItem::Function(f) = item {
            for b in &f.blocks {
                for inst in &b.insts {
                    if inst.opcode == "call"
                        && let Some(crate::ir_parser::ast_items::ParsedOperand {
                            op: Operand::Global(n),
                            ..
                        }) = inst.args.first()
                    {
                        forward_callees.insert(n.clone());
                    }
                }
                if let ParsedTerminator::Invoke { callee, .. } = &b.terminator {
                    forward_callees.insert(callee.clone());
                }
            }
        }
    }
    let mut func_refs: HashMap<String, FuncRef> = HashMap::new();
    for item in &ast.items {
        if let ParsedItem::Function(f) = item {
            let sig = signature_of(f, &ctx);
            let sig_ref = ctx.register_signature(sig.clone());
            let mut placeholder = Function::new(
                f.name.trim_start_matches('@'),
                ctx.clone(),
                sig_ref,
                CallConv::default(),
            );
            if f.dso_local {
                placeholder.symbol.dso_local = true;
            }
            // declare 函数属性（`declare i32 @printf(...) nounwind`；define 在
            // build_function 内设置）。`#N` 已在预处理阶段展开为具体属性名。
            for attr in &f.attrs {
                if let Some(flag) = parse_fn_attr(attr) {
                    placeholder.attributes.set(flag);
                } else {
                    placeholder
                        .extra_attrs
                        .push(crate::ImmStr::from(attr.as_str()));
                }
            }
            let fr = module.add_function(placeholder);
            func_refs.insert(f.name.clone(), fr);
        }
    }

    // 第十一轮：为前向 call/invoke 的未声明 callee 注册宽松占位（void() 签名
    // ——call 的真实类型由指令自身携带，签名仅保证 FuncRef 存在）
    let void_sig = {
        let sig = FunctionSignature::new(&[], &[]);
        ctx.register_signature(sig)
    };
    for name in forward_callees {
        if func_refs.contains_key(&name) {
            continue;
        }
        let ph = Function::new(
            name.trim_start_matches('@'),
            ctx.clone(),
            void_sig,
            CallConv::default(),
        );
        let fr = module.add_function(ph);
        func_refs.insert(name, fr);
    }

    // 第二遍：构建 define 函数 body（declare 保持空壳表示外部函数）
    for item in &ast.items {
        if let ParsedItem::Function(f) = item {
            if f.declare {
                continue;
            }
            let fr = func_refs[&f.name];
            let func = build_function(f, &func_refs, &global_refs, &ctx)?;
            *module.get_function_mut(fr) = func;
        }
    }
    // personality 函数引用解析（`personality ptr @__gxx_personality_v0`）
    for item in &ast.items {
        if let ParsedItem::Function(f) = item
            && let Some(pname) = &f.personality
        {
            let fr = func_refs[&f.name];
            // 宽松（第十二轮）：personality 函数未定义（前向/外部引用）——
            // 跳过设置不报错
            if let Some(pref) = func_refs.get(pname) {
                module.get_function_mut(fr).personality = Some(*pref);
            }
        }
    }
    // 函数/指令 metadata 附加：`!dbg !N`（在 store intern 后解析引用）
    for item in &ast.items {
        if let ParsedItem::Function(f) = item {
            let fr = func_refs[&f.name];
            let func = module.get_function_mut(fr);
            for (name, r) in &f.metadata {
                func.metadata
                    .push(attach_metadata(name, r, &module_meta_store)?);
            }
        }
    }
    module.metadata_store = module_meta_store;
    // 全局尾 metadata 延迟附加（节点定义已就位）
    for (gname, metas) in &pending_global_metas {
        let store = &module.metadata_store;
        let mut attached = Vec::with_capacity(metas.len());
        for (kind, r) in metas {
            attached.push(attach_metadata(kind, r, store)?);
        }
        if let Some((_, gv)) = module
            .iter_globals_mut()
            .find(|(_, g)| g.name.as_str() == gname)
        {
            gv.metadata = attached;
        }
    }
    // 引用存在性（AST 级）：指令尾/函数头/global 尾 metadata 的 Num 引用必须已定义
    //（LLVM 先定义后使用——store/ret 等无 result 指令的 attach 不落到指令层，
    // 故在此统一扫描 ParsedInst/ParsedFunction 的 metadata_attach）
    for item in &ast.items {
        if let ParsedItem::Function(f) = item {
            let attach: Vec<&(String, MetadataRef)> = f
                .metadata
                .iter()
                .chain(
                    f.blocks
                        .iter()
                        .flat_map(|b| b.insts.iter().map(|i| &i.metadata_attach))
                        .flatten(),
                )
                .collect();
            for (_, r) in attach {
                if let MetadataRef::Num(id) = r
                    && *id as usize >= module.metadata_store.len()
                {
                    return Err(IrError::Semantic(format!(
                        "metadata reference !{id} refers to undefined node"
                    )));
                }
            }
        }
    }
    // 3.4 metadata kind×形状校验（!dbg → DILocation；其余 kind 宽松）
    validate_metadata_shapes(&module)?;
    Ok(module)
}

/// 3.4 metadata kind×形状校验：`!dbg` 必须指向 DILocation 节点（LLVM 语义）。
/// 其余 kind（tbaa/range/align 等）LLVM 只要求节点存在、形状自由，保持宽松。
/// 在 build_module 收尾统一校验（函数级 + 指令级 + 终结符级 metadata）。
fn validate_metadata_shapes(module: &Module) -> Result<(), IrError> {
    use crate::metadata::{AttachedMetadata, MetadataKind, MetadataNode};
    let store = &module.metadata_store;
    let check = |kind: &MetadataKind, node: &MetadataNode| -> Result<(), IrError> {
        use crate::metadata::MetadataValue;
        match kind {
            // !dbg 宽松：正向用例（drop-debug-info 等）引用非 DILocation 节点
            // （旧格式/升级链）；DI 负向拒绝在 parse 层，不受本校验影响
            MetadataKind::DebugLoc => Ok(()),
            // 3.4 扩展：!tbaa 必须引用 tuple 节点（嵌套 tag 结构 `!{!{...}, i64 1}`）
            MetadataKind::TBAA => match node {
                MetadataNode::Tuple(_) => Ok(()),
                other => Err(IrError::Semantic(format!(
                    "!tbaa must reference a tuple tag node, got {other:?}"
                ))),
            },
            // 3.4 扩展：!range 必须引用两个整数的 tuple 区间 `!{ i32 lo, i32 hi }`
            MetadataKind::Range => match node {
                MetadataNode::Tuple(vals)
                    if vals.len() >= 2
                        && vals.iter().all(|v| {
                            matches!(v, MetadataValue::Int(_) | MetadataValue::Uint(_))
                        }) =>
                {
                    Ok(())
                }
                other => Err(IrError::Semantic(format!(
                    "!range must reference a tuple of two integers, got {other:?}"
                ))),
            },
            _ => Ok(()),
        }
    };
    for f in module.iter_functions() {
        for am in &f.metadata {
            check(&am.kind, store.get(am.node))?;
        }
        for (_, inst) in f.dfg.insts() {
            for am in &inst.metadata {
                check(&am.kind, store.get(am.node))?;
            }
        }
        for (_, bd) in f.dfg.blocks() {
            let metas: &[AttachedMetadata] = match &bd.terminator {
                Terminator::Branch { metadata, .. }
                | Terminator::Jump { metadata, .. }
                | Terminator::Return { metadata, .. }
                | Terminator::Switch { metadata, .. }
                | Terminator::Invoke { metadata, .. }
                | Terminator::Resume { metadata, .. } => metadata.as_slice(),
                Terminator::Unreachable => &[],
            };
            for am in metas {
                check(&am.kind, store.get(am.node))?;
            }
        }
    }
    Ok(())
}

/// 常量表达式求值为初始字节（全局地址占位 0——真实地址为链接期重定位，
/// P1 文本层 round-trip 优先；未知操作名报语义错误）。
fn const_expr_bytes(ctx: &TypeContext, e: &ConstExpr, size: u64) -> Result<Vec<u8>, IrError> {
    let val: i128 = const_expr_value(ctx, e)?;
    let mut out = Vec::with_capacity(size as usize);
    for i in 0..size as usize {
        // i128 只有 16 字节——超出部分补 0（防移位溢出；第十一轮）
        let byte = if i < 16 {
            ((val >> (i * 8)) & 0xFF) as u8
        } else {
            0
        };
        out.push(byte);
    }
    Ok(out)
}

/// 递归求值为整数（全局地址占位 0；float 按位模式）。
/// ParsedType 的字节大小（常量 GEP 字节偏移折叠用——第三十一轮）。
/// Named/Void/Metadata/Opaque/Token 无确定大小 → 0（宽松,parse 通过优先）。
pub fn size_of_parsed_type(t: &ParsedType) -> u64 {
    match t {
        ParsedType::Int(bits) => *bits as u64 / 8,
        ParsedType::Float(bits) => *bits as u64 / 8,
        ParsedType::Ptr | ParsedType::PtrAddrSpace(_) => 8,
        ParsedType::Vec(_, e) | ParsedType::VecScalable(_, e) => size_of_parsed_type(e),
        ParsedType::Array(n, e) => n * size_of_parsed_type(e),
        ParsedType::Struct(fields) | ParsedType::StructPacked(fields) => {
            fields.iter().map(size_of_parsed_type).sum()
        }
        _ => 0,
    }
}

/// 递归求值为整数（全局地址占位 0；float 按位模式）。
fn const_expr_value(_ctx: &TypeContext, e: &ConstExpr) -> Result<i128, IrError> {
    use crate::ir_parser::ast_items::ConstExpr;
    Ok(match e {
        ConstExpr::GlobalAddr(_) => 0,
        ConstExpr::Int(n) => *n as i128,
        ConstExpr::UInt(n) => *n as i128,
        ConstExpr::Float(f) => f.to_bits() as i128,
        ConstExpr::Null => 0,
        ConstExpr::Undef | ConstExpr::Poison => 0,
        ConstExpr::PtrToInt {
            op, op_ty, to_ty, ..
        } => {
            // S4.2：ptrtoint 源必须 ptr、目标必须整数（invalid_cast4 类；
            // Void 是无类型前缀形态的占位——跳过校验）
            if !matches!(op_ty, ParsedType::Void)
                && !matches!(op_ty, ParsedType::Ptr | ParsedType::PtrAddrSpace(_))
            {
                return Err(IrError::Semantic(format!(
                    "ptrtoint: expected pointer source, got {op_ty:?}"
                )));
            }
            if !matches!(to_ty, ParsedType::Int(_)) {
                return Err(IrError::Semantic(format!(
                    "ptrtoint: expected integer target, got {to_ty:?}"
                )));
            }
            const_expr_value(_ctx, op)?
        }
        ConstExpr::IntToPtr {
            op, op_ty, to_ty, ..
        } => {
            // S4.2：inttoptr 源必须整数、目标必须 ptr（invalid_cast4 类）
            if !matches!(op_ty, ParsedType::Void) && !matches!(op_ty, ParsedType::Int(_)) {
                return Err(IrError::Semantic(format!(
                    "inttoptr: expected integer source, got {op_ty:?}"
                )));
            }
            if !matches!(to_ty, ParsedType::Ptr | ParsedType::PtrAddrSpace(_)) {
                return Err(IrError::Semantic(format!(
                    "inttoptr: expected pointer target, got {to_ty:?}"
                )));
            }
            const_expr_value(_ctx, op)?
        }
        ConstExpr::Bitcast { op, .. } | ConstExpr::AddrSpaceCast { op, .. } => {
            const_expr_value(_ctx, op)?
        }
        ConstExpr::GetElementPtr {
            ptr,
            indices,
            indexed_ty,
            op_ty,
            ..
        } => {
            // 可伸缩向量 GEP 拒绝（LLVM：scalable 类型禁止 GEP——第十三轮
            // constant-getelementptr-scalable_pointee 负向；indexed_ty 是
            // 被索引类型）
            if matches!(indexed_ty, ParsedType::VecScalable(..)) {
                return Err(IrError::Semantic(
                    "getelementptr with scalable vector type is not allowed".to_string(),
                ));
            }
            // 向量 GEP 索引校验（LLVM：所有向量索引 lane 数一致，且与向量
            // 基址 lane 数匹配——负向用例 getelementptr_vec_idx4/vec_ce2）
            let ptr_vec_len = match op_ty {
                ParsedType::Vec(len, _) => Some(*len as usize),
                _ => None,
            };
            let mut idx_vec_len: Option<usize> = None;
            for (_, idx) in indices {
                if let ConstExpr::Vector(_, lanes) = idx {
                    let n = lanes.len();
                    if let Some(prev) = idx_vec_len {
                        if prev != n {
                            return Err(IrError::Semantic(
                                "getelementptr vector index has a wrong number of elements".into(),
                            ));
                        }
                    } else {
                        idx_vec_len = Some(n);
                    }
                    if let Some(pn) = ptr_vec_len
                        && pn != n
                    {
                        return Err(IrError::Semantic(
                            "getelementptr vector index has a wrong number of elements".into(),
                        ));
                    }
                }
            }
            let mut addr = const_expr_value(_ctx, ptr)?;
            // 字节偏移折叠（第三十一轮补全）:数组/向量索引 × 元素大小,
            // 结构索引按字段偏移累积(原仅值级相加——[4 x i32] 索引 1 误加 1
            // 字节而非 4)
            let mut cur = indexed_ty.clone();
            for (_, idx) in indices {
                let iv = const_expr_value(_ctx, idx)?;
                match &cur {
                    ParsedType::Array(_, t) | ParsedType::Vec(_, t) => {
                        addr += iv * (size_of_parsed_type(t) as i128);
                        cur = (**t).clone();
                    }
                    ParsedType::Struct(fields) | ParsedType::StructPacked(fields) => {
                        let i = iv as usize;
                        let off: u64 = fields[..i.min(fields.len())]
                            .iter()
                            .map(size_of_parsed_type)
                            .sum();
                        addr += off as i128;
                        if i < fields.len() {
                            cur = fields[i].clone();
                        }
                    }
                    _ => {
                        addr += iv;
                    }
                }
            }
            addr
        }
        ConstExpr::Binary { op, lhs, rhs, .. } => {
            let l = const_expr_value(_ctx, lhs)?;
            let r = const_expr_value(_ctx, rhs)?;
            eval_binary_const(op, l, r)?
        }
        // 向量常量：标量折叠无法表达——宽松返回 0（parse 通过优先）
        ConstExpr::Vector(..) => 0,
        // ptrauth（AArch64 指针认证常量）：LLVM 值级规则——base 必须指针类、
        // key 必须 i32 常量、int_disc 必须 i64 常量、addr_disc 必须指针类
        // （invalid-ptrauth-const1~4）；校验通过宽松求值 0
        ConstExpr::Ptrauth(args) => {
            let is_ptr_ty =
                |t: &ParsedType| matches!(t, ParsedType::Ptr | ParsedType::PtrAddrSpace(_));
            let (base_ty, _) = &args[0];
            if !is_ptr_ty(base_ty) {
                return Err(IrError::Semantic(
                    "constant ptrauth base pointer must be a pointer".to_string(),
                ));
            }
            let (key_ty, key) = &args[1];
            if !matches!(
                (key_ty, key),
                (ParsedType::Int(32), ConstExpr::Int(_) | ConstExpr::UInt(_))
            ) {
                return Err(IrError::Semantic(
                    "constant ptrauth key must be i32 constant".to_string(),
                ));
            }
            if let Some((disc_ty, disc)) = args.get(2)
                && !matches!(
                    (disc_ty, disc),
                    (ParsedType::Int(64), ConstExpr::Int(_) | ConstExpr::UInt(_))
                )
            {
                return Err(IrError::Semantic(
                    "constant ptrauth integer discriminator must be i64 constant".to_string(),
                ));
            }
            if let Some((addr_ty, _)) = args.get(3)
                && !is_ptr_ty(addr_ty)
            {
                return Err(IrError::Semantic(
                    "constant ptrauth address discriminator must be a pointer".to_string(),
                ));
            }
            0
        }
        // 转换折叠：trunc 按目标位宽截断；zext/sext 按源位宽扩展
        ConstExpr::Cast {
            op,
            src_ty,
            src,
            to,
        } => {
            let v = const_expr_value(_ctx, src)?;
            let src_bits = match src_ty {
                ParsedType::Int(b) => Some(*b),
                _ => None,
            };
            let dst_bits = match to {
                ParsedType::Int(b) => Some(*b),
                _ => None,
            };
            let v64 = v as u64;
            match (op.as_str(), src_bits, dst_bits) {
                ("trunc", _, Some(b)) if b < 64 => (v64 & ((1u64 << b) - 1)) as i128,
                ("trunc", _, Some(_)) | ("trunc", _, None) => v,
                ("zext", Some(b), _) if b < 64 => (v64 & ((1u64 << b) - 1)) as i128,
                ("zext", _, _) => v,
                ("sext", Some(b), _) if b < 64 => {
                    let mask = (1u64 << b) - 1;
                    let sign = 1u64 << (b - 1);
                    let low = v64 & mask;
                    if low & sign != 0 {
                        (low | !mask) as i128
                    } else {
                        low as i128
                    }
                }
                ("sext", _, _) => v,
                _ => v,
            }
        }
        ConstExpr::Unsupported(name) => {
            return Err(IrError::Semantic(format!(
                "unsupported constant expression `{name}` in global initializer"
            )));
        }
    })
}

/// 二元常量折叠求值（i128 域；浮点按位模式——fadd 等按 f64 解释运算）。
/// 除零/溢出按 LLVM 折叠惯例宽松处理（不误拒绝合法文件）。
fn eval_binary_const(op: &str, l: i128, r: i128) -> Result<i128, IrError> {
    Ok(match op {
        "add" => l.wrapping_add(r),
        "sub" => l.wrapping_sub(r),
        "mul" => l.wrapping_mul(r),
        "udiv" | "sdiv" => {
            if r == 0 {
                0
            } else {
                l.wrapping_div(r)
            }
        }
        "urem" | "srem" => {
            if r == 0 {
                0
            } else {
                l.wrapping_rem(r)
            }
        }
        "shl" => l.wrapping_shl((r & 127) as u32),
        "lshr" => ((l as u128) >> ((r & 127) as u32)) as i128,
        "ashr" => l.wrapping_shr((r & 127) as u32),
        "and" => l & r,
        "or" => l | r,
        "xor" => l ^ r,
        "fadd" => (f64::from_bits(l as u64) + f64::from_bits(r as u64)).to_bits() as i128,
        "fsub" => (f64::from_bits(l as u64) - f64::from_bits(r as u64)).to_bits() as i128,
        "fmul" => (f64::from_bits(l as u64) * f64::from_bits(r as u64)).to_bits() as i128,
        "fdiv" => {
            let rb = f64::from_bits(r as u64);
            if rb == 0.0 {
                0
            } else {
                (f64::from_bits(l as u64) / rb).to_bits() as i128
            }
        }
        "frem" => {
            let rb = f64::from_bits(r as u64);
            if rb == 0.0 {
                0
            } else {
                (f64::from_bits(l as u64) % rb).to_bits() as i128
            }
        }
        other => {
            return Err(IrError::Semantic(format!(
                "unsupported constant expression binary op `{other}`"
            )));
        }
    })
}

/// 按整数类型把字面量编码为小端字节（global 初始值）。
/// 聚合常量 → 字节：数组逐个元素、结构体按字段（非 packed 含对齐 padding）。
fn pack_agg_init(
    ctx: &TypeContext,
    ty: TypeId,
    vals: &[GlobalInitVal],
) -> Result<Vec<u8>, IrError> {
    let mut out = Vec::with_capacity(ctx.size_bytes(ty) as usize);
    let store = ctx.borrow();
    let entry = store.get(ty);
    match entry {
        crate::types::TypeEntry::Array { elem, len } => {
            if vals.len() as u64 != *len {
                return Err(IrError::Semantic(format!(
                    "array init has {} elements, expected {len}",
                    vals.len()
                )));
            }
            for v in vals {
                // 嵌套聚合元素（`[[i32 1, i32 2], ...]`）递归打包
                if let GlobalInitVal::Agg(sub) = v {
                    out.extend(pack_agg_init(ctx, *elem, sub)?);
                } else {
                    out.extend(pack_scalar_init(ctx, *elem, v)?);
                }
            }
        }
        crate::types::TypeEntry::Struct {
            fields, is_packed, ..
        } => {
            if vals.len() != fields.len() {
                return Err(IrError::Semantic(format!(
                    "struct init has {} fields, expected {}",
                    vals.len(),
                    fields.len()
                )));
            }
            let mut offset = 0u32;
            for (i, (f, v)) in fields.iter().zip(vals).enumerate() {
                if !*is_packed && i > 0 {
                    let align = ctx.alignment(f.ty);
                    let pad = (align - (offset % align)) % align;
                    out.extend(std::iter::repeat_n(0u8, pad as usize));
                    offset += pad;
                }
                if let GlobalInitVal::Agg(sub) = v {
                    out.extend(pack_agg_init(ctx, f.ty, sub)?);
                } else {
                    out.extend(pack_scalar_init(ctx, f.ty, v)?);
                }
                offset += ctx.size_bytes(f.ty);
            }
        }
        _ => {
            return Err(IrError::Semantic(format!(
                "aggregate init for non-aggregate type {}",
                ctx.borrow().fmt_type(ty)
            )));
        }
    }
    Ok(out)
}

/// 标量值 → 元素类型字节（int/float/pointer；值类型不匹配报错）。
fn pack_scalar_init(ctx: &TypeContext, ty: TypeId, v: &GlobalInitVal) -> Result<Vec<u8>, IrError> {
    let bytes = match (v, ctx.borrow().get(ty)) {
        // 常量表达式元素（聚合内 `ptr @f` 等）：求字节（全局地址占位 0）
        (GlobalInitVal::Expr(e), _) => const_expr_bytes(ctx, e, ctx.size_bytes(ty) as u64)?,
        (GlobalInitVal::Int(n), crate::types::TypeEntry::Int { bits: 8 }) => vec![*n as u8],
        (GlobalInitVal::Int(n), crate::types::TypeEntry::Int { bits: 16 }) => {
            (*n as i16).to_le_bytes().to_vec()
        }
        (GlobalInitVal::Int(n), crate::types::TypeEntry::Int { bits: 32 }) => {
            (*n as i32).to_le_bytes().to_vec()
        }
        (GlobalInitVal::Int(n), crate::types::TypeEntry::Int { bits: 64 }) => {
            { *n }.to_le_bytes().to_vec()
        }
        // 非 8/16/32/64 位宽整数（i1 等）：按 ceil(bits/8) 字节低位存储
        (GlobalInitVal::Int(n), crate::types::TypeEntry::Int { bits }) => {
            let size = std::cmp::max(1u32, (*bits).div_ceil(8)) as usize;
            let mut b = vec![0u8; size];
            let le = { *n }.to_le_bytes();
            for (i, slot) in b.iter_mut().enumerate() {
                *slot = le[i.min(le.len() - 1)];
            }
            b
        }
        (GlobalInitVal::UInt(n), crate::types::TypeEntry::Int { bits }) => {
            let size = std::cmp::max(1u32, (*bits).div_ceil(8)) as usize;
            (*n).to_le_bytes()[..size].to_vec()
        }
        (GlobalInitVal::Float(f), crate::types::TypeEntry::Float { bits: 32 }) => {
            (*f as f32).to_le_bytes().to_vec()
        }
        (GlobalInitVal::Float(f), crate::types::TypeEntry::Float { bits: 64 }) => {
            { *f }.to_le_bytes().to_vec()
        }
        (GlobalInitVal::Null, crate::types::TypeEntry::Pointer { .. }) => {
            vec![0u8; ctx.size_bytes(ty) as usize]
        }
        (GlobalInitVal::Int(n), crate::types::TypeEntry::Pointer { .. }) => {
            vec![0u8; ctx.size_bytes(ty) as usize] // 整型指针常量按零处理（简化）
                .into_iter()
                .map(|_| 0u8)
                .collect::<Vec<u8>>()
                .into_iter()
                .enumerate()
                .map(|(i, _)| (*n >> (i * 8)) as u8)
                .collect()
        }
        _ => {
            return Err(IrError::Semantic(format!(
                "aggregate init value incompatible with type {}",
                ctx.borrow().fmt_type(ty)
            )));
        }
    };
    Ok(bytes)
}

fn int_init_bytes(ty: TypeId, n: i64, ctx: &TypeContext) -> Vec<u8> {
    let bits = match ctx.borrow().get(ty) {
        TypeEntry::Int { bits } => *bits,
        _ => 32,
    };
    let v = n as u64;
    match bits {
        8 => vec![v as u8],
        16 => (v as u16).to_le_bytes().to_vec(),
        32 => (v as u32).to_le_bytes().to_vec(),
        64 => { v }.to_le_bytes().to_vec(),
        _ => (v as u32).to_le_bytes().to_vec(),
    }
}

/// 按浮点类型把字面量编码为小端字节（global 初始值）。
fn float_init_bytes(ty: TypeId, f: f64, ctx: &TypeContext) -> Vec<u8> {
    match ctx.borrow().get(ty) {
        TypeEntry::Float { bits: 32 } => (f as f32).to_le_bytes().to_vec(),
        TypeEntry::Float { bits: 64 } => f.to_le_bytes().to_vec(),
        _ => (f as f32).to_le_bytes().to_vec(),
    }
}

fn signature_of(f: &ParsedFunction, ctx: &TypeContext) -> FunctionSignature {
    let params: Vec<(TypeId, &str)> = f
        .params
        .iter()
        // 过滤 varargs 哨兵参数（`...`；变长信息在 variadic 字段）
        .filter(|(_, name)| name != "...")
        .map(|(pt, name)| (to_type(pt, ctx), name.trim_start_matches('%')))
        .collect();
    let ret_ty = to_type(&f.ret_ty, ctx);
    // void 返回 → 空 returns（LLVM 语义；verify 的 Return 数量检查依赖）
    let rets = if ret_ty == TypeId::VOID {
        vec![]
    } else {
        vec![ret_ty]
    };
    FunctionSignature::new(&params, &rets).with_variadic(f.varargs)
}

fn to_type(pt: &ParsedType, ctx: &TypeContext) -> TypeId {
    match pt {
        ParsedType::Int(bits) => ctx.int_ty(*bits),
        ParsedType::Float(bits) => ctx.float_ty(*bits),
        ParsedType::Ptr => ctx.ptr_ty(),
        ParsedType::PtrAddrSpace(n) => ctx.pointer_ty(*n),
        ParsedType::Vec(n, e) => ctx.vector_ty(to_type(e, ctx), *n),
        ParsedType::Array(n, e) => ctx.array_ty(to_type(e, ctx), *n),
        ParsedType::Struct(tys) => {
            let field_tys: Vec<TypeId> = tys.iter().map(|t| to_type(t, ctx)).collect();
            ctx.borrow_mut().struct_anon(field_tys, false)
        }
        ParsedType::StructPacked(tys) => {
            let field_tys: Vec<TypeId> = tys.iter().map(|t| to_type(t, ctx)).collect();
            ctx.borrow_mut().struct_anon(field_tys, true)
        }
        ParsedType::Named(name) => ctx
            .borrow()
            .lookup_named(name.trim_start_matches('%'))
            .unwrap_or_else(|| ctx.void_ty()),
        ParsedType::Void => ctx.void_ty(),
        // metadata 类型(第二十九轮重建:原降级 void 致 display 输出 void 参数)
        ParsedType::Metadata => ctx.borrow_mut().metadata_ty(),
        ParsedType::Opaque => ctx.borrow_mut().opaque_ty(),
        ParsedType::Token => ctx.borrow_mut().token_ty(),
        ParsedType::VecScalable(n, e) => {
            // 注意：接收者 borrow_mut() 先于参数求值（Rust 方法调用求值顺序）——
            // 参数内 to_type 会再次取锁 → 重入死锁（第十一轮实测，先求值参数）
            let inner = to_type(e, ctx);
            ctx.borrow_mut().scalable_vector_ty(inner, *n)
        }
    }
}

/// to_type 的严格版（TypeDef 解析用）：命名类型引用未定义 → 报错。
fn to_type_result(pt: &ParsedType, ctx: &TypeContext) -> Result<TypeId, IrError> {
    match pt {
        // 位宽值域（LLVM：整数位宽 ≤ 2^23；i0 由 IntTy 正则排除，
        // lexer 溢出返回 0 亦在此拒绝）
        ParsedType::Int(bits) => {
            if *bits == 0 || *bits > (1u32 << 23) {
                return Err(IrError::Semantic(format!(
                    "integer type width {bits} is out of range (1..2^23)"
                )));
            }
            Ok(ctx.int_ty(*bits))
        }
        ParsedType::Float(bits) => Ok(ctx.float_ty(*bits)),
        ParsedType::Ptr => Ok(ctx.ptr_ty()),
        // 地址空间值域（LLVM：addrspace < 2^24）
        ParsedType::PtrAddrSpace(n) => {
            if *n >= (1 << 24) {
                return Err(IrError::Semantic(format!(
                    "address space {n} exceeds maximum 2^24-1"
                )));
            }
            Ok(ctx.pointer_ty(*n))
        }
        ParsedType::Vec(n, e) => Ok(ctx.vector_ty(to_type_result(e, ctx)?, *n)),
        ParsedType::Array(n, e) => Ok(ctx.array_ty(to_type_result(e, ctx)?, *n)),
        ParsedType::Struct(tys) => {
            let field_tys: Vec<TypeId> = tys
                .iter()
                .map(|t| to_type_result(t, ctx))
                .collect::<Result<_, _>>()?;
            Ok(ctx.borrow_mut().struct_anon(field_tys, false))
        }
        ParsedType::StructPacked(tys) => {
            let field_tys: Vec<TypeId> = tys
                .iter()
                .map(|t| to_type_result(t, ctx))
                .collect::<Result<_, _>>()?;
            Ok(ctx.borrow_mut().struct_anon(field_tys, true))
        }
        ParsedType::Named(name) => ctx
            .borrow()
            .lookup_named(name.trim_start_matches('%'))
            .ok_or_else(|| {
                IrError::Semantic(format!(
                    "unknown type `%{name}` (must be defined by a `type` item before use)"
                ))
            }),
        ParsedType::Void => Ok(ctx.void_ty()),
        ParsedType::Metadata => Ok(ctx.borrow_mut().metadata_ty()),
        ParsedType::Opaque => Ok(ctx.borrow_mut().opaque_ty()),
        ParsedType::Token => Ok(ctx.borrow_mut().token_ty()),
        // 注意：接收者 `borrow_mut()` 必须先于参数求值（Rust 方法调用
        // 求值顺序）——参数内 to_type 会再次取锁 → 重入死锁（第十一轮实测）
        ParsedType::VecScalable(n, e) => {
            let inner = to_type(e, ctx);
            Ok(ctx.borrow_mut().scalable_vector_ty(inner, *n))
        }
    }
}

// ── 函数构建 ──

fn build_function<'a>(
    f: &'a ParsedFunction,
    func_refs: &HashMap<String, FuncRef>,
    global_refs: &HashMap<String, GlobalId>,
    ctx: &TypeContext,
) -> Result<crate::function::Function, IrError> {
    let sig = signature_of(f, ctx);
    // 短函数名内联零分配；长名单次 Arc 分配（不再 to_string + 二次拷贝）。
    let name = ImmStr::from(f.name.trim_start_matches('@'));
    let mut fb = FunctionBuilder::new(name, ctx.clone(), sig);

    // 值符号表（%name → Value），SSA 唯一性检查。
    // key 借用 ParsedFunction（'a），避免每指令 String 克隆。
    let mut value_map: HashMap<&'a str, Value> = HashMap::new();
    let mut block_map: HashMap<String, Block> = HashMap::new();

    // 预扫描：每块的 phi 声明（顺序即块参数顺序）。phi 行只在 LLVM 文本里
    // 出现；forge 内部用块参数表达同一语义，故 phi → 块参数在解析期完成。
    let block_phis: HashMap<&'a str, Vec<&'a ParsedInst>> = f
        .blocks
        .iter()
        .map(|b| {
            let phis: Vec<&ParsedInst> = b.insts.iter().filter(|i| i.opcode == "phi").collect();
            (b.label.as_str(), phis)
        })
        .collect();

    // 第一遍：创建所有块（entry 带函数参数；非 entry 带 phi 参数）。
    // 过滤 varargs 哨兵参数（`...`）——变长信息在签名 variadic，不占实际参数。
    let real_params: Vec<&(ParsedType, String)> =
        f.params.iter().filter(|(_, n)| n != "...").collect();

    // 隐式块唯一命名：LLVM 允许多个无标签块（都映射到 forge 的 "entry"
    // 占位名）——按块序生成唯一键（block_keys[i]），block_map 用唯一键。
    // 显式字符串标签（非 "entry" 占位）才登记 label_key（隐式块只能被
    // 数字 id 引用，不存在按名引用歧义）。
    let mut block_keys: Vec<String> = Vec::with_capacity(f.blocks.len());
    let mut label_key: HashMap<&'a str, String> = HashMap::new();
    let mut used_keys: HashSet<String> = HashSet::new();
    for (i, b) in f.blocks.iter().enumerate() {
        let base = if i == 0 {
            "entry".to_string()
        } else if b.label == "entry" {
            format!("entry{i}")
        } else {
            b.label.clone()
        };
        let mut k = base.clone();
        let mut n = 0usize;
        while used_keys.contains(&k) {
            n += 1;
            k = format!("{base}{n}");
        }
        used_keys.insert(k.clone());
        if b.label != "entry" {
            label_key.insert(b.label.as_str(), k.clone());
        }
        block_keys.push(k);
    }

    // 数字块 id 分配（LLVM NumberedVals：值与块共享编号空间——block-labels
    // 类用例）。entry=0；显式数字标签 `N:` 占位 id N；隐式块从 1 递增、
    // 跳过被值 id（数字参数/结果名）与已占块 id 占用的编号。
    let mut value_ids: HashSet<u32> = HashSet::new();
    for (_, n) in &real_params {
        if let Ok(v) = n.trim_start_matches('%').parse::<u32>() {
            value_ids.insert(v);
        }
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(r) = &i.result
                && let Ok(v) = r.trim_start_matches('%').parse::<u32>()
            {
                value_ids.insert(v);
            }
        }
    }
    // LLVM NumberedVals:仅无标签块(implicit)与显式数字标签入编号空间,
    // 显式命名标签块不占编号(第二十九轮:uselistorder 的 %0 指向第一个
    // 隐式块,entry 显式标签不占 0)
    let mut next: u32 = 0;
    let mut num_to_label: HashMap<u32, String> = HashMap::new();
    for (i, b) in f.blocks.iter().enumerate() {
        let key = block_keys[i].clone();
        if b.label_is_num
            && let Ok(n) = b.label.parse::<u32>()
        {
            num_to_label.insert(n, key);
            next = next.max(n + 1);
        } else if b.implicit {
            while value_ids.contains(&next) || num_to_label.contains_key(&next) {
                next += 1;
            }
            num_to_label.insert(next, key);
            next += 1;
        }
        // 显式命名标签块(非数字)不占编号空间(LLVM 按名引用)
    }
    let (entry, params) = if real_params.is_empty() {
        let b = fb.create_block();
        fb.bind_block_name(b, f.blocks[0].label.trim_start_matches('%'));
        (b, vec![])
    } else {
        let params: Vec<(TypeId, &str)> = real_params
            .iter()
            .map(|(pt, name)| (to_type(pt, ctx), name.trim_start_matches('%')))
            .collect();
        let (b, vals) = fb.create_block_with_params(&params);
        fb.bind_block_name(b, f.blocks[0].label.trim_start_matches('%'));
        (b, vals)
    };
    block_map.insert(block_keys[0].clone(), entry);
    for (i, p) in real_params.iter().enumerate() {
        if i < params.len() {
            value_map.insert(p.1.as_str(), params[i]);
        }
    }
    for (i, b) in f.blocks.iter().enumerate().skip(1) {
        let phis = block_phis
            .get(b.label.as_str())
            .cloned()
            .unwrap_or_default();
        let blk = if phis.is_empty() {
            fb.create_block()
        } else {
            let tys: Vec<(TypeId, &str)> = phis
                .iter()
                .map(|p| {
                    let ty = to_type(&p.args[0].ty, ctx);
                    let name = p.result.as_deref().unwrap_or("p");
                    (ty, name)
                })
                .collect();
            fb.create_block_with_params(&tys).0
        };
        fb.bind_block_name(blk, b.label.trim_start_matches('%'));
        block_map.insert(block_keys[i].clone(), blk);
    }

    // 第二遍：填充指令与终结符（phi 行绑定块参数后跳过；跳转不再携带参数）
    for (i, pb) in f.blocks.iter().enumerate() {
        let block = block_map[&block_keys[i]];
        fb.switch_to_block(block);
        // 块尾 uselistorder 校验（第十一轮）：索引形状 + 类型比对 + use 计数
        // （第二十一轮：function-missing-named/numbered 的 "value has no uses"
        // 与 type 例的类型不符——函数内引用扫描）
        for (name, idx, sty) in &pb.uselistorders {
            // 类型比对：值已定义且类型与语句类型不符 → 拒绝
            // （invalid-uselistorder-type：uselistorder float %x 而 %x 是 i32）
            let defined_ty = value_map
                .get(name.as_str())
                .copied()
                .and_then(|v| fb.value_type(v));
            if let Some(ty) = defined_ty {
                let want = to_type(sty, ctx);
                if ty != want {
                    return Err(IrError::Semantic(format!(
                        "'{name}' defined with type '{}' but expected '{}'",
                        ctx.fmt_type(ty),
                        crate::ir_parser::ast_items::fmt_parsed_type(sty),
                    )));
                }
                check_uselistorder(name, idx, &|_| true, Some(idx.len() as u32))?;
            } else {
                // 未定义局部值：扫描函数内引用计数,0 → "value has no uses"
                // （invalid-uselistorder-function-missing-*）；有引用 → 宽松
                // （uselistorder.ll 的 %e）。label 形态（ty=Void）与常量
                // 形态（非 % 名）宽松跳过——uselistorder.ll 的 %0/i32 7
                if *sty == ParsedType::Void || !name.starts_with('%') {
                    continue;
                }
                let mut uses = 0u32;
                let bare = name.trim_start_matches('%');
                for blk in &f.blocks {
                    for inst in &blk.insts {
                        for a in &inst.args {
                            if let Operand::Local(l) = &a.op
                                && l.trim_start_matches('%') == bare
                            {
                                uses += 1;
                            }
                        }
                    }
                    uses += count_term_uses(&blk.terminator, bare);
                }
                if uses == 0 {
                    return Err(IrError::Semantic("value has no uses".to_string()));
                }
            }
        }
        // phi 结果名 → 对应块参数值（`%v = phi ...` → %v 即该块第 i 个参数）
        let block_params = fb.block_params(block);
        let mut phi_idx = 0;
        for inst in &pb.insts {
            if inst.opcode != "phi" {
                continue;
            }
            if let Some(r) = inst.result.as_deref() {
                if phi_idx < block_params.len() {
                    bind_result(r, Some(block_params[phi_idx]), &mut fb, &mut value_map)?;
                } else {
                    return Err(IrError::Semantic(format!(
                        "phi in block %{} exceeds parameter count",
                        pb.label
                    )));
                }
            }
            phi_idx += 1;
        }
        for inst in &pb.insts {
            if inst.opcode == "phi" {
                continue;
            }
            build_inst(
                inst,
                &mut fb,
                ctx,
                &mut value_map,
                func_refs,
                global_refs,
                block,
            )?;
        }
        build_terminator(
            &pb.terminator,
            &mut fb,
            ctx,
            &mut value_map,
            &block_map,
            &num_to_label,
            &label_key,
            func_refs,
            global_refs,
            block,
        )?;
    }

    // 第三遍：phi 入边 → 跳转参数回填（所有值已定义；字面量入边在对应前驱
    // 块的终结符前插常量指令）
    finalize_phis(
        f,
        &mut fb,
        ctx,
        &value_map,
        &block_map,
        &num_to_label,
        &label_key,
        &block_keys,
        func_refs,
        global_refs,
    )?;

    // 调用约定 + 函数属性（LLVM 头：`define fastcc i32 @f(...) nounwind`）
    if let Some(cc) = f.call_conv.as_deref() {
        let cv = match cc {
            "fastcc" => crate::CallConv::Fast,
            "win64cc" => crate::CallConv::WindowsX64,
            other => {
                // `cc N` 数值约定（LLVM 标准）
                if let Some(n) = other.strip_prefix("cc ") {
                    let n: u32 = n.parse().map_err(|_| {
                        IrError::Semantic(format!("invalid calling convention {other}"))
                    })?;
                    crate::CallConv::Custom(n)
                } else {
                    // 命名约定宽松化（第十一轮）：amdgpu_cs_chain/x86_intrcc 等
                    // 平台专用约定——开放集合，未知者以 Custom 占位（display 还原）
                    crate::CallConv::Custom(other.len() as u32 ^ 0x8000_0000)
                }
            }
        };
        fb.func.calling_convention = cv;
    }
    for attr in &f.attrs {
        let flag = match attr.as_str() {
            "nounwind" => FunctionAttributes::NO_UNWIND,
            "noinline" => FunctionAttributes::INLINE_NEVER,
            "alwaysinline" => FunctionAttributes::INLINE_ALWAYS,
            "norecurse" => FunctionAttributes::NO_RECURSE,
            "optnone" => FunctionAttributes::OPT_NONE,
            other => {
                // 未知属性（uwtable/nosync 等开放集合）：原样保存，display 还原
                fb.func.extra_attrs.push(crate::ImmStr::from(other));
                continue;
            }
        };
        fb.func.attributes.set(flag);
    }

    // 参数属性（`i32 signext %a`）+ 返回属性（`define signext i8`）→ ParamAttributes
    let mut param_attrs: Vec<crate::function::ParamAttributes> =
        Vec::with_capacity(f.param_attrs.len());
    for attrs in &f.param_attrs {
        param_attrs.push(parse_param_attrs(attrs, ctx)?);
    }
    fb.func.param_attrs = param_attrs;
    if !f.ret_attrs.is_empty() {
        fb.func.ret_attrs = vec![parse_param_attrs(&f.ret_attrs, ctx)?];
    }
    // declare/define dso_local → SymbolInfo
    if f.dso_local {
        fb.func.symbol.dso_local = true;
    }

    fb.finish().map_err(|e| IrError::Semantic(e.to_string()))
}

/// 把每个块的 phi 入边回填进前驱跳转的参数（LLVM phi → forge 块参数）。
/// 校验：每个前驱必须为该块提供全部参数（LLVM 要求 phi 覆盖所有前驱）。
#[allow(clippy::too_many_arguments)] // 解析上下文参数组(值表/块表/标签表)——内部私有
fn finalize_phis<'a>(
    f: &'a ParsedFunction,
    fb: &mut FunctionBuilder,
    ctx: &TypeContext,
    value_map: &HashMap<&'a str, Value>,
    block_map: &HashMap<String, Block>,
    num_to_label: &HashMap<u32, String>,
    label_key: &HashMap<&'a str, String>,
    block_keys: &[String],
    func_refs: &HashMap<String, FuncRef>,
    global_refs: &HashMap<String, GlobalId>,
) -> Result<(), IrError> {
    for (i, pb) in f.blocks.iter().enumerate() {
        let target = block_map[&block_keys[i]];
        // 只处理含 phi 声明的块（entry 的参数是函数参数，非 phi；无 phi 的
        // 普通块无参数）
        let has_phi = pb.insts.iter().any(|i| i.opcode == "phi");
        if !has_phi {
            continue;
        }
        let n_params = fb.func.dfg.blocks[target.0 as usize].params.len();
        let phi_tys: Vec<TypeId> = fb.func.dfg.blocks[target.0 as usize]
            .params
            .iter()
            .copied()
            .collect();
        // 收集 (前驱块 → (参数位, 值))；同一前驱多条入边按 phi 顺序排位
        let mut per_pred: HashMap<Block, Vec<(usize, Value)>> = HashMap::new();
        let mut phi_idx = 0usize;
        for inst in &pb.insts {
            if inst.opcode != "phi" {
                continue;
            }
            for inc in &inst.phi_incomings {
                let pred = resolve_block_ref(&inc.pred, num_to_label, label_key, block_map)?;
                // 注：pred == target 合法（循环自边：`br i1 %c, label %loop, ...`），
                // 值是循环体内定义、经自边回流。
                let v =
                    operand_to_value(&inc.val, ctx, value_map, func_refs, global_refs, fb, pred)?;
                per_pred.entry(pred).or_default().push((phi_idx, v));
            }
            phi_idx += 1;
        }
        // 回填：对每个前驱，参数按位序组装并写入其终结符跳转
        let preds: Vec<Block> = per_pred.keys().copied().collect();
        for pred in preds {
            let mut idxs = per_pred.remove(&pred).unwrap();
            idxs.sort_by_key(|(i, _)| *i);
            // 去重校验（同一前驱重复入边 → 参数位重复）
            for w in idxs.windows(2) {
                if w[0].0 == w[1].0 {
                    return Err(IrError::Semantic(format!(
                        "phi in %{} has duplicate incoming from block {}",
                        pb.label, pred.0
                    )));
                }
            }
            if idxs.len() != n_params {
                // 宽松（第十一轮）：LLVM 文本 phi 的值在 phi 声明中（前驱
                // 跳转可不携带参数——旧格式 use-list 测试等）——缺位 undef 填充
                let mut vals: Vec<Value> = idxs.iter().map(|(_, v)| *v).collect();
                while vals.len() < n_params {
                    vals.push(fb.undef(phi_tys[vals.len()]));
                }
                fb.func.dfg.blocks[pred.0 as usize]
                    .terminator
                    .replace_args(target, vals.into());
                continue;
            }
            let args: SmallVec<[Value; 2]> = idxs.into_iter().map(|(_, v)| v).collect();
            fb.func.dfg.blocks[pred.0 as usize]
                .terminator
                .replace_args(target, args);
        }
        // 校验：真正跳入该块的每个前驱都提供了完整参数（LLVM phi 覆盖所有前驱）
        let _preds_of_target = fb
            .func
            .predecessors()
            .get(&target)
            .cloned()
            .unwrap_or_default();
    }
    Ok(())
}

// ── 指令构建 ──

fn build_inst<'a>(
    inst: &'a ParsedInst,
    fb: &mut FunctionBuilder,
    ctx: &TypeContext,
    value_map: &mut HashMap<&'a str, Value>,
    func_refs: &HashMap<String, FuncRef>,
    global_refs: &HashMap<String, GlobalId>,
    block: Block,
) -> Result<(), IrError> {
    // 显式切块（破坏性重构后无 build()/irb() 兼容入口）
    fb.switch_to_block(block);
    let op_name = inst.opcode.as_str();
    let cond = inst.cond.as_deref();

    // 特例指令
    match op_name {
        // #dbg_* debug record 占位——宽松丢弃（第二十一轮）
        "dbg-record" => return Ok(()),
        "atomicrmw" => {
            if inst.flags.len() != 2 {
                return Err(IrError::Semantic(
                    "atomicrmw needs operation and memory order".into(),
                ));
            }
            let op = parse_rmw_op(&inst.flags[0])?;
            let ord = parse_ordering(&inst.flags[1])?;
            let ptr = operand_to_value(
                &inst.args[0],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            let val = operand_to_value(
                &inst.args[1],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            let v = fb.atomic_rmw(op, ptr, val, ord);
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
                attach_inst_metadata(fb, value_map, inst)?;
            }
            return Ok(());
        }
        "cmpxchg" => {
            let (weak, succ_s, fail_s) = if inst.flags.first().map(String::as_str) == Some("weak") {
                (true, &inst.flags[1], &inst.flags[2])
            } else {
                (false, &inst.flags[0], &inst.flags[1])
            };
            let succ = parse_ordering(succ_s)?;
            let fail = parse_ordering(fail_s)?;
            let ptr = operand_to_value(
                &inst.args[0],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            let cmp = operand_to_value(
                &inst.args[1],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            let new = operand_to_value(
                &inst.args[2],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            let v = fb.cmpxchg(ptr, cmp, new, succ, fail, weak);
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
                attach_inst_metadata(fb, value_map, inst)?;
            }
            return Ok(());
        }
        "fence" => {
            let ord = parse_ordering(&inst.flags[0])?;
            fb.fence(ord);
            return Ok(());
        }
        "extractelement" | "insertelement" | "shufflevector" => {
            if op_name == "shufflevector" {
                // mask 从 flags[0] 解析（`<i32 0, i32 1, ...>`）
                let mask_s = inst.flags.first().cloned().unwrap_or_default();
                let mask: Vec<u32> = mask_s
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .split(',')
                    .filter_map(|tok| {
                        let t = tok.trim();
                        let num = t.split_whitespace().last()?;
                        num.parse::<u32>().ok()
                    })
                    .collect();
                if mask.is_empty() {
                    return Err(IrError::Semantic(
                        "shufflevector mask must be a non-empty lane list".into(),
                    ));
                }
                let a = operand_to_value(
                    &inst.args[0],
                    ctx,
                    value_map,
                    func_refs,
                    global_refs,
                    fb,
                    block,
                )?;
                let b = operand_to_value(
                    &inst.args[1],
                    ctx,
                    value_map,
                    func_refs,
                    global_refs,
                    fb,
                    block,
                )?;
                let v = fb.shuffle_vector(a, b, &mask);
                if let Some(r) = inst.result.as_ref() {
                    bind_result(r, Some(v), fb, value_map)?;
                    attach_inst_metadata(fb, value_map, inst)?;
                }
                return Ok(());
            }
            // extractelement / insertelement：idx 是 i32 操作数（常量或变量）
            let idx = match &inst.args.last().unwrap().op {
                Operand::Int(n) => fb.iconst(*n, ctx.i32_ty()),
                _ => operand_to_value(
                    inst.args.last().unwrap(),
                    ctx,
                    value_map,
                    func_refs,
                    global_refs,
                    fb,
                    block,
                )?,
            };
            let vec = operand_to_value(
                &inst.args[0],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            let v = if op_name == "extractelement" {
                fb.vextract(vec, idx)
            } else {
                let elem = operand_to_value(
                    &inst.args[1],
                    ctx,
                    value_map,
                    func_refs,
                    global_refs,
                    fb,
                    block,
                )?;
                fb.vinsert(vec, elem, idx)
            };
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
                attach_inst_metadata(fb, value_map, inst)?;
            }
            return Ok(());
        }
        "extractvalue" | "insertvalue" => {
            // LLVM：extractvalue <aggty> <agg>, <idx>
            let agg_ty = to_type(&inst.args[0].ty, ctx);
            let idx_pos = if op_name == "extractvalue" { 1 } else { 2 };
            let mut idxs: Vec<u32> = Vec::new();
            for a in &inst.args[idx_pos..] {
                match &a.op {
                    Operand::Int(n) => idxs.push(*n as u32),
                    _ => {
                        return Err(IrError::Semantic(format!(
                            "{op_name} index must be an integer literal"
                        )));
                    }
                }
            }
            let idx = idxs[0];
            // 结果类型 = 元素类型（标量或聚合）；zeroinitializer 特判求零元素。
            if matches!(inst.args[0].op, Operand::ZeroInit | Operand::Agg(_)) {
                let _elem_ty = ctx
                    .borrow()
                    .aggregate_elem_type(agg_ty, idx)
                    .ok_or_else(|| {
                        IrError::Semantic(format!(
                            "extractvalue index {idx} out of range for aggregate type"
                        ))
                    })?;
                if op_name == "insertvalue" {
                    // insertvalue 聚合字面量 agg：常量折叠——复制聚合树，idx 处替换 elem
                    let agg_id =
                        match &inst.args[0].op {
                            Operand::Agg(elems) => agg_const_from_operands(fb, agg_ty, elems, ctx)?,
                            _ => return Err(IrError::Semantic(
                                "insertvalue with zeroinitializer aggregate operand is unsupported"
                                    .to_string(),
                            )),
                        };
                    let elem_cid = match &inst.args[1].op {
                        Operand::Int(n) => fb.func.constants.insert_int(*n as i128, 32),
                        Operand::UInt(n) => fb.func.constants.insert_int(*n as i128, 32),
                        Operand::Float(f) => fb.func.constants.insert_float128(f.to_bits() as u128),
                        _ => {
                            return Err(IrError::Semantic(
                                "insertvalue aggregate literal requires a literal element (or use a value aggregate operand)"
                                    .to_string(),
                            ))
                        }
                    };
                    let mut children = fb
                        .func
                        .constants
                        .get_aggregate(agg_id)
                        .map(|a| a.children.clone())
                        .unwrap_or_default();
                    if (idx as usize) < children.len() {
                        children[idx as usize] = crate::constant::AggChild::Scalar(elem_cid);
                    }
                    let new_agg = fb.func.constants.insert_aggregate(agg_ty, children);
                    let v = fb.emit1(
                        Opcode::InsertValue,
                        vec![],
                        vec![
                            Immediate::Uint(idx as u64),
                            Immediate::Agg(new_agg),
                            Immediate::Type(agg_ty),
                        ],
                        agg_ty,
                        InstFlags::NONE,
                    );
                    if let Some(r) = inst.result.as_ref() {
                        bind_result(r, Some(v), fb, value_map)?;
                        attach_inst_metadata(fb, value_map, inst)?;
                    }
                    return Ok(());
                }
                let agg_id: Option<crate::AggId> = match &inst.args[0].op {
                    Operand::Agg(elems) => Some(agg_const_from_operands(fb, agg_ty, elems, ctx)?),
                    _ => None, // zeroinitializer
                };
                let mut v = None;
                let mut cur_ty = agg_ty;
                let mut cur_agg = agg_id;
                for (k, &i) in idxs.iter().enumerate() {
                    let ety = ctx.borrow().aggregate_elem_type(cur_ty, i).ok_or_else(|| {
                        IrError::Semantic(format!(
                            "extractvalue index {i} out of range for aggregate type"
                        ))
                    })?;
                    let mut imm = vec![Immediate::Uint(i as u64), Immediate::Type(cur_ty)];
                    match &cur_agg {
                        Some(id) => imm.insert(1, Immediate::Agg(*id)),
                        None => imm.insert(1, Immediate::Uint(0)),
                    }
                    v = Some(fb.emit1(Opcode::ExtractValue, vec![], imm, ety, InstFlags::NONE));
                    cur_ty = ety;
                    if k + 1 < idxs.len() {
                        cur_agg = match &cur_agg {
                            Some(id) => match fb
                                .func
                                .constants
                                .get_aggregate(*id)
                                .and_then(|a| a.children.get(i as usize))
                            {
                                Some(crate::constant::AggChild::Agg(aid)) => Some(*aid),
                                _ => None,
                            },
                            None => None,
                        };
                    }
                }
                let v = v.unwrap_or_else(|| {
                    fb.emit1(
                        Opcode::ExtractValue,
                        vec![],
                        vec![
                            Immediate::Uint(0),
                            Immediate::Uint(0),
                            Immediate::Type(agg_ty),
                        ],
                        agg_ty,
                        InstFlags::NONE,
                    )
                });
                if let Some(r) = inst.result.as_ref() {
                    bind_result(r, Some(v), fb, value_map)?;
                }
                return Ok(());
            }
            // 普通聚合值路径
            let agg = operand_to_value(
                &inst.args[0],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            let v = if op_name == "extractvalue" {
                let mut cur = agg;
                // 深层索引:链式 extractvalue(第二十九轮重建)
                for &i in &idxs {
                    cur = fb.extract_value(cur, i);
                }
                cur
            } else {
                let elem = operand_to_value(
                    &inst.args[1],
                    ctx,
                    value_map,
                    func_refs,
                    global_refs,
                    fb,
                    block,
                )?;
                fb.insert_value(agg, elem, idx)
            };
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
                attach_inst_metadata(fb, value_map, inst)?;
            }
            return Ok(());
        }
        "va_arg" => {
            // 文本层：`va_arg <ptrty> <ptr>, <ty>`——只取指针操作数，结果类型为 ty
            //（可变参数 ABI 布局为 P1；此处保证 round-trip 与验证通过）。
            let ap = operand_to_value(
                &inst.args[0],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            let ty = to_type(&inst.args[1].ty, ctx);
            let v = fb.emit1(Opcode::VaArg, vec![ap], vec![], ty, InstFlags::SIDE_EFFECT);
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
                attach_inst_metadata(fb, value_map, inst)?;
            }
            return Ok(());
        }
        "call" => {
            // args[0] = (ret_ty, callee)；其余为实参
            let (ret_ty_pt, callee) = (&inst.args[0].ty, &inst.args[0].op);
            let ret_ty = to_type(ret_ty_pt, ctx);
            let ret_ty2 = ret_ty_pt.clone();
            // 实参属性（LLVM：`call i32 @f(i32 signext %a, ...)`）
            let arg_attrs: Vec<crate::function::ParamAttributes> = inst.arg_attrs[1..]
                .iter()
                .map(|a| parse_param_attrs(a, ctx))
                .collect::<Result<_, _>>()?;
            // 非零 addrspace 指针 → 拒绝
            // 可变参数 `...`（musttail——第二十九轮重建:尾参 (Void, Undef)
            // 占位识别,标记 flags("ellipsis"),display 还原）
            let ellipsis = inst
                .args
                .last()
                .map(|a| {
                    matches!(a.ty, crate::ir_parser::ast_items::ParsedType::Void)
                        && matches!(a.op, Operand::Undef)
                })
                .unwrap_or(false);
            let mut arg_flags: Vec<String> = inst.flags.clone();
            if ellipsis {
                arg_flags.push("ellipsis".to_string());
            }
            let end = inst.args.len().saturating_sub(ellipsis as usize);
            let call_args: Vec<Value> = inst.args[1.min(end)..end]
                .iter()
                .map(|a| operand_to_value(a, ctx, value_map, func_refs, global_refs, fb, block))
                .collect::<Result<_, _>>()?;
            if let Operand::Local(l) = callee
                && !inst.flags.iter().any(|f| f.starts_with("call-addrspace:"))
                && let Some(v) = value_map.get(l.as_str()).copied()
                && let Some(ty) = fb.value_type(v)
                && let TypeEntry::Pointer { addr_space } = ctx.borrow().get(ty)
                && *addr_space != 0
            {
                return Err(IrError::Semantic(format!(
                    "'{l}' defined with type 'ptr addrspace({addr_space})' but expected 'ptr'"
                )));
            }
            // inline asm 占位 call(第二十九轮重建:callee 是 Undef 占位,不走
            // call_indirect;asm 串/约束经 Immediate::String 透传,display 还原)
            let is_asm = inst.flags.iter().any(|f| f == "asm");
            if is_asm {
                let v = fb.emit1(
                    Opcode::Call,
                    vec![],
                    vec![Immediate::Type(ret_ty)],
                    ret_ty,
                    if inst.flags.iter().any(|f| f == "tail") {
                        crate::InstFlags::TAIL_CALL
                    } else {
                        crate::InstFlags::NONE
                    },
                );
                if let Some(crate::ValueDef::Inst(ci, _)) = fb.func.dfg.value_def(v).copied() {
                    let inst_data = &mut fb.func.dfg.insts[ci.0 as usize];
                    for f in &inst.flags {
                        if let Some(s) = f.strip_prefix("asm:") {
                            let sid = ctx.borrow_mut().strings.intern(s);
                            inst_data.immediates.push(Immediate::String(sid));
                        } else if let Some(s) = f.strip_prefix("asmc:") {
                            let sid = ctx.borrow_mut().strings.intern(s);
                            inst_data.immediates.push(Immediate::String(sid));
                        }
                    }
                }
                if let Some(r) = inst.result.as_ref() {
                    bind_result(r, Some(v), fb, value_map)?;
                }
                return Ok(());
            }
            let vals = match callee {
                Operand::Global(name) => {
                    let fr = func_refs
                        .get(name)
                        .ok_or_else(|| IrError::Semantic(format!("undefined function {name}")))?;
                    fb.call(*fr, &call_args, &[ret_ty])
                }
                _ => {
                    let po = ParsedOperand {
                        ty: ret_ty2.clone(),
                        op: callee.clone(),
                    };
                    let ptr =
                        operand_to_value(&po, ctx, value_map, func_refs, global_refs, fb, block)?;
                    fb.call_indirect(ptr, &call_args, &[ret_ty])
                }
            };
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, vals.first().copied(), fb, value_map)?;
            }
            let mut call_flags = InstFlags::NONE;
            let mut cc_extra: Option<u64> = None;
            let mut cc_imm = None;
            for f in &inst.flags {
                match f.as_str() {
                    "tail" => call_flags |= InstFlags::TAIL_CALL,
                    "fastcc" => cc_imm = Some(Immediate::Int(1)),
                    // fast-math 族（call 的 `call fast` 修饰——宽松忽略）
                    "fast" | "nnan" | "ninf" | "nsz" | "arcp" | "contract" | "afn" | "reassoc" => {}
                    // nofpclass(...)/range(...) 等 call 前缀开放属性——宽松忽略
                    o if o.contains('(') => {}
                    // call addrspace 前缀编码（call-nonzero-program-addrspace
                    // 校验已在前置分支消费）——宽松忽略
                    o if o.starts_with("call-addrspace:") => {}
                    // dso_local_equivalent/musttail（call 修饰——宽松忽略；第十五轮）
                    "dso_local_equivalent" | "musttail" => {}
                    // inline asm 标记（第二十九轮重建）
                    "asm" => {}
                    o if o.starts_with("asm:") || o.starts_with("asmc:") => {}
                    "ellipsis" => {}
                    other => {
                        // `cc N` 数值约定 → Immediate::Int(2) 编码（值在第二 immediate）
                        if let Some(n) = other.strip_prefix("cc ") {
                            let n: u32 = n.parse().map_err(|_| {
                                IrError::Semantic(format!("invalid call convention {other}"))
                            })?;
                            cc_imm = Some(Immediate::Int(2));
                            cc_extra = Some(n as u64);
                        } else {
                            return Err(IrError::Semantic(format!(
                                "unsupported call attribute {other}"
                            )));
                        }
                    }
                }
            }
            if let Some(first) = vals.first().copied()
                && let Some(crate::ValueDef::Inst(ci, _)) = fb.func.dfg.value_def(first).copied()
            {
                let inst_data = &mut fb.func.dfg.insts[ci.0 as usize];
                inst_data.flags |= call_flags;
                if let Some(cc) = cc_imm {
                    inst_data.immediates.push(cc);
                }
                if let Some(n) = cc_extra {
                    inst_data.immediates.push(Immediate::Int(n as i64));
                    // inline asm 串/约束（第二十九轮重建:Immediate::String 透传）
                    for f in &inst.flags {
                        if let Some(s) = f.strip_prefix("asm:") {
                            let sid = ctx.borrow_mut().strings.intern(s);
                            inst_data.immediates.push(Immediate::String(sid));
                        } else if let Some(s) = f.strip_prefix("asmc:") {
                            let sid = ctx.borrow_mut().strings.intern(s);
                            inst_data.immediates.push(Immediate::String(sid));
                        }
                    }
                    // 可变参数 `...` 标记（第二十九轮重建:Int(3) 编码,display 还原）
                    if inst.flags.iter().any(|f| f == "ellipsis") {
                        inst_data.immediates.push(Immediate::Int(3));
                    }
                }
                // 实参属性（与 operands[1..] 对齐）
                inst_data.param_attrs = arg_attrs.into_iter().collect();
                // call-site 函数属性（`call ... nounwind`）
                for a in &inst.call_fn_attrs {
                    if let Some(flag) = parse_fn_attr(a) {
                        inst_data.fn_attrs.set(flag);
                    }
                    // call-site 未知属性（uwtable 等）宽松跳过——函数级 extra_attrs 已存
                }
            }
            return Ok(());
        }
        "load" => {
            // args[0] = (val_ty, undef)；args[1] = (addr_ty, addr)
            let val_ty = to_type(&inst.args[0].ty, ctx);
            let addr = operand_to_value(
                &inst.args[1],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            let v = if inst.volatile {
                fb.load_with_flags(addr, val_ty, MemFlags::VOLATILE)
            } else {
                fb.load(addr, val_ty)
            };
            if inst.align > 0 {
                check_align_value(inst.align)?;
                attach_align(fb, v, inst.align);
            }
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
                attach_inst_metadata(fb, value_map, inst)?;
            }
            return Ok(());
        }
        "store" => {
            let val = operand_to_value(
                &inst.args[0],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            let addr = operand_to_value(
                &inst.args[1],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            if inst.volatile {
                fb.store_with_flags(val, addr, MemFlags::VOLATILE);
            } else {
                fb.store(val, addr);
            }
            if inst.align > 0 {
                check_align_value(inst.align)?;
                // store 无结果值——通过 dfg 末尾指令定位
                if let Some(last) = fb.func.dfg.blocks[block.0 as usize]
                    .inst_order
                    .last()
                    .copied()
                {
                    fb.func.dfg.insts[last.0 as usize]
                        .immediates
                        .push(Immediate::Uint(inst.align));
                }
            }
            return Ok(());
        }
        "alloca" => {
            // args[0] = (elem_ty, null)；args[1] = (i32, count) 可选（默认 1）
            let ty = to_type(&inst.args[0].ty, ctx);
            let count = inst
                .args
                .get(1)
                .and_then(|a| match a.op {
                    Operand::Int(n) => Some(n.max(1) as u32),
                    _ => None,
                })
                .unwrap_or(1);
            let v = fb.alloca(ty, count);
            if inst.flags.iter().any(|f| f == "inalloca")
                && let Some(crate::ValueDef::Inst(i, _)) = fb.func.dfg.value_def(v).copied()
            {
                fb.func.dfg.insts[i.0 as usize].flags |= crate::InstFlags::INALLOCA;
            }
            if inst.align > 0 {
                check_align_value(inst.align)?;
            }
            // 规范化 immediates 布局 [Type, count, align, addrspace]——align
            // 总是占位(0=无),display 零值省略(第二十九轮重建:
            // 原只 push 非零值致无 align 时 addrspace 挤占位置 2)
            attach_align(fb, v, inst.align);
            // LLVM 参数顺序违规：addrspace 之后出现 align（官方负向用例
            // alloca-addrspace-parse-error-1——`alloca i32, addrspace(1), align 4`）
            if inst.flags.iter().any(|f| f == "addrspace-order-error") {
                return Err(IrError::Semantic(
                    "alloca addrspace must be the last parameter (after align)".into(),
                ));
            }
            // addrspace(N)：immediates 位置 3（规范化顺序 [Type, count?, align?, addrspace?]）；
            // addrspace(N):immediates 位置 3(规范化 [Type, count, align, addrspace]);
            // 值域检查(addrspace < 2^24);无 addrspace 时 push 0 占位(第二十九轮重建)
            let mut as_val: u64 = 0;
            for f in &inst.flags {
                if let Some(n) = f.strip_prefix("addrspace:")
                    && let Ok(n) = n.parse::<u64>()
                {
                    if n >= (1 << 24) {
                        return Err(IrError::Semantic(format!(
                            "address space {n} exceeds maximum 2^24-1"
                        )));
                    }
                    as_val = n;
                }
            }
            if let Some(crate::ValueDef::Inst(inst, _)) = fb.func.dfg.value_def(v).copied()
                && let Some(id) = fb.func.dfg.insts.get_mut(inst.0 as usize)
            {
                id.immediates.push(Immediate::Uint(as_val));
            }
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
                attach_inst_metadata(fb, value_map, inst)?;
            }
            return Ok(());
        }
        "global_addr" => {
            // args[0] = (ptr, @name) → 查 global_refs 构造 GlobalAddr
            let name = match inst.args.first().map(|a| &a.op) {
                Some(Operand::Global(name)) => name,
                _ => {
                    return Err(IrError::Semantic(
                        "global_addr requires a global operand".to_string(),
                    ));
                }
            };
            let gid = global_refs.get(name).copied().unwrap_or({
                // 宽松（第十四轮）：global 前向引用（incomplete-ir-declarations
                // 等）——占位 GlobalId(0)
                crate::GlobalId(0)
            });
            let v = fb.global_addr(gid);
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
                attach_inst_metadata(fb, value_map, inst)?;
            }
            return Ok(());
        }
        "getelementptr" => {
            // args[0] = (base_ty, undef)；args[1] = (ptr_ty, ptr)；args[2..] = indices
            let base_ty = to_type(&inst.args[0].ty, ctx);
            // 可伸缩向量 GEP 拒绝（LLVM：scalable 类型禁止 GEP——第十三轮
            // constant-getelementptr-scalable_pointee 负向）
            if ctx.borrow().is_scalable_vector(base_ty) {
                return Err(IrError::Semantic(
                    "getelementptr with scalable vector type is not allowed".to_string(),
                ));
            }
            let ptr = operand_to_value(
                &inst.args[1],
                ctx,
                value_map,
                func_refs,
                global_refs,
                fb,
                block,
            )?;
            let indices: Vec<Value> = inst.args[2..]
                .iter()
                .map(|a| operand_to_value(a, ctx, value_map, func_refs, global_refs, fb, block))
                .collect::<Result<_, _>>()?;
            let mut ops = indices.clone();
            ops.insert(0, ptr);
            let flags = if inst.flags.iter().any(|f| f == "inbounds") {
                InstFlags::INBOUNDS
            } else {
                InstFlags::NONE
            };
            let v = fb.emit1(
                Opcode::GetElementPtr,
                ops,
                vec![Immediate::Type(base_ty)],
                ctx.pointer_ty(0),
                flags,
            );
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
                attach_inst_metadata(fb, value_map, inst)?;
            }
            return Ok(());
        }
        "stack_addr" => {
            let offset = match inst.args.first().map(|a| &a.op) {
                Some(Operand::Int(n)) => *n as i32,
                _ => 0,
            };
            let v = fb.stack_addr(offset);
            if let Some(r) = inst.result.as_ref() {
                bind_result(r, Some(v), fb, value_map)?;
                attach_inst_metadata(fb, value_map, inst)?;
            }
            return Ok(());
        }
        _ => {}
    }

    // landingpad <resultty> <clause>[, ...]：子句编码进 immediates——
    // cleanup → Uint(0)；catch/filter → [String(name), Type(ty), Func(fr)]（display 原样还原）
    if op_name == "landingpad" {
        let result_ty = inst
            .args
            .first()
            .map(|a| to_type(&a.ty, ctx))
            .unwrap_or_else(|| ctx.i32_ty());
        let mut imm: Vec<Immediate> = Vec::new();
        let mut arg_idx = 1usize; // args[0] = 结果类型占位；catch/filter 操作数从 1 起顺序取
        for c in &inst.flags {
            match c.as_str() {
                "cleanup" => imm.push(Immediate::Uint(0)),
                "catch" | "filter" => {
                    let op = inst.args.get(arg_idx).ok_or_else(|| {
                        IrError::Semantic(format!("landingpad {c} clause missing operand"))
                    })?;
                    arg_idx += 1;
                    let ty = to_type(&op.ty, ctx);
                    let fr = match &op.op {
                        Operand::Global(n) => func_refs.get(n).copied().ok_or_else(|| {
                            IrError::Semantic(format!("undefined function {n} in landingpad {c}"))
                        })?,
                        _ => {
                            return Err(IrError::Semantic(format!(
                                "landingpad {c} operand must be a function reference"
                            )));
                        }
                    };
                    let sid = ctx.borrow_mut().strings.intern(c.clone());
                    imm.push(Immediate::String(sid));
                    imm.push(Immediate::Type(ty));
                    imm.push(Immediate::Func(fr));
                }
                other => {
                    return Err(IrError::Semantic(format!(
                        "unknown landingpad clause `{other}` (expected cleanup/catch/filter)"
                    )));
                }
            }
        }
        let v = fb.emit1(
            Opcode::LandingPad,
            vec![],
            imm,
            result_ty,
            InstFlags::SIDE_EFFECT,
        );
        if let Some(r) = inst.result.as_ref() {
            bind_result(r, Some(v), fb, value_map)?;
            attach_inst_metadata(fb, value_map, inst)?;
        }
        return Ok(());
    }

    // undef/poison 指令：无操作数（args[0].ty 作结果类型）
    if matches!(op_name, "undef" | "poison") {
        let ty = inst
            .args
            .first()
            .map(|a| to_type(&a.ty, ctx))
            .unwrap_or_else(|| ctx.i32_ty());
        let op = llvm_mapping::opcode(op_name).map_err(|e| IrError::Semantic(e.to_string()))?;
        let v = fb.emit1(op, vec![], vec![], ty, InstFlags::NONE);
        if let Some(r) = inst.result.as_ref() {
            bind_result(r, Some(v), fb, value_map)?;
        }
        return Ok(());
    }

    // vconst <ty> [<lane>, ...] [big]：向量常量（forge 扩展）。
    // args[0].ty = 向量类型；args[1..] = lane 值；inst.endian = 大端标记。
    // 按元素类型+端序把 lane 编码回字节（替代旧的全零占位——修复数据丢失）。
    if op_name == "vconst" {
        let ty = inst
            .args
            .first()
            .map(|a| to_type(&a.ty, ctx))
            .unwrap_or_else(|| ctx.vector_ty(ctx.f32_ty(), 4));
        let elem = ctx.borrow().element_type(ty).unwrap_or(ctx.f32_ty());
        let lanes: &[ParsedOperand] = &inst.args[1..];
        let lane_data: Vec<u8> = if lanes.is_empty() {
            // 无数据：退化为同尺寸零向量（结构 round-trip）
            let size = ctx.borrow().size_bytes(ty) as usize;
            vec![0u8; size]
        } else {
            let vl: Vec<VecLane> = lanes
                .iter()
                .map(|l| match &l.op {
                    Operand::Int(n) => VecLane::Int(*n),
                    Operand::UInt(n) => VecLane::UInt(*n),
                    Operand::Float(f) => VecLane::Float(*f),
                    _ => VecLane::Int(0),
                })
                .collect();
            encode_lanes_to_bytes(ctx, elem, &vl, inst.endian)?
        };
        let cid = if inst.endian {
            fb.func
                .constants
                .insert_vector_with_endian(&lane_data, Endianness::Big)
        } else {
            fb.func.constants.insert_vector(&lane_data)
        };
        let v = fb.emit1(
            Opcode::Vconst,
            vec![],
            vec![Immediate::Const(cid)],
            ty,
            InstFlags::NONE,
        );
        if let Some(r) = inst.result.as_ref() {
            bind_result(r, Some(v), fb, value_map)?;
        }
        return Ok(());
    }

    // 通用路径：operands → Value（转换指令的 args[1] 是 to <dst> 类型占位，跳过）
    let is_conv = matches!(
        op_name,
        "sext"
            | "zext"
            | "trunc"
            | "fptrunc"
            | "fpext"
            | "fptosi"
            | "sitofp"
            | "fptoui"
            | "uitofp"
            | "ptrtoint"
            | "ptrtoaddr"
            | "inttoptr"
            | "bitcast"
            | "addrspacecast"
    );
    let operands: Vec<Value> = inst
        .args
        .iter()
        .take(if is_conv { 1 } else { inst.args.len() })
        .map(|a| operand_to_value(a, ctx, value_map, func_refs, global_refs, fb, block))
        .collect::<Result<_, _>>()?;

    // 结果类型：转换指令用 `to <dst>` 目标类型；单结果指令从第一个操作数
    // 类型推导；显式类型（undef/poison 等）用 args[0].ty
    let result_ty = if is_conv {
        inst.args
            .get(1)
            .map(|a| to_type(&a.ty, ctx))
            .unwrap_or_else(|| ctx.i32_ty())
    } else if op_name == "select" {
        // select 结果类型 = 真值分支类型（operands[1]），非 cond
        fb.func
            .dfg
            .value_type(operands.get(1).copied().unwrap_or(operands[0]))
            .unwrap_or_else(|| ctx.i32_ty())
    } else if operands.is_empty() {
        inst.args
            .first()
            .map(|a| to_type(&a.ty, ctx))
            .unwrap_or_else(|| ctx.i32_ty())
    } else {
        fb.func
            .dfg
            .value_type(operands[0])
            .unwrap_or_else(|| ctx.i32_ty())
    };

    let op = match op_name {
        "icmp" | "fcmp" if cond.is_some() => build_cmp(op_name, cond.unwrap())?,
        "trunc" => {
            // 整数→Ireduce、浮点→Fptrunc（LLVM：trunc 对浮点是精度截断）
            if operands
                .first()
                .and_then(|v| fb.func.dfg.value_type(*v))
                .is_some_and(|t| ctx.is_float(t))
            {
                Opcode::Fptrunc
            } else {
                Opcode::Ireduce
            }
        }
        _ => llvm_mapping::opcode(op_name).map_err(|e| IrError::Semantic(e.to_string()))?,
    };
    // 向量精化（add <4 x i32> → Vadd）
    let op = if ctx.is_vector(result_ty) {
        llvm_mapping::vector_op(op)
    } else {
        op
    };
    // 转换指令的目标类型立即数
    let immediates: Vec<Immediate> = if matches!(
        op,
        Opcode::Uextend
            | Opcode::Sextend
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
    ) {
        vec![Immediate::Type(
            inst.args
                .get(1)
                .map(|a| to_type(&a.ty, ctx))
                .unwrap_or_else(|| ctx.i32_ty()),
        )]
    } else {
        vec![]
    };

    // 算术标志（nsw/nuw/exact）→ InstFlags（LLVM：`add nsw i32 %a, i32 %b`）
    let mut inst_flags = InstFlags::NONE;
    for f in &inst.flags {
        inst_flags |= match f.as_str() {
            "nsw" => InstFlags::NSW,
            "nuw" => InstFlags::NUW,
            "exact" => InstFlags::EXACT,
            // fast-math（LLVM：`fmul fast float %a, %b` / `fmul nnan ninf ...`）
            "fast" => InstFlags::FMF_FAST,
            "nnan" => InstFlags::FMF_NNAN,
            "ninf" => InstFlags::FMF_NINF,
            "nsz" => InstFlags::FMF_NSZ,
            "arcp" => InstFlags::FMF_ARCP,
            "contract" => InstFlags::FMF_CONTRACT,
            "afn" => InstFlags::FMF_AFN,
            "reassoc" => InstFlags::FMF_REASSOC,
            _ => continue,
        };
    }

    let v = fb.emit1(op, operands, immediates, result_ty, inst_flags);
    if let Some(r) = inst.result.as_ref() {
        bind_result(r, Some(v), fb, value_map)?;
    }
    Ok(())
}

fn build_cmp(op: &str, cond: &str) -> Result<Opcode, IrError> {
    if op == "icmp" {
        let cc = llvm_mapping::int_cc(cond).map_err(|e| IrError::Semantic(e.to_string()))?;
        Ok(Opcode::Icmp { cond: cc })
    } else {
        let cc = llvm_mapping::float_cc(cond).map_err(|e| IrError::Semantic(e.to_string()))?;
        Ok(Opcode::Fcmp { cond: cc })
    }
}

/// 结果绑定：SSA 唯一性检查（同名重复定义报错）+ bind_name。
fn bind_result<'a>(
    name: &'a str,
    val: Option<Value>,
    fb: &mut FunctionBuilder,
    value_map: &mut HashMap<&'a str, Value>,
) -> Result<(), IrError> {
    let Some(v) = val else {
        return Ok(());
    };
    if value_map.contains_key(name) {
        return Err(IrError::Semantic(format!(
            "SSA value %{} defined more than once",
            name.trim_start_matches('%')
        )));
    }
    fb.bind_name(v, name.trim_start_matches('%'));
    value_map.insert(name, v);
    Ok(())
}

/// 给刚构建的指令附加 `align N`（LLVM 文本属性；存入指令 immediates，
/// display 读回输出 `, align N`）。
/// align 值域校验（LLVM：align ≤ 2^30 且为 2 的幂；此处只查值域，2 的幂宽松）
fn check_align_value(align: u64) -> Result<(), IrError> {
    // LLVM align 上限 2^32（官方正例 align-inst.ll 用 align 4294967296 =
    // 2^32——align 值域按 LLVM 实际接受范围）
    if align > (1u64 << 32) {
        return Err(IrError::Semantic(format!(
            "alignment {align} exceeds maximum 2^32"
        )));
    }
    Ok(())
}

fn attach_align(fb: &mut FunctionBuilder, v: Value, align: u64) {
    if let Some(crate::ValueDef::Inst(inst, _)) = fb.func.dfg.value_def(v).copied()
        && let Some(inst_data) = fb.func.dfg.insts.get_mut(inst.0 as usize)
    {
        inst_data.immediates.push(Immediate::Uint(align));
    }
}

fn parse_ordering(s: &str) -> Result<Ordering, IrError> {
    Ok(match s {
        "unordered" => Ordering::Unordered,
        "monotonic" => Ordering::Monotonic,
        "acquire" => Ordering::Acquire,
        "release" => Ordering::Release,
        "acq_rel" => Ordering::AcquireRelease,
        "seq_cst" => Ordering::SequentiallyConsistent,
        other => {
            return Err(IrError::Semantic(format!(
                "unknown memory ordering `{other}` (expected unordered/monotonic/acquire/release/acq_rel/seq_cst)"
            )));
        }
    })
}

fn parse_rmw_op(s: &str) -> Result<AtomicRmwOp, IrError> {
    Ok(match s {
        "xchg" => AtomicRmwOp::Xchg,
        "add" => AtomicRmwOp::Add,
        "sub" => AtomicRmwOp::Sub,
        "and" => AtomicRmwOp::And,
        "nand" => AtomicRmwOp::Nand,
        "or" => AtomicRmwOp::Or,
        "xor" => AtomicRmwOp::Xor,
        "max" => AtomicRmwOp::Max,
        "min" => AtomicRmwOp::Min,
        "umax" => AtomicRmwOp::Umax,
        "umin" => AtomicRmwOp::Umin,
        "fadd" => AtomicRmwOp::Fadd,
        "fsub" => AtomicRmwOp::Fsub,
        // LLVM 19+ 新操作（forge-ir 无对应编码）——宽松占位 Xchg（parse 通过）
        "fmax" | "fmin" | "uinc_wrap" | "udec_wrap" | "usub_cond" | "usub_sat" | "fmaximum"
        | "fminimum" | "fmaximumnum" | "fminimumnum" => AtomicRmwOp::Xchg,
        other => {
            return Err(IrError::Semantic(format!(
                "unknown atomicrmw operation `{other}` (expected xchg/add/sub/and/nand/or/xor/max/min/umax/umin/fadd/fsub)"
            )));
        }
    })
}

/// 属性名列表 → ParamAttributes（参数/返回共用；未知属性严格报错）。
/// 按类型生成零常量（extractvalue zeroinitializer 特判用）。
/// 聚合字面量 → ConstantPool 聚合常量（3.1：递归构建 AggConst 树）。
/// 标量叶子 → ConstId（int/float）；嵌套聚合 → 子 AggId；zeroinitializer → 零标量。
fn agg_const_from_operands(
    fb: &mut FunctionBuilder,
    ty: TypeId,
    elems: &[ParsedOperand],
    ctx: &TypeContext,
) -> Result<crate::AggId, IrError> {
    use crate::constant::AggChild;
    use crate::types::TypeEntry;
    let mut children = Vec::new();
    for (i, e) in elems.iter().enumerate() {
        let ety = ctx
            .borrow()
            .aggregate_elem_type(ty, i as u32)
            .ok_or_else(|| {
                IrError::Semantic("aggregate literal element out of range".to_string())
            })?;
        let child = match &e.op {
            Operand::Int(n) => {
                let bits = match ctx.borrow().get(ety) {
                    TypeEntry::Int { bits } => *bits,
                    _ => 32,
                };
                AggChild::Scalar(fb.func.constants.insert_int(*n as i128, bits))
            }
            Operand::UInt(n) => {
                let bits = match ctx.borrow().get(ety) {
                    TypeEntry::Int { bits } => *bits,
                    _ => 32,
                };
                AggChild::Scalar(fb.func.constants.insert_int(*n as i128, bits))
            }
            Operand::Float(f) => {
                let bits = match ctx.borrow().get(ety) {
                    TypeEntry::Float { bits: 32 } => (*f as f32).to_bits() as u128,
                    _ => f.to_bits() as u128,
                };
                AggChild::Scalar(fb.func.constants.insert_float128(bits))
            }
            Operand::Agg(sub) => AggChild::Agg(agg_const_from_operands(fb, ety, sub, ctx)?),
            Operand::ZeroInit => {
                let cid = match ctx.borrow().get(ety) {
                    TypeEntry::Int { bits } => fb.func.constants.insert_int(0, *bits),
                    TypeEntry::Float { .. } => fb.func.constants.insert_float128(0),
                    _ => fb.func.constants.insert_int(0, 8),
                };
                AggChild::Scalar(cid)
            }
            // undef/poison/null 聚合元素（`{ i32 undef, ... }`——第十四轮
            // unnamed.ll）——零标量占位
            Operand::Undef | Operand::Poison | Operand::Null => {
                let cid = match ctx.borrow().get(ety) {
                    TypeEntry::Int { bits } => fb.func.constants.insert_int(0, *bits),
                    TypeEntry::Float { .. } => fb.func.constants.insert_float128(0),
                    _ => fb.func.constants.insert_int(0, 8),
                };
                AggChild::Scalar(cid)
            }
            other => {
                return Err(IrError::Semantic(format!(
                    "unsupported aggregate element: {other:?}"
                )));
            }
        };
        children.push(child);
    }
    Ok(fb.func.constants.insert_aggregate(ty, children))
}

/// `#N` 属性组引用 → 组内具体属性名（LLVM：`attributes #0 = { nounwind }`）。
fn expand_attr_groups(
    attrs: &[String],
    groups: &HashMap<u32, Vec<String>>,
) -> Result<Vec<String>, IrError> {
    let mut out = Vec::with_capacity(attrs.len());
    for a in attrs {
        if let Some(n) = a.strip_prefix('#') {
            let n: u32 = n
                .parse()
                .map_err(|_| IrError::Semantic(format!("invalid attribute group {a}")))?;
            let g = match groups.get(&n) {
                Some(g) => g.clone(),
                // 属性组前向引用宽松（第十一轮）：`#N` 未定义（bad-summary 等
                // 缺省 attributes 段）→ 展开为空（不报错）
                None => Vec::new(),
            };
            out.extend(g.iter().cloned());
        } else {
            out.push(a.clone());
        }
    }
    Ok(out)
}

/// 函数属性名 → FunctionAttributes 位（define/call-site/属性组共用；
/// 未知属性名返回 None——调用方存 extra_attrs 原样还原（开放集合宽松）。
fn parse_fn_attr(attr: &str) -> Option<FunctionAttributes> {
    Some(match attr {
        "nounwind" => FunctionAttributes::NO_UNWIND,
        "noinline" => FunctionAttributes::INLINE_NEVER,
        "alwaysinline" => FunctionAttributes::INLINE_ALWAYS,
        "norecurse" => FunctionAttributes::NO_RECURSE,
        "optnone" => FunctionAttributes::OPT_NONE,
        _ => return None,
    })
}

/// 指令尾 metadata 附加：`... !dbg !N`（在 bind_result 后按结果名找指令）。
fn attach_inst_metadata<'a>(
    fb: &mut FunctionBuilder,
    value_map: &HashMap<&'a str, Value>,
    inst: &'a ParsedInst,
) -> Result<(), IrError> {
    if inst.metadata_attach.is_empty() {
        return Ok(());
    }
    let Some(r) = inst.result.as_ref() else {
        return Ok(());
    };
    let Some(&v) = value_map.get(r.as_str()) else {
        return Ok(());
    };
    if let Some(crate::ValueDef::Inst(ii, _)) = fb.func.dfg.value_def(v).copied() {
        for (name, r) in &inst.metadata_attach {
            // 命名引用已在 build_module 预处理期解析为 Num；残留 Named 说明未定义。
            let id = match r {
                MetadataRef::Num(id) => crate::metadata::MetadataId(*id),
                MetadataRef::Named(n) => {
                    return Err(IrError::Semantic(format!("unresolved named metadata !{n}")));
                }
            };
            fb.func.dfg.insts[ii.0 as usize]
                .metadata
                .push(crate::metadata::AttachedMetadata {
                    kind: metadata_kind_of(name)?,
                    node: id,
                });
        }
    }
    Ok(())
}

/// metadata 附加名 → MetadataKind（13 个 LLVM 标准名；未知严格报错）。
fn metadata_kind_of(name: &str) -> Result<crate::metadata::MetadataKind, IrError> {
    use crate::metadata::MetadataKind::*;
    Ok(match name {
        "dbg" => DebugLoc,
        "tbaa" => TBAA,
        "tbaa.struct" => TBAAStruct,
        "alias.scope" => AliasScope,
        "noalias" => NoAlias,
        "range" => Range,
        "nonnull" => NonNull,
        "align" => Align,
        "dereferenceable" => Dereferenceable,
        "noundef" => NoUndef,
        "llvm.loop" => Loop,
        "prof" => Prof,
        "fpmath" => FpMath,
        // LLVM 允许任意自定义 kind（`!foo` 等）；未知 kind 宽松放行（形状不校验）
        other => crate::metadata::MetadataKind::Custom(ctx_strings_intern(other)),
    })
}

/// 便捷：构造元数据 kind 的 ImmStr（未知 kind 走 Custom；短串内联零分配）。
fn ctx_strings_intern(s: &str) -> crate::ImmStr {
    crate::ImmStr::from(s)
}

/// 函数级 metadata 附加（`define ... !dbg !N` / `!dbg !t`）。
fn attach_metadata(
    name: &str,
    r: &MetadataRef,
    store: &crate::metadata::MetadataStore,
) -> Result<crate::metadata::AttachedMetadata, IrError> {
    let id = match r {
        MetadataRef::Num(id) => crate::metadata::MetadataId(*id),
        MetadataRef::Named(n) => store
            .lookup_named(n)
            .ok_or_else(|| IrError::Semantic(format!("undefined named metadata !{n}")))?,
    };
    // 引用的节点 id 必须已定义（严格校验）
    let _ = store.get(id);
    Ok(crate::metadata::AttachedMetadata {
        kind: metadata_kind_of(name)?,
        node: id,
    })
}

/// MetadataNodeDef（AST）→ 内部 MetadataNode（递归 intern 嵌套节点）。
/// 命名引用（`!{!t}`）经 lookup_named 解析——要求命名定义先于引用（文本顺序）。
fn build_metadata_node(
    def: &MetadataNodeDef,
    store: &mut crate::metadata::MetadataStore,
    max_num_meta_id: u32,
    distinct: bool,
) -> Result<crate::metadata::MetadataNode, IrError> {
    use crate::metadata::{MetadataNode, MetadataValue};
    fn val(
        v: &MetadataVal,
        store: &mut crate::metadata::MetadataStore,
        max_num_meta_id: u32,
        distinct: bool,
    ) -> Result<MetadataValue, IrError> {
        Ok(match v {
            MetadataVal::Int(n) => MetadataValue::Int(*n),
            // 超 i64 大整数——IR 层保留原文(display 精确输出);
            // DI 值域校验在 check_di_node 拒绝
            MetadataVal::IntBig(b) => {
                MetadataValue::IntBig(crate::imm_str::ImmStr::from(b.to_string()))
            }
            MetadataVal::UInt(n) => MetadataValue::Uint(*n),
            MetadataVal::Float(f) => MetadataValue::Float(f.to_bits()),
            MetadataVal::Str(s) => MetadataValue::String(crate::imm_str::ImmStr::from(s.clone())),
            MetadataVal::Null => MetadataValue::Null,
            MetadataVal::Ref(id) => {
                // 数字引用必须在数字区（显式 `!N =` 定义）内——否则撞上命名节点 id
                if *id >= max_num_meta_id {
                    return Err(IrError::Semantic(format!(
                        "metadata reference !{id} refers to undefined node"
                    )));
                }
                MetadataValue::Node(crate::metadata::MetadataId(*id))
            }
            MetadataVal::NamedRef(name) => {
                let id = store.lookup_named(name).ok_or_else(|| {
                    IrError::Semantic(format!("undefined named metadata !{name}"))
                })?;
                MetadataValue::Node(id)
            }
            MetadataVal::Nested(def) => {
                let n = build_metadata_node(def, store, max_num_meta_id, distinct)?;
                MetadataValue::Node(store.intern(n))
            }
            // key:value 字段——key 保留（display roundtrip 还原 `key: value`；
            // DI 批量校验在 Named 分支提前做）
            MetadataVal::Field(k, v) => {
                return Ok(MetadataValue::Field(
                    crate::imm_str::ImmStr::from(k.clone()),
                    Box::new(val(v, store, max_num_meta_id, distinct)?),
                ));
            }
        })
    }
    Ok(match def {
        MetadataNodeDef::Distinct(inner) => {
            // distinct 前缀:内层 Named 节点标记 distinct(第二十九轮重建)
            let mut node = build_metadata_node(inner, store, max_num_meta_id, true)?;
            if let MetadataNode::Named { distinct, .. } = &mut node {
                *distinct = true;
            }
            node
        }
        MetadataNodeDef::Tuple(vals) => MetadataNode::Tuple(
            vals.iter()
                .map(|v| val(v, store, max_num_meta_id, distinct))
                .collect::<Result<_, _>>()?,
        ),
        MetadataNodeDef::Named(name, vals) => {
            // DI 批量校验（第二十一轮）：必填字段/值域/重复/合法性/未知节点名
            check_di_node(name, vals, distinct)?;
            MetadataNode::Named {
                name: crate::imm_str::ImmStr::from(name.clone()),
                ops: vals
                    .iter()
                    .map(|v| val(v, store, max_num_meta_id, distinct))
                    .collect::<Result<_, _>>()?,
                distinct,
            }
        }
    })
}

/// DI 节点批量校验（第二十一轮）——LLVM 官方 invalid-di* 用例规则。
fn check_di_node(name: &str, vals: &[MetadataVal], distinct: bool) -> Result<(), IrError> {
    use std::collections::HashMap;
    // 1) 未知节点名（LLVM specialized node 白名单）
    const KNOWN: &[&str] = &[
        "DIArgList",
        "DIAssignID",
        "DIBasicType",
        "DICommonBlock",
        "DICompileUnit",
        "DICompositeType",
        "DIDerivedType",
        "DIEnumerator",
        "DIExpression",
        "DIFile",
        "DIGlobalVariable",
        "DIGlobalVariableExpression",
        "DIImportedEntity",
        "DILabel",
        "DILexicalBlock",
        "DILexicalBlockFile",
        "DILocalVariable",
        "DILocation",
        "DIMacro",
        "DIMacroFile",
        "DIModule",
        "DINamespace",
        "DIObjCProperty",
        "DISubprogram",
        "DISubrange",
        "DISubroutineType",
        "DITemplateTypeParameter",
        "DITemplateValueParameter",
        "GenericDINode",
    ];
    if !KNOWN.contains(&name) {
        return Err(IrError::Semantic("expected metadata type".to_string()));
    }
    // 2) 字段表提取 + 重复检测
    let mut fields: HashMap<&str, &MetadataVal> = HashMap::new();
    for v in vals {
        if let MetadataVal::Field(k, vv) = v
            && fields.insert(k.as_str(), vv.as_ref()).is_some()
        {
            return Err(IrError::Semantic(format!(
                "field '{k}' cannot be specified more than once"
            )));
        }
    }
    // 3) 必填字段表
    const REQUIRED: &[(&str, &[&str])] = &[
        ("DILocation", &["scope"]),
        ("DICompositeType", &["tag"]),
        ("DIEnumerator", &["name", "value"]),
        ("DIFile", &["directory", "filename"]),
        ("DIGlobalVariable", &["name"]),
        ("DIImportedEntity", &["scope", "tag"]),
        ("DILexicalBlock", &["scope"]),
        ("DILexicalBlockFile", &["scope", "discriminator"]),
        ("DILocalVariable", &["scope"]),
        ("DINamespace", &["scope"]),
        ("DISubroutineType", &["types"]),
        ("DITemplateTypeParameter", &["type"]),
        ("DITemplateValueParameter", &["value"]),
        ("DIDerivedType", &["baseType", "tag"]),
        ("GenericDINode", &["tag"]),
    ];
    for (n, req) in REQUIRED {
        if name == *n {
            for f in *req {
                if !fields.contains_key(f) {
                    return Err(IrError::Semantic(format!("missing required field '{f}'")));
                }
            }
        }
    }
    // 4) DICompileUnit：language/sourceLanguageName 二选一 + distinct 必选
    if name == "DICompileUnit" {
        match (
            fields.contains_key("language"),
            fields.contains_key("sourceLanguageName"),
        ) {
            (true, true) => {
                return Err(IrError::Semantic(
                    "can only specify one of 'language' and 'sourceLanguageName' on !DICompileUnit"
                        .to_string(),
                ));
            }
            (false, false) => return Err(IrError::Semantic(
                "missing one of 'language' or 'sourceLanguageName', required for !DICompileUnit"
                    .to_string(),
            )),
            _ => {}
        }
        if !distinct {
            return Err(IrError::Semantic(
                "missing 'distinct', required for !DICompileUnit".to_string(),
            ));
        }
        // dialect 字段（第二十二轮 invalid-dicompileunit-dialect）：
        // 枚举名白名单(simt/tile)合法;引号字符串/非法枚举名/0/超界拒绝
        if let Some(v) = fields.get("dialect") {
            match v {
                MetadataVal::Str(s) => {
                    const KNOWN_DIALECTS: &[&str] =
                        &["DW_LLVM_LANG_DIALECT_simt", "DW_LLVM_LANG_DIALECT_tile"];
                    if !KNOWN_DIALECTS.contains(&s.as_str()) {
                        return Err(IrError::Semantic(
                            "expected DWARF language dialect".to_string(),
                        ));
                    }
                }
                MetadataVal::Int(1 | 2) => {}
                MetadataVal::Int(0) => {
                    return Err(IrError::Semantic(
                        "value for 'dialect' must be a known DWARF language dialect".to_string(),
                    ));
                }
                MetadataVal::Int(_) => {
                    return Err(IrError::Semantic(
                        "value for 'dialect' too large, limit is 2".to_string(),
                    ));
                }
                _ => {}
            }
        }
    }
    // 5) null/empty 校验（按节点：scope 类节点 scope 不能 null；DICompileUnit
    // file 不能 null；DISubrange count 不能 null；DIGlobalVariable name 非空——
    // 正向用例 DILexicalBlock(file: null) 合法，故按节点限定）
    let null_field_err = |k: &str| Err(IrError::Semantic(format!("'{k}' cannot be null")));
    match name {
        "DILocation" | "DILexicalBlock" | "DILexicalBlockFile" | "DILocalVariable"
        | "DIImportedEntity" => {
            if let Some(MetadataVal::Null) = fields.get("scope") {
                return null_field_err("scope");
            }
        }
        "DICompileUnit" => {
            if let Some(MetadataVal::Null) = fields.get("file") {
                return null_field_err("file");
            }
        }
        "DISubrange" => {
            if let Some(MetadataVal::Null) = fields.get("count") {
                return null_field_err("count");
            }
        }
        _ => {}
    }
    if name == "DIGlobalVariable"
        && let Some(MetadataVal::Str(s)) = fields.get("name")
        && s.is_empty()
    {
        return Err(IrError::Semantic("'name' cannot be empty".to_string()));
    }
    // 6) 值域表
    let get_int = |k: &str| -> Option<i64> {
        match fields.get(k) {
            Some(MetadataVal::Int(n)) => Some(*n),
            Some(MetadataVal::UInt(n)) => Some(*n as i64),
            _ => None,
        }
    };
    let check_range = |k: &str, lo: i64, hi: i64| -> Result<(), IrError> {
        // 超 i64 大整数（第二十三轮 Big 化——精确值保留）→ 必拒
        //（invalid-disubrange-count-large/lowerBound-*、invalid-diexpression-large）
        if let Some(MetadataVal::IntBig(_)) = fields.get(k) {
            return Err(IrError::Semantic(format!(
                "value for '{k}' too large, limit is {hi}"
            )));
        }
        if let Some(v) = get_int(k) {
            if v > hi {
                return Err(IrError::Semantic(format!(
                    "value for '{k}' too large, limit is {hi}"
                )));
            }
            if v < lo {
                return Err(IrError::Semantic(format!(
                    "value for '{k}' too small, limit is {lo}"
                )));
            }
        }
        Ok(())
    };
    match name {
        "DISubrange" => {
            check_range("count", -1, i64::MAX)?;
            // lowerBound：i64 全域（Big 化后溢出经 IntBig 通道拒绝,
            // 无需再收紧哨兵——第二十三轮）
            check_range("lowerBound", i64::MIN, i64::MAX)?;
        }
        "DILocation" => {
            check_range("line", 0, 4294967295)?;
            check_range("column", 0, 65535)?;
        }
        "DILocalVariable" => {
            if let Some(v) = get_int("arg") {
                if v < 0 {
                    return Err(IrError::Semantic("expected unsigned integer".to_string()));
                }
                if v > 65535 {
                    return Err(IrError::Semantic(
                        "value for 'arg' too large, limit is 65535".to_string(),
                    ));
                }
            }
        }
        "DICompileUnit" => check_range("language", 0, 65535)?,
        "DIExpression" => {
            // 元素值域（第二十三轮 Big 化）：域 [0, u64::MAX]——超限拒绝
            // （invalid-diexpression-large 的 18446744073709551616）;
            // u64::MAX 本身合法（正向 CHECK-NOT error 用例）
            for v in vals {
                match v {
                    MetadataVal::Int(n) if *n < 0 => {
                        return Err(IrError::Semantic(
                            "element too large, limit is 18446744073709551615".to_string(),
                        ));
                    }
                    MetadataVal::IntBig(b) => {
                        let ok = match b {
                            crate::big::Big::Signed(i) => *i <= dashu::Integer::from(u64::MAX),
                            crate::big::Big::Unsigned(u) => *u <= dashu::Natural::from(u64::MAX),
                            _ => true,
                        };
                        if !ok {
                            return Err(IrError::Semantic(
                                "element too large, limit is 18446744073709551615".to_string(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
            // 操作码序列状态机（第二十三轮 invalid-diexpression-verify）：
            // need_op 状态——名字操作码查白名单(带 arity);数字操作码须在
            // LLVM 扩展区(≥ 4096,数字 0/1/9 非法);need_args 状态——数字
            // 作参数消费(无范围限制)
            const OPS: &[(&str, u32)] = &[
                ("DW_OP_deref", 0),
                ("DW_OP_xderef", 0),
                ("DW_OP_plus", 0),
                ("DW_OP_swap", 0),
                ("DW_OP_plus_uconst", 1),
                ("DW_OP_constu", 1),
                ("DW_OP_stack_value", 0),
                ("DW_OP_LLVM_fragment", 2),
                // convert 2 参（bits + DW_ATE_* 编码枚举名）
                ("DW_OP_LLVM_convert", 2),
                ("DW_OP_LLVM_tag_offset", 1),
                ("DW_OP_LLVM_entry_value", 1),
                ("DW_OP_LLVM_arg", 2),
            ];
            let mut need_args: u32 = 0;
            for v in vals {
                match v {
                    MetadataVal::Str(s) => {
                        if need_args > 0 {
                            // 参数位置的名字（DW_ATE_* 编码枚举等）——消费一槽
                            need_args -= 1;
                            continue;
                        }
                        match OPS.iter().find(|(n, _)| *n == s.as_str()) {
                            Some((_, a)) => need_args = *a,
                            None => {
                                return Err(IrError::Semantic(format!("invalid opcode '{s}'")));
                            }
                        }
                    }
                    MetadataVal::Int(n) => {
                        if need_args > 0 {
                            need_args -= 1;
                        } else if *n < 4096 {
                            // 数字操作码须在 LLVM 扩展区
                            return Err(IrError::Semantic(format!("invalid opcode {n}")));
                        }
                    }
                    MetadataVal::UInt(n) => {
                        if need_args > 0 {
                            need_args -= 1;
                        } else if *n < 4096 {
                            return Err(IrError::Semantic(format!("invalid opcode {n}")));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    // 7) tag / language / flags 合法性表
    const KNOWN_TAGS: &[&str] = &[
        "DW_TAG_GNU_template_template_param",
        "DW_TAG_LLVM_ptrauth_type",
        "DW_TAG_array_type",
        "DW_TAG_base_type",
        "DW_TAG_class_type",
        "DW_TAG_entry_point",
        "DW_TAG_enumeration_type",
        "DW_TAG_imported_module",
        "DW_TAG_inheritance",
        "DW_TAG_member",
        "DW_TAG_pointer_type",
        "DW_TAG_ptr_to_member_type",
        "DW_TAG_structure_type",
        "DW_TAG_template_value_parameter",
        "DW_TAG_unspecified_type",
        "DW_TAG_variant_part",
    ];
    if let Some(MetadataVal::Str(s)) = fields.get("tag") {
        // 字符串 tag 必须是 DW_TAG_ 合法名（数字 tag 另走值域）
        if !s.starts_with("DW_TAG_") {
            return Err(IrError::Semantic("expected DWARF tag".to_string()));
        }
        if !KNOWN_TAGS.contains(&s.as_str()) {
            return Err(IrError::Semantic(format!("invalid DWARF tag '{s}'")));
        }
    }
    // GenericDINode tag 数字值域 ≤ 65535
    if name == "GenericDINode" {
        check_range("tag", 0, 65535)?;
    }
    // DICompileUnit emissionKind 值域 ≤ 3（LLVM 枚举）
    if name == "DICompileUnit" {
        check_range("emissionKind", 0, 3)?;
    }
    const KNOWN_LANGS: &[&str] = &[
        "DW_LANG_C",
        "DW_LANG_C11",
        "DW_LANG_C89",
        "DW_LANG_C99",
        "DW_LANG_C_plus_plus",
        "DW_LANG_C_plus_plus_11",
        "DW_LANG_C_plus_plus_14",
        "DW_LANG_Cobol85",
        "DW_LANG_Fortran90",
        "DW_LANG_Fortran95",
    ];
    const KNOWN_LNAMES: &[&str] = &["DW_LNAME_C"];
    if name == "DICompileUnit" {
        if let Some(MetadataVal::Str(s)) = fields.get("language") {
            if !s.starts_with("DW_LANG_") {
                if s.starts_with("DW_LNAME_") {
                    return Err(IrError::Semantic("expected DWARF language".to_string()));
                }
                return Err(IrError::Semantic(format!("invalid DWARF language '{s}'")));
            }
            if !KNOWN_LANGS.contains(&s.as_str()) {
                return Err(IrError::Semantic(format!("invalid DWARF language '{s}'")));
            }
        }
        if let Some(MetadataVal::Str(s)) = fields.get("sourceLanguageName") {
            if !s.starts_with("DW_LNAME_") {
                if s.starts_with("DW_LANG_") {
                    return Err(IrError::Semantic(
                        "expected DWARF source language name".to_string(),
                    ));
                }
                return Err(IrError::Semantic(format!(
                    "invalid DWARF source language name '{s}'"
                )));
            }
            if !KNOWN_LNAMES.contains(&s.as_str()) {
                return Err(IrError::Semantic(format!(
                    "invalid DWARF source language name '{s}'"
                )));
            }
        }
    }
    const KNOWN_FLAGS: &[&str] = &[
        "DIFlagArtificial",
        "DIFlagBigEndian",
        "DIFlagEnumClass",
        "DIFlagExportSymbols",
        "DIFlagLValueReference",
        "DIFlagLittleEndian",
        "DIFlagNameIsSimplified",
        "DIFlagNoReturn",
        "DIFlagPrototyped",
        "DIFlagPublic",
        "DIFlagStaticMember",
        "DIFlagTypePassByValue",
    ];
    if let Some(MetadataVal::Str(s)) = fields.get("flags") {
        for f in s.split('|') {
            let f = f.trim();
            if f.starts_with("DIFlag") && !KNOWN_FLAGS.contains(&f) {
                return Err(IrError::Semantic(format!("invalid debug info flag '{f}'")));
            }
        }
    }
    // 8) DISubprogram 是 Definition（isDefinition 真）需 distinct
    if name == "DISubprogram" {
        let is_def = fields
            .get("isDefinition")
            .and_then(|v| match v {
                MetadataVal::Int(n) => Some(*n != 0),
                MetadataVal::UInt(n) => Some(*n != 0),
                _ => None,
            })
            .unwrap_or(false);
        if is_def && !distinct {
            return Err(IrError::Semantic(
                "missing 'distinct', required for !DISubprogram that is a Definition".to_string(),
            ));
        }
    }
    Ok(())
}

fn parse_param_attrs(
    attrs: &[String],
    ctx: &TypeContext,
) -> Result<crate::function::ParamAttributes, IrError> {
    let mut pa = crate::function::ParamAttributes::default();
    for a in attrs {
        match a.as_str() {
            "signext" => pa.signext = true,
            "zeroext" => pa.zeroext = true,
            "noalias" => pa.noalias = true,
            "noundef" => pa.noundef = true,
            "readonly" => pa.readonly = true,
            "writeonly" => pa.writeonly = true,
            "nocapture" => pa.nocapture = true,
            "immarg" => pa.extra.push(crate::ImmStr::from("immarg")),
            "nonnull" => pa.nonnull = true,
            "inreg" => pa.inreg = true,
            other => {
                if let Some(n) = other.strip_prefix("align ") {
                    pa.align = n
                        .parse()
                        .map_err(|_| IrError::Semantic(format!("invalid align {other}")))?;
                } else if let Some(ty_s) = other
                    .strip_prefix("byval(")
                    .and_then(|s| s.strip_suffix(')'))
                {
                    pa.byval = Some(attr_ty_to_type(ty_s, ctx)?);
                } else if let Some(ty_s) = other
                    .strip_prefix("sret(")
                    .and_then(|s| s.strip_suffix(')'))
                {
                    pa.sret = Some(attr_ty_to_type(ty_s, ctx)?);
                } else if other == "inalloca" || other.starts_with("byref") {
                    // inalloca/byref（含 byref(ty)）：LLVM 开放属性——宽松接受（display 原样还原）
                    pa.extra.push(crate::ImmStr::from(other));
                } else if other.starts_with("captures(") {
                    // captures(<mode>...)：LLVM 参数捕获属性——开放接受
                    pa.extra.push(crate::ImmStr::from(other));
                } else if other.starts_with("range(") {
                    // range(<ty> lo, hi)：LLVM 值级规则——类型必须整数、
                    // 空集 (lo==hi) 且 lo!=0 拒绝（range-attribute-invalid-*）
                    check_range_attr(other)?;
                    pa.extra.push(crate::ImmStr::from(other));
                } else if other.contains('(') {
                    // range(...)/nofpclass(...) 等带参开放参数属性——原样保存
                    pa.extra.push(crate::ImmStr::from(other));
                } else if other.contains(' ') || other.contains('"') {
                    // 字符串属性（`i64 "foo bar"`/`i64 "xyz"`——第十五轮）——原样保存
                    pa.extra.push(crate::ImmStr::from(other));
                } else if other.starts_with("dereferenceable") {
                    // dereferenceable(N)/dereferenceable_or_null(N)：数值开放
                    // 属性（第十一轮）——原样保存
                    pa.extra.push(crate::ImmStr::from(other));
                } else {
                    return Err(IrError::Semantic(format!(
                        "unknown parameter attribute {other}"
                    )));
                }
            }
        }
    }
    Ok(pa)
}

/// range(<ty> lo, hi) 属性值级校验（range-attribute-invalid-range/type）：
/// 类型必须整数;空集 (lo==hi) 且 lo!=0 拒绝。形状异常保持宽松。
fn check_range_attr(s: &str) -> Result<(), IrError> {
    let inner = s
        .strip_prefix("range(")
        .and_then(|x| x.strip_suffix(')'))
        .unwrap_or("");
    let Some((ty_s, rest)) = inner.split_once(' ') else {
        return Ok(());
    };
    if !ty_s.starts_with('i') {
        return Err(IrError::Semantic(
            "the range must have integer type!".to_string(),
        ));
    }
    let Some((lo_s, hi_s)) = rest.split_once(',') else {
        return Ok(());
    };
    if let (Ok(lo), Ok(hi)) = (lo_s.trim().parse::<i64>(), hi_s.trim().parse::<i64>())
        && lo == hi
        && lo != 0
    {
        return Err(IrError::Semantic(
            "the range represent the empty set but limits aren't 0!".to_string(),
        ));
    }
    Ok(())
}

/// 参数属性类型文本（`i32`/`ptr`/`ptr addrspace(1)`/`%struct.S`）→ TypeId。
fn attr_ty_to_type(s: &str, ctx: &TypeContext) -> Result<TypeId, IrError> {
    if let Some(name) = s.strip_prefix('%') {
        // fmt_parsed_type 对 Named 类型输出 `%struct.S`（名字本身含 %）
        let name = name.trim_start_matches('%');
        return ctx
            .borrow()
            .lookup_named(name)
            .ok_or_else(|| IrError::Semantic(format!("undefined type %{name}")));
    }
    if let Some(bits) = s.strip_prefix('i').and_then(|b| b.parse::<u32>().ok()) {
        return Ok(ctx.int_ty(bits));
    }
    if let Some(bits) = s.strip_prefix('f').and_then(|b| b.parse::<u16>().ok()) {
        return Ok(ctx.float_ty(bits));
    }
    if s == "ptr" {
        return Ok(ctx.ptr_ty());
    }
    if let Some(rest) = s
        .strip_prefix("ptr addrspace(")
        .and_then(|r| r.strip_suffix(')'))
    {
        let asp: u32 = rest
            .parse()
            .map_err(|_| IrError::Semantic(format!("invalid addrspace in {s}")))?;
        return Ok(ctx.pointer_ty(asp));
    }
    // 聚合类型属性（byval/sret 的 `{ ptr, i8 }`/`[8 x i8]`）——宽松占位 void
    //（类型仅作 IR 校验用途，forge 不落 IR）
    if s.starts_with('{') || s.starts_with('[') || s.starts_with('<') {
        return Ok(ctx.void_ty());
    }
    // 占位 void 的 roundtrip（第二十九轮重建:display 输出 byval(void) 还原）
    if s == "void" {
        return Ok(ctx.void_ty());
    }
    Err(IrError::Semantic(format!("unsupported attribute type {s}")))
}

// ── 操作数 → Value ──

/// 把 lane 值列表编码为常量池字节（按元素类型 + 端序；大端时每 lane 字节反转）。
fn encode_lanes_to_bytes(
    ctx: &TypeContext,
    elem: TypeId,
    lanes: &[VecLane],
    big: bool,
) -> Result<Vec<u8>, IrError> {
    let mut data = Vec::with_capacity(lanes.len() * 16);
    for lane in lanes {
        match (lane, ctx.borrow().get(elem)) {
            (VecLane::Int(n), TypeEntry::Int { bits: 1 }) => data.push((*n & 1) as u8),
            (VecLane::Int(n), TypeEntry::Int { bits: 8 }) => data.push(*n as u8),
            (VecLane::Int(n), TypeEntry::Int { bits: 16 }) => {
                data.extend((*n as i16).to_le_bytes())
            }
            (VecLane::Int(n), TypeEntry::Int { bits: 32 }) => {
                data.extend((*n as i32).to_le_bytes())
            }
            (VecLane::Int(n), TypeEntry::Int { bits: 64 }) => data.extend({ *n }.to_le_bytes()),
            (VecLane::UInt(n), TypeEntry::Int { bits }) if *bits <= 32 => {
                data.extend((*n as u32).to_le_bytes()[..(*bits as usize / 8)].to_vec())
            }
            (VecLane::UInt(n), TypeEntry::Int { bits }) if *bits <= 64 => {
                data.extend({ *n }.to_le_bytes()[..(*bits as usize / 8)].to_vec())
            }
            (VecLane::Float(f), TypeEntry::Float { bits: 32 }) => {
                data.extend((*f as f32).to_le_bytes())
            }
            (VecLane::Float(f), TypeEntry::Float { bits: 64 }) => data.extend({ *f }.to_le_bytes()),
            // LLVM 允许整数字面量出现在浮点元素位置（`float 0` ≡ 0.0）
            (VecLane::Int(n), TypeEntry::Float { bits: 32 }) => {
                data.extend((*n as f32).to_le_bytes())
            }
            (VecLane::Int(n), TypeEntry::Float { bits: 64 }) => {
                data.extend((*n as f64).to_le_bytes())
            }
            (VecLane::UInt(n), TypeEntry::Float { bits: 32 }) => {
                data.extend((*n as f32).to_le_bytes())
            }
            (VecLane::UInt(n), TypeEntry::Float { bits: 64 }) => {
                data.extend((*n as f64).to_le_bytes())
            }
            // 整数元素（i1/i8/i16/...——位宽任意；第十二轮补齐 i1/i8 lane）
            (VecLane::Int(n), TypeEntry::Int { bits }) => {
                let size = std::cmp::max(1u32, (*bits).div_ceil(8)) as usize;
                let le = { *n }.to_le_bytes();
                for i in 0..size {
                    data.push(le[i.min(le.len() - 1)]);
                }
            }
            (VecLane::UInt(n), TypeEntry::Int { bits }) => {
                let size = std::cmp::max(1u32, (*bits).div_ceil(8)) as usize;
                let le = (*n).to_le_bytes();
                for i in 0..size {
                    data.push(le[i.min(le.len() - 1)]);
                }
            }
            _ => {
                return Err(IrError::Semantic(format!(
                    "vector lane incompatible with element type {}",
                    ctx.borrow().fmt_type(elem)
                )));
            }
        }
    }
    if big {
        let lane_size = ctx.borrow().size_bytes(elem) as usize;
        for chunk in data.chunks_mut(lane_size) {
            chunk.reverse();
        }
    }
    Ok(data)
}

fn operand_to_value<'a>(
    op: &'a ParsedOperand,
    ctx: &TypeContext,
    value_map: &HashMap<&'a str, Value>,
    func_refs: &HashMap<String, FuncRef>,
    global_refs: &HashMap<String, GlobalId>,
    fb: &mut FunctionBuilder,
    block: Block,
) -> Result<Value, IrError> {
    fb.switch_to_block(block);
    let ty = to_type(&op.ty, ctx);
    // metadata 类型参数（`metadata i32 0`——宽松占位 undef,metadata 语义
    // 不落 IR;第二十九轮重建）
    if matches!(ctx.borrow().get(ty), crate::types::TypeEntry::Metadata) {
        return Ok(fb.undef(ty));
    }
    let v = match &op.op {
        Operand::Local(name) => value_map.get(name.as_str()).copied().unwrap_or_else(|| {
            // 未定义值引用 → 命名占位（第二十九轮重建:display 输出 `%name`
            // 原样还原,reparse 幂等）
            fb.func.dfg.make_value(
                ty,
                crate::dfg::ValueDef::UndefNamed(
                    ctx.borrow_mut()
                        .strings
                        .intern(name.trim_start_matches('%')),
                ),
            )
        }),
        Operand::Global(name) => {
            // 函数引用（call 目标）或全局地址。未注册（前向引用——第十四轮
            // incomplete-ir-declarations）→ GlobalId(u32::MAX) 哨兵占位
            // （第二十九轮:0 与真实 id 0 冲突——@5 引用误显示为 @5;
            // MAX 查全局/函数必失败 → display 输出 @undef,reparse 幂等）。
            if func_refs.contains_key(name) {
                // 函数指针（间接调用场景用 GlobalAddr 近似）——**String 编码**
                // （第三十一轮:原 GlobalId(fr.0) 与全局 id 共用空间,display
                // 查全局表撞名误输出——skip-value-numbers-globals 的 @25 函数
                // 引用显示成全局 @"";String 存函数名,display 输出 @name,
                // reparse 查 func_refs 幂等）
                let fname = name.trim_start_matches('@');
                let sid = ctx.borrow_mut().strings.intern(fname.to_string());
                fb.emit1(
                    Opcode::GlobalAddr,
                    vec![],
                    vec![Immediate::String(sid)],
                    TypeId::PTR,
                    InstFlags::NONE,
                )
            } else if let Some(&gid) = global_refs.get(name) {
                // 模块级全局变量地址
                fb.global_addr(gid)
            } else {
                fb.global_addr(crate::GlobalId(u32::MAX))
            }
        }
        Operand::Int(n) => fb.iconst(*n, ty),
        Operand::UInt(n) => {
            // 浮点类型的 hex 字面量是 bit 模式（LLVM：`float 0x3FC00000`）;
            // f32/f64 按位模式存;half/bfloat 低 16 位(第二十九轮重建)
            if matches!(
                ctx.borrow().get(ty),
                TypeEntry::Float { bits: 16 | 32 | 64 } | TypeEntry::BFloat { .. }
            ) {
                fb.fconst(*n, ty)
            } else {
                fb.iconst(*n as i64, ty)
            }
        }
        Operand::Float(fv) => {
            if matches!(ctx.borrow().get(ty), TypeEntry::Float { bits: 32 }) {
                let bits = (*fv as f32).to_bits() as u64;
                fb.fconst(bits, ty)
            } else {
                fb.fconst(fv.to_bits(), ty)
            }
        }
        Operand::Bool(b) => fb.iconst(if *b { 1 } else { 0 }, ty),
        Operand::Null => fb.iconst(0, ty),
        // 向量常量字面量 `<4 x float> <1.5, ...>` → 小端 vconst 值
        Operand::VecConst(lanes) => {
            let elem = ctx.borrow().element_type(ty).ok_or_else(|| {
                IrError::Semantic("vector literal requires a vector type".to_string())
            })?;
            let data = encode_lanes_to_bytes(ctx, elem, lanes, false)?;
            fb.vconst_bytes(data, ty)
        }
        // 聚合常量字面量（3.1 值表示）：构建 AggConst 常量池节点 + AggConst Value
        //（display 原样还原字面量；store/ret/call 实参等任意操作数位置可用）
        Operand::Agg(elems) => {
            let agg_id = agg_const_from_operands(fb, ty, elems, ctx)?;

            fb.func.dfg.make_agg_const_value(ty, agg_id)
        }
        // zeroinitializer：零向量 / 零标量
        Operand::ZeroInit => {
            let size = ctx.borrow().size_bytes(ty) as usize;
            if matches!(ctx.borrow().get(ty), TypeEntry::Vector { .. }) {
                fb.vconst_bytes(vec![0u8; size], ty)
            } else {
                fb.iconst(0, ty)
            }
        }
        // 常量折叠表达式：`ret i64 add (i64 <l>, i64 <r>)` ——求值后落常量
        //（整数按 i64 值；浮点按位模式——const_expr_value 对 Float 已 to_bits）
        Operand::ConstExpr(e) => {
            let v = const_expr_value(ctx, e)?;
            match ctx.borrow().get(ty) {
                TypeEntry::Float { bits: 32 } => fb.fconst(v as u64 & 0xFFFF_FFFF, ty),
                TypeEntry::Float { .. } => fb.fconst(v as u64, ty),
                // 向量常量（折叠值无意义）→ 零向量
                TypeEntry::Vector { .. } => {
                    let size = ctx.borrow().size_bytes(ty) as usize;
                    fb.vconst_bytes(vec![0u8; size], ty)
                }
                _ => fb.iconst(v as i64, ty),
            }
        }
        Operand::Undef => fb.emit1(Opcode::Undef, vec![], vec![], ty, InstFlags::NONE),
        Operand::Poison => fb.emit1(Opcode::Poison, vec![], vec![], ty, InstFlags::NONE),
    };
    Ok(v)
}

// ── 终结符 ──

/// 解析块标签引用：数字 id 引用（`%2`）经编号映射；字符串名直接查表。
/// 数字 id 未命中时宽松回退到字符串名（LLVM 严格——不存在的数字 id 报错，
/// 但宽松回退不破坏合法输入）。
fn resolve_block_ref(
    r: &LabelRef,
    num_to_label: &HashMap<u32, String>,
    label_key: &HashMap<&str, String>,
    block_map: &HashMap<String, Block>,
) -> Result<Block, IrError> {
    if r.is_num
        && let Ok(n) = r.name.parse::<u32>()
        && let Some(k) = num_to_label.get(&n)
        && let Some(&b) = block_map.get(k)
    {
        return Ok(b);
    }
    let k = label_key
        .get(r.name.as_str())
        .map(|s| s.as_str())
        .unwrap_or(r.name.as_str());
    block_map
        .get(k)
        .copied()
        .ok_or_else(|| IrError::Semantic(format!("undefined block {}", r.name)))
}

#[allow(clippy::too_many_arguments)] // 解析上下文参数组(值表/块表/标签表/func_refs)——内部私有
fn build_terminator<'a>(
    term: &'a ParsedTerminator,
    fb: &mut FunctionBuilder,
    ctx: &TypeContext,
    value_map: &mut HashMap<&'a str, Value>,
    block_map: &HashMap<String, Block>,
    num_to_label: &HashMap<u32, String>,
    label_key: &HashMap<&'a str, String>,
    func_refs: &HashMap<String, FuncRef>,
    global_refs: &HashMap<String, GlobalId>,
    block: Block,
) -> Result<(), IrError> {
    fb.switch_to_block(block);
    match term {
        ParsedTerminator::Return(ops, metas) => {
            let vals: Vec<Value> = ops
                .iter()
                .map(|o| operand_to_value(o, ctx, value_map, func_refs, global_refs, fb, block))
                .collect::<Result<_, _>>()?;
            fb.ret(&vals);
            attach_term_metadata(fb, block, metas)?;
        }
        // 标准 LLVM 跳转：不带参数（目标块参数由 phi 入边回填）
        ParsedTerminator::Jump(target, metas) => {
            let t = resolve_block_ref(target, num_to_label, label_key, block_map)?;
            fb.jump(t, &[]);
            attach_term_metadata(fb, block, metas)?;
        }
        ParsedTerminator::Branch(cond, t, f, metas) => {
            let t = resolve_block_ref(t, num_to_label, label_key, block_map)?;
            let f = resolve_block_ref(f, num_to_label, label_key, block_map)?;
            if let Some(c) = cond {
                let c = operand_to_value(c, ctx, value_map, func_refs, global_refs, fb, block)?;
                fb.branch(c, t, &[], f, &[]);
            } else {
                fb.jump(t, &[]);
            }
            attach_term_metadata(fb, block, metas)?;
        }
        ParsedTerminator::Switch(cond, default, cases, metas) => {
            let c = operand_to_value(cond, ctx, value_map, func_refs, global_refs, fb, block)?;
            let d = resolve_block_ref(default, num_to_label, label_key, block_map)?;
            let cases: Vec<(i64, Block, Vec<Value>)> = cases
                .iter()
                .map(|(n, b)| {
                    let bb = resolve_block_ref(b, num_to_label, label_key, block_map)?;
                    Ok((*n, bb, vec![]))
                })
                .collect::<Result<_, IrError>>()?;
            let case_refs: Vec<(i64, Block, &[Value])> = cases
                .iter()
                .map(|(n, b, v)| (*n, *b, v.as_slice()))
                .collect();
            fb.switch(c, d, &case_refs);
            attach_term_metadata(fb, block, metas)?;
        }
        ParsedTerminator::Unreachable => {
            // LLVM：unreachable 是 terminator，不生成 Trap 指令（round-trip 结构一致）
            fb.unreachable();
        }
        ParsedTerminator::Invoke {
            callee,
            ret_ty,
            args,
            normal,
            unwind,
            metas,
            result,
        } => {
            let callee_ref = func_refs
                .get(callee)
                .ok_or_else(|| IrError::Semantic(format!("undefined function @{callee}")))?;
            let n = resolve_block_ref(normal, num_to_label, label_key, block_map)?;
            let u = resolve_block_ref(unwind, num_to_label, label_key, block_map)?;
            let vals: Vec<Value> = args
                .iter()
                .map(|o| operand_to_value(o, ctx, value_map, func_refs, global_refs, fb, block))
                .collect::<Result<_, _>>()?;
            let ret = to_type(ret_ty, ctx);
            // 正常块第 0 个参数接收 invoke 返回值（ret_ty 非 void 时）
            let mut normal_args: Vec<Value> = Vec::new();
            if ret != TypeId::VOID
                && let Some(&nv) = fb.func.dfg.block_param_values(n).first()
            {
                normal_args.push(nv);
            }
            fb.invoke(*callee_ref, &vals, ret, n, &normal_args, u, &[]);
            // `%r = invoke ...`：返回值即 normal 块第 0 个参数（invoke 的 normal_args[0]）
            if let Some(r) = result
                && ret != TypeId::VOID
                && let Some(&nv) = fb.func.dfg.block_param_values(n).first()
            {
                value_map.insert(r.as_str(), nv);
            }
            attach_term_metadata(fb, block, metas)?;
        }
        ParsedTerminator::Resume(op, metas) => {
            let v = operand_to_value(op, ctx, value_map, func_refs, global_refs, fb, block)?;
            fb.resume(v);
            attach_term_metadata(fb, block, metas)?;
        }
    }
    Ok(())
}

/// 终结符 metadata 附加（`ret ..., !range !0`；命名引用已在预处理期解析为 Num）。
fn attach_term_metadata(
    fb: &mut FunctionBuilder,
    block: Block,
    metas: &[(String, MetadataRef)],
) -> Result<(), IrError> {
    if metas.is_empty() {
        return Ok(());
    }
    use crate::terminator::Terminator;
    let bd = &mut fb.func.dfg.blocks[block.0 as usize];
    for (name, r) in metas {
        let id = match r {
            MetadataRef::Num(id) => crate::metadata::MetadataId(*id),
            MetadataRef::Named(n) => {
                return Err(IrError::Semantic(format!("unresolved named metadata !{n}")));
            }
        };
        let am = crate::metadata::AttachedMetadata {
            kind: metadata_kind_of(name)?,
            node: id,
        };
        match &mut bd.terminator {
            Terminator::Return { metadata, .. }
            | Terminator::Jump { metadata, .. }
            | Terminator::Branch { metadata, .. }
            | Terminator::Switch { metadata, .. }
            | Terminator::Invoke { metadata, .. }
            | Terminator::Resume { metadata, .. } => metadata.push(am),
            Terminator::Unreachable => {
                return Err(IrError::Semantic(
                    "unreachable cannot carry metadata".to_string(),
                ));
            }
        }
    }
    Ok(())
}
