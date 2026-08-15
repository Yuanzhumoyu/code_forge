//! 函数和模块 — IR 顶层容器。
//!
//! `Function` 封装 DFG、UseLists、常量池、分析缓存。
//! `Module` 管理多个函数、全局变量、类型系统和模块元数据。

use crate::ImmStr;
use crate::error::IrError;

use super::analysis::DominatorTree;
use super::constant::ConstantPool;
use super::data_layout::DataLayout;
use super::data_layout::TargetTriple;
use super::debug_info::DebugInfo;
use super::dfg::{BlockData, DataFlowGraph, Instruction};
use super::entity::*;
use super::loop_info::LoopForest;
use super::metadata::{AttachedMetadata, MetadataStore};
use super::opcode::Opcode;
use super::string_pool::InternedStr;
use super::symbol::{Comdat, ComdatId, ComdatKind, SymbolInfo};
use super::terminator::Terminator;
use super::types::{CallConv, FunctionSignature, TypeContext};
use super::use_list::UseLists;
use smallvec::SmallVec;
use std::collections::HashMap;
use std::sync::OnceLock;

// ============================================================
// FunctionAttributes
// ============================================================

/// 函数级属性位掩码 — 扩展自 LLVM 的 AttributeList。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FunctionAttributes(u32);

impl FunctionAttributes {
    pub const NONE: Self = Self(0);
    // Inline hints
    pub const INLINE_ALWAYS: Self = Self(1 << 0);
    pub const INLINE_NEVER: Self = Self(1 << 1);
    // Purity
    pub const CONST: Self = Self(1 << 2); // reads no memory, no side effects
    pub const PURE: Self = Self(1 << 3); // reads memory, no side effects
    // Control flow
    pub const NO_RECURSE: Self = Self(1 << 4); // does not recurse
    pub const NO_UNWIND: Self = Self(1 << 5); // does not throw
    pub const WILL_RETURN: Self = Self(1 << 6); // always returns normally
    pub const NO_RETURN: Self = Self(1 << 7); // never returns (e.g., exit)
    // Memory effects
    pub const READ_ONLY: Self = Self(1 << 8); // only reads memory
    pub const WRITE_ONLY: Self = Self(1 << 9); // only writes memory
    pub const ARG_MEM_ONLY: Self = Self(1 << 10); // only accesses argument memory
    pub const INACCESSIBLE_MEM_ONLY: Self = Self(1 << 11); // no alias with regular memory
    // Optimization hints
    pub const COLD: Self = Self(1 << 12); // rarely executed
    pub const HOT: Self = Self(1 << 13); // frequently executed
    pub const OPT_NONE: Self = Self(1 << 14); // no optimization
    pub const OPT_SIZE: Self = Self(1 << 15); // optimize for size
    // Calling convention
    pub const NO_RED_ZONE: Self = Self(1 << 16); // no red zone in stack frame
    pub const SPECULATABLE: Self = Self(1 << 17); // safe to speculate

    pub const fn contains(self, attr: Self) -> bool {
        (self.0 & attr.0) != 0
    }

    pub fn set(&mut self, attr: Self) {
        self.0 |= attr.0;
    }
}

// ============================================================
// ParamAttributes
// ============================================================

/// Attributes attached to a function parameter or return value.
#[derive(Clone, Debug, Default)]
pub struct ParamAttributes {
    /// Zero-extend the value before passing (small integer → larger register).
    pub zeroext: bool,
    /// Sign-extend the value before passing.
    pub signext: bool,
    /// The value does not alias any other memory.
    pub noalias: bool,
    /// The callee only reads this pointer (not writes).
    pub readonly: bool,
    /// The callee only writes to this pointer (not reads).
    pub writeonly: bool,
    /// Pass by value (copy the struct).
    pub byval: Option<TypeId>,
    /// Struct return — this parameter is the return buffer.
    pub sret: Option<TypeId>,
    /// Pass in register if possible.
    pub inreg: bool,
    /// The callee does not capture this pointer.
    pub nocapture: bool,
    /// The pointer is guaranteed non-null.
    pub nonnull: bool,
    /// Required alignment (0 = natural alignment).
    pub align: u32,
    /// The value is guaranteed not to be poison/undef at this call site.
    pub noundef: bool,
    /// 开放集合参数属性（byref/inalloca 等——display 原样还原）。
    pub extra: Vec<crate::ImmStr>,
}

impl ParamAttributes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_align(mut self, align: u32) -> Self {
        self.align = align;
        self
    }

    pub fn with_nonnull(mut self) -> Self {
        self.nonnull = true;
        self
    }

    pub fn with_noalias(mut self) -> Self {
        self.noalias = true;
        self
    }

    pub fn with_sret(mut self, ty: TypeId) -> Self {
        self.sret = Some(ty);
        self
    }
}

// ============================================================
// Layout
// ============================================================

/// 块的布局顺序。
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub block_order: Vec<Block>,
}

impl Layout {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, block: Block) {
        self.block_order.push(block);
    }
}

// ============================================================
// AnalysisCache
// ============================================================

/// 分析缓存 — 惰性计算的 CFG 分析结果。
///
/// 用 [`OnceLock`]（原子惰性初始化）替代原来的 `UnsafeCell` 内部可变性：
/// - 多线程共享 `&Function` 时并发调用惰性分析方法安全（无 data race，无需 unsafe impl Sync）
/// - 失效需 `&mut self`（`invalidate`），与其它线程的 `&self` 共享由借用检查互斥
#[derive(Default)]
pub struct AnalysisCache {
    pub predecessors: OnceLock<HashMap<Block, Vec<Block>>>,
    pub successors: OnceLock<HashMap<Block, Vec<Block>>>,
    /// 支配树 (惰性构建)
    pub dominator_tree: OnceLock<DominatorTree>,
    /// 循环森林 (惰性构建，依赖 DominatorTree)
    pub loop_forest: OnceLock<LoopForest>,
}

impl AnalysisCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn invalidate(&mut self) {
        self.predecessors = OnceLock::new();
        self.successors = OnceLock::new();
        self.dominator_tree = OnceLock::new();
        self.loop_forest = OnceLock::new();
    }
}

// ============================================================
// Function
// ============================================================

/// IR 函数 — SSA 函数的主要容器。
/// Clone: compile_raw 需要可变的 IR 副本（Stage 3 pattern isel 就地重写）。
/// `analysis`（OnceLock 缓存）不克隆——副本的缓存按需重建。
///
/// # Thread safety
/// `analysis` 是 [`OnceLock`]-backed 惰性分析缓存：多线程共享 `&Function`
/// 时并发调用 `predecessors`/`successors`/`dominator_tree`/`loop_forest`
/// 是安全的（原子初始化，无 data race）。失效（`invalidate`）需要 `&mut self`，
/// 与其它线程的 `&self` 共享由借用检查互斥。
pub struct Function {
    /// 函数名.
    pub name: ImmStr,

    /// 函数签名引用 (TypeStore 中的权威签名)。
    pub signature: SigRef,

    /// 缓存的调用约定。
    pub calling_convention: CallConv,

    // === 核心数据 ===
    /// 数据流图: Value/Inst/Block 的存储。
    pub dfg: DataFlowGraph,

    /// 块布局顺序。
    pub layout: Layout,

    /// Use-List: 增量维护的 def-use 链。
    pub use_lists: UseLists,

    /// 函数级常量池。
    pub constants: ConstantPool,

    // === 属性 ===
    pub attributes: FunctionAttributes,
    /// 未知函数属性名（`uwtable`/`nosync` 等开放集合——位标志外原样保存，
    /// display 还原；LLVM 属性名开放，宽松接受）。
    pub extra_attrs: Vec<crate::ImmStr>,

    /// 参数属性 — 按参数索引。
    pub param_attrs: Vec<ParamAttributes>,

    /// 返回值属性。
    pub ret_attrs: Vec<ParamAttributes>,

    /// 附加的 metadata — (kind, MetadataId) 对。
    pub metadata: SmallVec<[AttachedMetadata; 2]>,

    /// personality 函数（异常处理：`personality ptr @__gxx_personality_v0`；P1.1 文本层）。
    pub personality: Option<FuncRef>,

    /// 符号信息 — linkage, visibility, section, comdat, TLS。
    pub symbol: SymbolInfo,

    /// 是否为 const 函数 (编译时可求值)。
    pub is_const: bool,

    // === 入口 ===
    pub entry_block: Option<Block>,

    // === 调试 ===
    pub debug_info: Option<DebugInfo>,

    /// 类型上下文 — 持有动态（interned）类型的定义，供 lowering/后续阶段
    /// 按位宽推导寄存器类（动态 vector 长度感知）。
    pub types: TypeContext,

    /// 值名称 — 调试和 IR 打印时使用 (e.g., %add1, %cmp)。
    pub value_names: HashMap<Value, InternedStr>,

    /// 块名称 — 调试和 IR 打印时使用 (e.g., %entry, %loop_body)。
    pub block_names: HashMap<Block, InternedStr>,

    // === 分析缓存 ===
    pub analysis: AnalysisCache,
}

impl Clone for Function {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            signature: self.signature,
            calling_convention: self.calling_convention,
            dfg: self.dfg.clone(),
            layout: self.layout.clone(),
            use_lists: self.use_lists.clone(),
            constants: self.constants.clone(),

            attributes: self.attributes,
            extra_attrs: self.extra_attrs.clone(),
            param_attrs: self.param_attrs.clone(),
            ret_attrs: self.ret_attrs.clone(),
            metadata: self.metadata.clone(),
            personality: self.personality,
            symbol: self.symbol.clone(),
            is_const: self.is_const,
            entry_block: self.entry_block,
            debug_info: self.debug_info.clone(),
            types: self.types.clone(),
            value_names: self.value_names.clone(),
            block_names: self.block_names.clone(),
            analysis: AnalysisCache::default(),
        }
    }
}

// SourceLocation is defined in debug_info.rs — re-exported via pub use

impl Function {
    /// 构造空函数（无 entry block/指令）。
    ///
    /// `types` 必须已注册 `sig_ref` 对应的签名（`TypeContext::register_signature`），
    /// 之后 `param_types()`/`return_types()` 从签名查询——不再有冗余字段。
    pub fn new(
        name: impl Into<ImmStr>,
        types: TypeContext,
        signature: SigRef,
        calling_convention: CallConv,
    ) -> Self {
        Self {
            name: name.into(),
            signature,
            calling_convention,
            dfg: DataFlowGraph::new(),
            layout: Layout::new(),
            use_lists: UseLists::new(),
            constants: ConstantPool::new(),

            attributes: FunctionAttributes::NONE,
            extra_attrs: Vec::new(),
            param_attrs: Vec::new(),
            ret_attrs: Vec::new(),
            metadata: SmallVec::new(),
            personality: None,
            symbol: SymbolInfo::new(),
            is_const: false,
            entry_block: None,
            types,
            debug_info: None,
            value_names: HashMap::new(),
            block_names: HashMap::new(),
            analysis: AnalysisCache::new(),
        }
    }

    /// 参数类型（来自权威签名）。
    pub fn param_types(&self) -> Vec<TypeId> {
        self.types.get_signature(self.signature).param_types()
    }

    /// 返回类型（来自权威签名）。
    pub fn return_types(&self) -> Vec<TypeId> {
        self.types.get_signature(self.signature).returns.clone()
    }

    // ============================================================
    // 分析访问 (unsafe 内部可变性)
    // ============================================================

    /// 获取分析缓存的不可变引用（只读；惰性初始化由 OnceLock 保证并发安全）。
    pub fn analysis(&self) -> &AnalysisCache {
        &self.analysis
    }

    /// 获取分析缓存的可变引用（失效缓存用）。需要 `&mut self`——
    /// 与其它线程的 `&self` 共享由借用检查互斥。
    pub fn analysis_mut(&mut self) -> &mut AnalysisCache {
        &mut self.analysis
    }

    /// 按 Block 获取 BlockData。
    pub fn block(&self, id: Block) -> &BlockData {
        self.dfg.block(id)
    }

    /// 按 Block 获取可变 BlockData。
    pub fn block_mut(&mut self, id: Block) -> &mut BlockData {
        self.dfg.block_mut(id)
    }

    /// 惰性获取支配树（OnceLock 原子初始化，并发安全）。
    pub fn dominator_tree(&self) -> &DominatorTree {
        self.analysis
            .dominator_tree
            .get_or_init(|| DominatorTree::build(self))
    }

    /// 惰性获取前驱映射: Block → Vec<Block>.
    pub fn predecessors(&self) -> &HashMap<Block, Vec<Block>> {
        self.analysis.predecessors.get_or_init(|| {
            let mut preds: HashMap<Block, Vec<Block>> = HashMap::new();
            for (block, bd) in self.dfg.blocks() {
                for succ in bd.terminator.successors() {
                    preds.entry(succ).or_default().push(block);
                }
            }
            preds
        })
    }

    /// 惰性获取后继映射: Block → Vec<Block>.
    pub fn successors(&self) -> &HashMap<Block, Vec<Block>> {
        self.analysis.successors.get_or_init(|| {
            let mut succs: HashMap<Block, Vec<Block>> = HashMap::new();
            for (block, bd) in self.dfg.blocks() {
                succs.insert(block, bd.terminator.successors());
            }
            succs
        })
    }

    /// 惰性获取循环森林 (依赖 DominatorTree).
    pub fn loop_forest(&self) -> &LoopForest {
        self.analysis.loop_forest.get_or_init(|| {
            let dt = self.dominator_tree();
            LoopForest::build(self, dt)
        })
    }

    // ============================================================
    // 原子 IR 修改原语（保持 use_lists 新鲜）
    // ============================================================

    /// RAUW：把 `old` 的所有使用替换为 `new`，同步更新 DFG 操作数与 use-lists。
    /// 返回被替换的使用数量。
    ///
    /// 注意：只覆盖指令操作数（use-lists 记录范围）；终结符参数请用
    /// PHASE 2 的 `Terminator::map_values` / `Function::retarget`。
    pub fn replace_all_uses(&mut self, old: Value, new: Value) -> usize {
        let uses: Vec<(Inst, u8)> = self
            .use_lists
            .uses(old)
            .iter()
            .map(|u| (u.user, u.operand_idx))
            .collect();
        let count = uses.len();
        for (user, operand_idx) in uses {
            if let Some(inst) = self.dfg.insts.get_mut(user.0 as usize)
                && let Some(slot) = inst.operands.get_mut(operand_idx as usize)
            {
                *slot = new;
            }
        }
        self.use_lists.replace_all_uses(old, new);
        count
    }

    /// 原子删除指令：use-lists 清理 + 墓碑化 + 结果值 VOID 化。
    /// 调用方须保证结果值无活跃使用（或先 RAUW）。
    pub fn kill_inst(&mut self, inst: Inst) {
        self.use_lists.remove_inst(&self.dfg, inst);
        for &r in self.dfg.inst_results(inst) {
            self.use_lists.remove_value(r);
        }
        self.dfg.remove_inst(inst);
    }

    /// RAUW + kill 一体：把 `old_inst` 的每个结果替换为 `new_values` 中对应的
    /// 新值，然后原子删除 `old_inst`。替代 pass 中手工"replace_all_uses +
    /// 清字段 + remove_inst"的成对调用，保证 use_lists 始终新鲜。
    ///
    /// `new_values` 长度须等于 `old_inst` 的结果数。
    pub fn replace_all_uses_and_kill(&mut self, old_inst: Inst, new_values: &[Value]) {
        let results: Vec<Value> = self.dfg.inst_results(old_inst).to_vec();
        assert_eq!(
            results.len(),
            new_values.len(),
            "replace_all_uses_and_kill: inst {} has {} results but {} replacement values",
            old_inst,
            results.len(),
            new_values.len()
        );
        for (old, &new) in results.iter().zip(new_values.iter()) {
            self.replace_all_uses(*old, new);
        }
        self.kill_inst(old_inst);
    }

    /// 把 `block` 终结符中指向 `old_target` 的所有跳转目标改为 `new_target`
    /// （参数原样保留；不改变其它块的终结符）。
    /// 删除块的第 `idx` 个参数，并同步清理**所有前驱终结符**传给该位置的
    /// 参数（替代 block_param_coalesce 的手工逐前驱删除）。
    /// 被删除参数值的其它使用（非终结符）由调用方先 RAUW。
    pub fn remove_block_param(&mut self, block: Block, idx: usize) {
        // 1. 先收集前驱（不可变借用结束），再改终结符
        let preds: Vec<Block> = self.predecessors().get(&block).cloned().unwrap_or_default();
        // 2. 清理前驱终结符对应参数位置
        for pred in preds {
            if let Some(pd) = self.dfg.blocks.get_mut(pred.0 as usize) {
                pd.terminator.remove_arg(block, idx);
            }
        }
        // 3. 从块参数表删除
        let bd = &mut self.dfg.blocks[block.0 as usize];
        if idx < bd.params.len() {
            bd.params.remove(idx);
        }
        if idx < bd.param_values.len() {
            bd.param_values.remove(idx);
        }
    }

    /// 批量替换：把 `replacements`（old → new）一次性应用到所有指令操作数与
    /// 终结符参数，**同步维护 use-lists**（单趟语义：不追溯链式映射——
    /// 与 pass 现有批量替换行为一致）。
    /// 返回被替换的操作数总数。
    pub fn apply_replacements(&mut self, replacements: &HashMap<Value, Value>) -> usize {
        let mut count = 0;
        // 指令操作数 + use-lists 同步（dfg 与 use_lists 为不相交字段，可同时可变借用）
        for (idx, inst) in self.dfg.insts.iter_mut().enumerate() {
            if matches!(inst.opcode, Opcode::Nop) {
                continue;
            }
            let inst_id = Inst(idx as u32);
            for (operand_idx, operand) in inst.operands.iter_mut().enumerate() {
                if let Some(&replacement) = replacements.get(operand) {
                    let old = *operand;
                    *operand = replacement;
                    self.use_lists
                        .replace_operand(old, replacement, inst_id, operand_idx as u8);
                    count += 1;
                }
            }
        }
        // 终结符参数（不在 use-lists 中，直接改值）
        for block in self.dfg.blocks.iter_mut() {
            match &mut block.terminator {
                Terminator::Branch {
                    cond,
                    then_args,
                    else_args,
                    ..
                } => {
                    if let Some(&r) = replacements.get(cond) {
                        *cond = r;
                        count += 1;
                    }
                    for v in then_args.iter_mut().chain(else_args.iter_mut()) {
                        if let Some(&r) = replacements.get(v) {
                            *v = r;
                            count += 1;
                        }
                    }
                }
                Terminator::Jump { args, .. } => {
                    for v in args.iter_mut() {
                        if let Some(&r) = replacements.get(v) {
                            *v = r;
                            count += 1;
                        }
                    }
                }
                Terminator::Return { values, .. } => {
                    for v in values.iter_mut() {
                        if let Some(&r) = replacements.get(v) {
                            *v = r;
                            count += 1;
                        }
                    }
                }
                Terminator::Switch {
                    discriminant,
                    default_args,
                    cases,
                    ..
                } => {
                    if let Some(&r) = replacements.get(discriminant) {
                        *discriminant = r;
                        count += 1;
                    }
                    for v in default_args.iter_mut() {
                        if let Some(&r) = replacements.get(v) {
                            *v = r;
                            count += 1;
                        }
                    }
                    for (_, _, args) in cases.iter_mut() {
                        for v in args.iter_mut() {
                            if let Some(&r) = replacements.get(v) {
                                *v = r;
                                count += 1;
                            }
                        }
                    }
                }
                Terminator::Invoke {
                    args,
                    normal_args,
                    unwind_args,
                    ..
                } => {
                    for v in args
                        .iter_mut()
                        .chain(normal_args.iter_mut())
                        .chain(unwind_args.iter_mut())
                    {
                        if let Some(&r) = replacements.get(v) {
                            *v = r;
                            count += 1;
                        }
                    }
                }
                Terminator::Resume { value, .. } => {
                    if let Some(&r) = replacements.get(value) {
                        *value = r;
                        count += 1;
                    }
                }
                Terminator::Unreachable => {}
            }
        }
        count
    }

    // ============================================================
    // 便捷查询
    // ============================================================

    /// 统一遍历单个块内活跃指令（可变），跳 Nop 墓碑。
    /// 借用约束同 [`DataFlowGraph::block_insts_mut`]：闭包内不可再访问
    /// 本函数其它字段（需同步 use-lists 的修改请用 kill_inst/apply_replacements）。
    pub fn block_for_insts_mut(&mut self, block: Block, mut f: impl FnMut(&mut Instruction)) {
        for inst in self.dfg.block_insts_mut(block) {
            f(inst);
        }
    }

    /// 统一遍历函数内所有块活跃指令（可变），跳 Nop 墓碑。
    pub fn for_insts_mut(&mut self, mut f: impl FnMut(&mut Instruction)) {
        let block_count = self.dfg.blocks.len();
        for bi in 0..block_count {
            let block = Block(bi as u32);
            self.block_for_insts_mut(block, &mut f);
        }
    }

    pub fn block_count(&self) -> usize {
        self.dfg.block_count()
    }

    pub fn inst_count(&self) -> usize {
        self.dfg.inst_count()
    }

    pub fn value_count(&self) -> usize {
        self.dfg.value_count()
    }
}

// ============================================================
// GlobalVariable
// ============================================================

// Linkage, Visibility, ComdatKind, ComdatId, SymbolInfo, Alias are now in symbol.rs

#[derive(Clone, Debug)]
pub struct GlobalVariable {
    pub name: ImmStr,
    pub ty: TypeId,
    pub init: Option<Vec<u8>>,
    /// 常量表达式 init（`ptrtoint (ptr @h to i32)`）——display 原样还原文本；
    /// 字节 init（占位）与表达式并存（真实地址为链接期重定位，P1 文本层占位 0）。
    pub init_expr: Option<crate::ir_parser::ast_items::ConstExpr>,
    /// Symbol information — linkage, visibility, section, comdat, TLS.
    pub symbol: SymbolInfo,
    pub is_constant: bool,
    pub alignment: u32,
    /// 全局地址空间（`@g = addrspace(1) global ...`；0 = 默认）。
    pub addr_space: u32,
    /// 全局尾 metadata 附加（`@g = global i32 0, !absolute_symbol !0`）。
    pub metadata: Vec<crate::metadata::AttachedMetadata>,
    /// ifunc（第二十九轮:IR 表示——`@f = ifunc <retty> (<params>), ptr @resolver`;
    /// ty 为返回类型;参数类型存解析层文本;resolver 为解析器名）。
    pub is_ifunc: bool,
    pub ifunc_params: Vec<crate::imm_str::ImmStr>,
    pub ifunc_resolver: Option<crate::imm_str::ImmStr>,
}

/// 模块级别名：`@a = alias <ty>, <aliasee>`（aliasee 为常量表达式，文本还原）。
pub struct GlobalAlias {
    pub name: ImmStr,
    pub ty: TypeId,
    pub linkage: crate::symbol::Linkage,
    pub dso_local: bool,
    pub unnamed_addr: bool,
    /// aliasee：类型前缀（TypeOp 形式）或 Void 占位（括号表达式自带类型）。
    pub aliasee_ty: Option<crate::ir_parser::ast_items::ParsedType>,
    pub aliasee: crate::ir_parser::ast_items::ConstExpr,
    pub metadata: Vec<crate::metadata::AttachedMetadata>,
}

impl GlobalVariable {
    pub fn constant(name: &str, ty: TypeId) -> Self {
        Self {
            name: ImmStr::from(name),
            ty,
            init: None,
            init_expr: None,
            symbol: SymbolInfo::new(),
            is_constant: true,
            alignment: 0,
            addr_space: 0,
            metadata: Vec::new(),
            is_ifunc: false,
            ifunc_params: Vec::new(),
            ifunc_resolver: None,
        }
    }

    pub fn mutable(name: &str, ty: TypeId) -> Self {
        Self {
            name: ImmStr::from(name),
            ty,
            init: None,
            init_expr: None,
            symbol: SymbolInfo::new(),
            is_constant: false,
            alignment: 0,
            addr_space: 0,
            metadata: Vec::new(),
            is_ifunc: false,
            ifunc_params: Vec::new(),
            ifunc_resolver: None,
        }
    }

    pub fn with_init(mut self, data: Vec<u8>) -> Self {
        self.init = Some(data);
        self
    }

    pub fn with_linkage(mut self, linkage: super::symbol::Linkage) -> Self {
        self.symbol.linkage = linkage;
        self
    }

    pub fn with_symbol(mut self, symbol: SymbolInfo) -> Self {
        self.symbol = symbol;
        self
    }
}

// ============================================================
// Module
// ============================================================

/// IR 模块 — 多个函数的容器，持有共享的类型系统和常量池。
pub struct Module {
    /// 类型系统 (跨函数共享)。
    pub types: TypeContext,

    /// 函数表。
    functions: Vec<Function>,
    func_names: HashMap<ImmStr, FuncRef>,

    /// 全局变量表。
    globals: Vec<GlobalVariable>,
    /// 模块级别名（`@a = alias ...`；与 symbol::Alias 并存）。
    global_aliases: Vec<GlobalAlias>,
    global_names: HashMap<ImmStr, GlobalId>,

    /// 模块级常量池 (跨函数共享)。
    pub constants: ConstantPool,

    /// Comdat 组 — 用于链接时去重 (C++ inline, 模板实例化等)。
    comdats: Vec<Comdat>,
    comdat_names: HashMap<ImmStr, ComdatId>,

    /// 元数据存储 — 跨函数共享的 metadata 节点。
    pub metadata_store: MetadataStore,

    /// 目标数据布局 — 控制类型大小、对齐、指针宽度。
    pub data_layout: DataLayout,

    /// 目标三元组 — 目标架构、供应商、OS、环境。
    pub target_triple: Option<TargetTriple>,

    /// `source_filename = "..."`（LLVM 模块元数据；编译产物调试用）。
    pub source_filename: Option<String>,

    /// `module asm "..."`（模块级内联汇编；可多条，按序输出）。
    pub module_asm: Vec<String>,
}

impl Module {
    pub fn new() -> Self {
        Self {
            types: TypeContext::new(),
            functions: Vec::new(),
            func_names: HashMap::new(),
            globals: Vec::new(),
            global_aliases: Vec::new(),
            global_names: HashMap::new(),
            constants: ConstantPool::new(),
            comdats: Vec::new(),
            comdat_names: HashMap::new(),
            metadata_store: MetadataStore::new(),
            data_layout: DataLayout::default(),
            target_triple: None,
            source_filename: None,
            module_asm: Vec::new(),
        }
    }

    /// 设置 DataLayout 并**同步重建类型上下文**——`TypeStore` 的 size/align
    /// 查询走内嵌 DataLayout（见 `TypeStore::size_bytes`/`alignment`），两者
    /// 必须一致。必须在 `add_function` 之前调用（重建会清空已注册签名）。
    ///
    /// 修复历史 bug：`parse_module` 曾只写 `data_layout` 字段，导致
    /// `types` 内嵌布局仍旧默认（x86_64），跨文件布局查询错位。
    pub fn set_data_layout(&mut self, data_layout: DataLayout) {
        self.types = TypeContext::with_data_layout(data_layout.clone());
        self.data_layout = data_layout;
    }

    /// 设置目标三元组。
    pub fn set_target_triple(&mut self, triple: &str) {
        self.target_triple = Some(TargetTriple::parse(triple));
    }

    // ============================================================
    // 函数管理
    // ============================================================

    pub fn add_function(&mut self, func: Function) -> FuncRef {
        let name = func.name.clone();
        let func_ref = FuncRef(self.functions.len() as u32);
        self.func_names.insert(name, func_ref);
        self.functions.push(func);
        func_ref
    }

    pub fn get_function(&self, fr: FuncRef) -> &Function {
        &self.functions[fr.0 as usize]
    }

    pub fn get_function_mut(&mut self, fr: FuncRef) -> &mut Function {
        &mut self.functions[fr.0 as usize]
    }

    pub fn find_function(&self, name: &str) -> Option<FuncRef> {
        self.func_names.get(name).copied()
    }

    pub fn function_count(&self) -> usize {
        self.functions.len()
    }

    pub fn iter_functions(&self) -> impl Iterator<Item = &Function> {
        self.functions.iter()
    }

    pub fn iter_functions_mut(&mut self) -> impl Iterator<Item = &mut Function> {
        self.functions.iter_mut()
    }

    // ============================================================
    // 全局变量
    // ============================================================

    pub fn add_global(&mut self, gv: GlobalVariable) -> Result<GlobalId, IrError> {
        let name = gv.name.clone();
        if self.global_names.contains_key(&name) {
            return Err(IrError::DuplicateSymbol(name.to_string()));
        }
        let id = GlobalId(self.globals.len() as u32);
        self.global_names.insert(name, id);
        self.globals.push(gv);
        Ok(id)
    }

    /// 添加模块级别名（`@a = alias ...`）。
    pub fn add_global_alias(&mut self, a: GlobalAlias) -> Result<(), IrError> {
        // 重复定义：global 同名（global_names）或已有 alias 同名
        if self.global_names.contains_key(&a.name)
            || self.global_aliases.iter().any(|x| x.name == a.name)
        {
            return Err(IrError::DuplicateSymbol(a.name.to_string()));
        }
        self.global_aliases.push(a);
        Ok(())
    }

    /// 迭代模块级别名（`@a = alias ...`）。
    pub fn iter_global_aliases(&self) -> impl Iterator<Item = &GlobalAlias> {
        self.global_aliases.iter()
    }

    pub fn get_global(&self, id: GlobalId) -> Option<&GlobalVariable> {
        self.globals.get(id.0 as usize)
    }

    pub fn global_count(&self) -> usize {
        self.globals.len()
    }

    /// 按名查找全局变量。
    pub fn find_global(&self, name: &str) -> Option<GlobalId> {
        self.global_names.get(name).copied()
    }

    /// 迭代所有全局变量。
    pub fn iter_globals(&self) -> impl Iterator<Item = (GlobalId, &GlobalVariable)> {
        self.globals
            .iter()
            .enumerate()
            .map(|(i, gv)| (GlobalId(i as u32), gv))
    }

    /// 可变迭代所有全局变量。
    pub fn iter_globals_mut(&mut self) -> impl Iterator<Item = (GlobalId, &mut GlobalVariable)> {
        self.globals
            .iter_mut()
            .enumerate()
            .map(|(i, gv)| (GlobalId(i as u32), gv))
    }

    /// 通过 FuncRef 获取函数签名。
    pub fn get_function_signature(&self, fr: FuncRef) -> Option<FunctionSignature> {
        let func = self.get_function(fr);
        Some(self.types.get_signature(func.signature))
    }

    // ============================================================
    // Comdat 管理
    // ============================================================

    /// 添加一个 comdat 组，返回 ComdatId。
    pub fn add_comdat(&mut self, name: &str, kind: ComdatKind) -> Result<ComdatId, IrError> {
        if self.comdat_names.contains_key(name) {
            return Err(IrError::DuplicateSymbol(name.to_string()));
        }
        let id = ComdatId(self.comdats.len() as u32);
        self.comdat_names.insert(ImmStr::from(name), id);
        self.comdats.push(Comdat {
            name: ImmStr::from(name),
            kind,
        });
        Ok(id)
    }

    /// 按名查找 comdat。
    pub fn find_comdat(&self, name: &str) -> Option<ComdatId> {
        self.comdat_names.get(name).copied()
    }

    /// 获取 comdat 信息。
    pub fn get_comdat(&self, id: ComdatId) -> &Comdat {
        &self.comdats[id.0 as usize]
    }

    /// Comdat 数量。
    pub fn comdat_count(&self) -> usize {
        self.comdats.len()
    }

    /// 迭代所有 (FuncRef, &Function) 对。
    pub fn iter_func_refs(&self) -> impl Iterator<Item = (FuncRef, &Function)> {
        self.functions
            .iter()
            .enumerate()
            .map(|(i, f)| (FuncRef(i as u32), f))
    }
}

impl Default for Module {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TypeStore;

    fn make_test_func(name: &str) -> Function {
        let mut store = TypeStore::new();
        let sig = FunctionSignature::void();
        let sig_ref = store.register_signature(sig);
        Function::new(
            ImmStr::from(name),
            TypeContext::from_store(store),
            sig_ref,
            CallConv::default(),
        )
    }

    #[test]
    fn test_function_new() {
        let func = make_test_func("test");
        assert_eq!(func.block_count(), 0);
        assert_eq!(func.inst_count(), 0);
    }

    #[test]
    fn test_module_add_function() {
        let mut module = Module::new();
        let func = make_test_func("test_func");

        let fr = module.add_function(func);
        assert_eq!(fr, FuncRef(0));
        assert_eq!(module.function_count(), 1);
    }

    #[test]
    fn test_module_globals() {
        let mut module = Module::new();
        let gv = GlobalVariable::constant("test_global", module.types.i32_ty());
        let id = module.add_global(gv).unwrap();
        assert_eq!(id, GlobalId(0));
        assert_eq!(module.global_count(), 1);
        assert!(module.get_global(id).is_some());
    }

    /// 多线程共享 `&Function` 并发调用惰性分析方法（OnceLock 原子初始化，
    /// 无 data race——替代原 UnsafeCell + unsafe impl Sync 的约束）。
    #[test]
    fn test_analysis_cache_thread_safe() {
        use std::sync::Arc;

        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[(ctx.i32_ty(), "x")], &[ctx.i32_ty()]);
        let mut fb = crate::builder::FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, params) = fb.create_block_with_params(&[(TypeId::I32, "x")]);
        let then_blk = fb.create_block();
        let else_blk = fb.create_block();
        fb.switch_to_block(entry);
        fb.branch(params[0], then_blk, &[], else_blk, &[]);
        fb.switch_to_block(then_blk);
        let v1 = fb.iconst_i32(1);
        fb.ret(&[v1]);
        fb.switch_to_block(else_blk);
        let v2 = fb.iconst_i32(2);
        fb.ret(&[v2]);
        let func = Arc::new(fb.finish().expect("build"));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let f = Arc::clone(&func);
                std::thread::spawn(move || {
                    let preds = f.predecessors();
                    let succs = f.successors();
                    let dt = f.dominator_tree();
                    let lf = f.loop_forest();
                    (preds.len(), succs.len(), dt.block_count(), lf.len())
                })
            })
            .collect();
        for h in handles {
            let (p, s, d, l) = h.join().expect("analysis thread panicked");
            assert_eq!(p, 2, "predecessors: only blocks with preds are keys");
            assert_eq!(s, 3, "successors of 3 blocks");
            assert_eq!(d, 3, "dominator tree blocks");
            assert_eq!(l, 0, "no loops");
        }
    }

    /// RAUW 同步更新 DFG 操作数与 use-lists；kill 后 IR 仍有效。
    #[test]
    fn test_rauw_and_kill_keep_use_lists_fresh() {
        use crate::dfg::ValueDef;

        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = crate::builder::FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let a = fb.iconst_i32(1);
        let c = fb.iconst_i32(2);
        let x = fb.iadd(a, c);
        let y = fb.iadd(x, a);
        let z = fb.iadd(x, y);
        fb.ret(&[z]);

        assert!(
            fb.func.use_lists.verify(&fb.func.dfg).is_ok(),
            "初始 use-lists 一致"
        );

        // RAUW: x → c（x 被 y、z 使用）
        let count = fb.func.replace_all_uses(x, c);
        assert_eq!(count, 2, "x 被 y 和 z 使用");
        assert!(
            fb.func.use_lists.verify(&fb.func.dfg).is_ok(),
            "RAUW 后 use-lists 一致"
        );
        let ValueDef::Inst(y_inst, _) = fb.func.dfg.value_def(y).unwrap() else {
            panic!()
        };
        assert!(
            fb.func.dfg.insts[y_inst.0 as usize].operands.contains(&c),
            "DFG 侧 operands 已更新为 c"
        );
        assert!(
            !fb.func.dfg.insts[y_inst.0 as usize].operands.contains(&x),
            "DFG 侧不再引用 x"
        );

        // kill: x 已无 use → 墓碑化 + 结果 VOID 化
        let ValueDef::Inst(x_inst, _) = fb.func.dfg.value_def(x).unwrap() else {
            panic!()
        };
        fb.func.kill_inst(*x_inst);
        assert!(
            fb.func.use_lists.verify(&fb.func.dfg).is_ok(),
            "kill 后 use-lists 一致"
        );
        assert_eq!(
            fb.func.dfg.value_type(x),
            Some(TypeId::VOID),
            "已删值类型为 VOID"
        );

        let func = fb.finish().expect("build");
        let mut verifier = crate::verify::Verifier::with_ctx(ctx.clone());
        assert!(verifier.verify(&func).is_ok(), "RAUW+kill 后 IR 仍有效");
    }

    /// replace_all_uses_and_kill：一体替换 + 删除。
    #[test]
    fn test_replace_all_uses_and_kill() {
        use crate::dfg::ValueDef;

        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = crate::builder::FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        fb.switch_to_block(entry);
        let a = fb.iconst_i32(1);
        let c = fb.iconst_i32(2);
        let x = fb.iadd(a, c);
        let y = fb.iadd(x, a);
        fb.ret(&[y]);

        let ValueDef::Inst(x_inst, _) = fb.func.dfg.value_def(x).unwrap() else {
            panic!()
        };
        fb.func.replace_all_uses_and_kill(*x_inst, &[c]);
        assert!(
            fb.func.use_lists.verify(&fb.func.dfg).is_ok(),
            "一体 API 后 use-lists 一致"
        );
        assert_eq!(fb.func.dfg.value_type(x), Some(TypeId::VOID), "x 已被删除");

        let func = fb.finish().expect("build");
        let mut verifier = crate::verify::Verifier::with_ctx(ctx.clone());
        assert!(verifier.verify(&func).is_ok(), "一体替换后 IR 仍有效");
    }

    /// clone_inst 保留全字段（flags/mem_flags/metadata/loc/isel_strategy），
    /// 并自动维护 value_remap（旧结果 → 新结果）。
    #[test]
    fn test_clone_inst_preserves_fields() {
        use crate::dfg::ValueDef;
        use std::collections::HashMap;

        let ctx = TypeContext::new();
        let sig = FunctionSignature::new(&[], &[ctx.i32_ty()]);
        let mut fb = crate::builder::FunctionBuilder::new("test", ctx.clone(), sig);
        let (entry, _) = fb.create_entry_block();
        let target = fb.create_block();
        fb.switch_to_block(entry);
        let a = fb.iconst_i32(1);
        let c = fb.iconst_i32(2);
        let x = fb.iadd(a, c);
        fb.ret(&[x]);

        // 给 x 指令附加非默认字段
        let ValueDef::Inst(x_inst, _) = *fb.func.dfg.value_def(x).unwrap() else {
            panic!()
        };
        {
            let inst = &mut fb.func.dfg.insts[x_inst.0 as usize];
            inst.flags = crate::inst_flags::InstFlags::MAY_UB;
            inst.isel_strategy = Some("lea_sib:4");
            inst.loc = Some(crate::debug_info::SourceLocation {
                file: Some(crate::imm_str::ImmStr::from("test.rs")),
                line: Some(42),
                column: Some(7),
            });
        }

        let mut remap: HashMap<Value, Value> = HashMap::new();
        let new_inst = fb.func.dfg.clone_inst(x_inst, target, &mut remap);

        let orig = &fb.func.dfg.insts[x_inst.0 as usize];
        let cloned = &fb.func.dfg.insts[new_inst.0 as usize];
        assert_eq!(cloned.opcode, orig.opcode, "opcode");
        assert_eq!(cloned.operands, orig.operands, "operands（无映射时保持）");
        assert_eq!(cloned.immediates, orig.immediates, "immediates");
        assert_eq!(cloned.flags, orig.flags, "flags 保留");
        assert_eq!(cloned.mem_flags, orig.mem_flags, "mem_flags 保留");
        assert_eq!(cloned.metadata, orig.metadata, "metadata 保留");
        assert_eq!(cloned.loc, orig.loc, "loc 保留");
        assert_eq!(
            cloned.isel_strategy, orig.isel_strategy,
            "isel_strategy 保留"
        );
        assert_eq!(cloned.block, target, "克隆到目标块");
        // value_remap：旧结果 → 新结果
        let new_x = remap.get(&x).expect("旧结果应映射到新结果");
        assert_eq!(fb.func.dfg.value_type(*new_x), Some(TypeId::I32));
        assert_eq!(*new_x, cloned.results[0]);
    }

    /// ConstantPool::remap_from：跨池常量重定位（int/float/big/vector 全 tag）。
    #[test]
    fn test_remap_from_cross_pool() {
        let mut src = ConstantPool::new();
        let mut dst = ConstantPool::new();
        let i = src.insert_int(42, 64);
        let f = src.insert_float(1.5f64.to_bits());
        let b = src.insert_big(crate::big::Big::from_i128(1i128 << 100));
        let v = src.insert_vector(&[1, 2, 3, 4]);
        // dst 先插入一条让两池索引错开，验证重定位不依赖索引
        dst.insert_int(7, 8);

        let ri = dst.remap_from(&src, i);
        let rf = dst.remap_from(&src, f);
        let rb = dst.remap_from(&src, b);
        let rv = dst.remap_from(&src, v);
        assert_eq!(dst.get_int(ri), Some((42, 64)));
        assert_eq!(dst.get_float(rf), Some(1.5f64.to_bits()));
        assert_eq!(dst.get_big(rb), src.get_big(b));
        assert_eq!(dst.get_vector(rv), src.get_vector(v));
        assert_eq!(
            dst.get_vector_endian(rv),
            src.get_vector_endian(v),
            "向量端序保留"
        );
    }
}
