//! 函数和模块 — IR 顶层容器。
//!
//! `Function` 封装 DFG、UseLists、常量池、分析缓存。
//! `Module` 管理多个函数、全局变量、类型系统和模块元数据。

use super::analysis::DominatorTree;
use super::constant::ConstantPool;
use super::data_layout::DataLayout;
use super::data_layout::TargetTriple;
use super::debug_info::{DebugInfo, SourceLocation};
use super::dfg::{BlockData, DataFlowGraph};
use super::entity::*;
use super::loop_info::LoopForest;
use super::metadata::{AttachedMetadata, MetadataStore};
use super::stack_slot::StackSlots;
use super::string_pool::InternedStr;
use super::symbol::{Comdat, ComdatId, ComdatKind, SymbolInfo};
use super::types::{CallConv, FunctionSignature, TypeContext};
use super::use_list::UseLists;
use smallvec::SmallVec;
use std::cell::UnsafeCell;
use std::collections::HashMap;

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
/// 在 Function 中存储为 `UnsafeCell` (内部可变性)。
#[derive(Default)]
pub struct AnalysisCache {
    pub predecessors: Option<HashMap<Block, Vec<Block>>>,
    pub successors: Option<HashMap<Block, Vec<Block>>>,
    /// 支配树 (惰性构建)
    pub dominator_tree: Option<DominatorTree>,
    /// 循环森林 (惰性构建，依赖 DominatorTree)
    pub loop_forest: Option<LoopForest>,
}

impl AnalysisCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn invalidate(&mut self) {
        self.predecessors = None;
        self.successors = None;
        self.dominator_tree = None;
        self.loop_forest = None;
    }
}

// ============================================================
// Function
// ============================================================

/// IR 函数 — SSA 函数的主要容器。
pub struct Function {
    /// 函数名.
    pub name: String,

    /// 函数签名引用 (TypeStore 中的权威签名)。
    pub signature: SigRef,

    /// 缓存的参数类型 (无需 TypeStore 即可快速访问)。
    pub param_tys: Vec<TypeId>,

    /// 缓存的返回类型。
    pub return_tys: Vec<TypeId>,

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

    /// 栈槽管理 — 为 alloca、spill、emergency slot 预留。
    pub stack_slots: StackSlots,

    // === 属性 ===
    pub attributes: FunctionAttributes,

    /// 参数属性 — 按参数索引。
    pub param_attrs: Vec<ParamAttributes>,

    /// 返回值属性。
    pub ret_attrs: Vec<ParamAttributes>,

    /// 附加的 metadata — (kind, MetadataId) 对。
    pub metadata: SmallVec<[AttachedMetadata; 2]>,

    /// 符号信息 — linkage, visibility, section, comdat, TLS。
    pub symbol: SymbolInfo,

    /// 是否为 const 函数 (编译时可求值)。
    pub is_const: bool,

    // === 入口 ===
    pub entry_block: Option<Block>,

    // === 调试 ===
    pub debug_info: Option<DebugInfo>,
    /// Deprecated: use `Instruction.loc` instead.
    /// Kept for backward compatibility — prefer setting loc on the Instruction directly.
    #[deprecated(note = "use Instruction.loc instead")]
    pub source_locations: HashMap<Value, SourceLocation>,

    /// 值名称 — 调试和 IR 打印时使用 (e.g., %add1, %cmp)。
    pub value_names: HashMap<Value, InternedStr>,

    /// 块名称 — 调试和 IR 打印时使用 (e.g., %entry, %loop_body)。
    pub block_names: HashMap<Block, InternedStr>,

    // === 分析缓存 ===
    pub analysis: UnsafeCell<AnalysisCache>,
}

// SourceLocation is defined in debug_info.rs — re-exported via pub use

impl Function {
    pub fn new(
        name: String,
        signature: SigRef,
        param_tys: Vec<TypeId>,
        return_tys: Vec<TypeId>,
        calling_convention: CallConv,
    ) -> Self {
        Self {
            name,
            signature,
            param_tys,
            return_tys,
            calling_convention,
            dfg: DataFlowGraph::new(),
            layout: Layout::new(),
            use_lists: UseLists::new(),
            constants: ConstantPool::new(),
            stack_slots: StackSlots::new(),
            attributes: FunctionAttributes::NONE,
            param_attrs: Vec::new(),
            ret_attrs: Vec::new(),
            metadata: SmallVec::new(),
            symbol: SymbolInfo::new(),
            is_const: false,
            entry_block: None,
            debug_info: None,
            #[allow(deprecated)]
            source_locations: HashMap::new(),
            value_names: HashMap::new(),
            block_names: HashMap::new(),
            analysis: UnsafeCell::new(AnalysisCache::new()),
        }
    }

    // ============================================================
    // 分析访问 (unsafe 内部可变性)
    // ============================================================

    pub fn with_const(mut self, is_const: bool) -> Self {
        self.is_const = is_const;
        self
    }

    pub fn with_attributes(mut self, attrs: FunctionAttributes) -> Self {
        self.attributes = attrs;
        self
    }

    // ============================================================
    // 分析访问 (unsafe 内部可变性)
    // ============================================================

    /// 获取分析缓存的不可变引用。
    ///
    /// # Safety
    /// 调用者必须确保没有同时进行的可变借用。
    pub fn analysis(&self) -> &AnalysisCache {
        unsafe { &*self.analysis.get() }
    }

    /// 获取分析缓存的可变引用。
    ///
    /// # Safety
    /// 调用者必须确保没有其他借用。
    #[allow(clippy::mut_from_ref)]
    pub fn analysis_mut(&self) -> &mut AnalysisCache {
        unsafe { &mut *self.analysis.get() }
    }

    /// 按 Block 获取 BlockData。
    pub fn block(&self, id: Block) -> &BlockData {
        self.dfg.block(id)
    }

    /// 按 Block 获取可变 BlockData。
    pub fn block_mut(&mut self, id: Block) -> &mut BlockData {
        self.dfg.block_mut(id)
    }

    /// 惰性获取支配树。
    pub fn dominator_tree(&self) -> &DominatorTree {
        let cache = self.analysis_mut();
        if cache.dominator_tree.is_none() {
            cache.dominator_tree = Some(DominatorTree::build(self));
        }
        // SAFETY: DominatorTree is stored in the cache and returned with
        // the same lifetime as &self.
        unsafe {
            let cache_ref = &*self.analysis.get();
            cache_ref.dominator_tree.as_ref().unwrap()
        }
    }

    /// 惰性获取前驱映射: Block → Vec<Block>.
    pub fn predecessors(&self) -> &HashMap<Block, Vec<Block>> {
        let cache = self.analysis_mut();
        if cache.predecessors.is_none() {
            let mut preds: HashMap<Block, Vec<Block>> = HashMap::new();
            for (block, bd) in self.dfg.blocks() {
                for succ in bd.terminator.successors() {
                    preds.entry(succ).or_default().push(block);
                }
            }
            cache.predecessors = Some(preds);
        }
        unsafe { &*self.analysis.get() }
            .predecessors
            .as_ref()
            .unwrap()
    }

    /// 惰性获取后继映射: Block → Vec<Block>.
    pub fn successors(&self) -> &HashMap<Block, Vec<Block>> {
        let cache = self.analysis_mut();
        if cache.successors.is_none() {
            let mut succs: HashMap<Block, Vec<Block>> = HashMap::new();
            for (block, bd) in self.dfg.blocks() {
                succs.insert(block, bd.terminator.successors());
            }
            cache.successors = Some(succs);
        }
        unsafe { &*self.analysis.get() }
            .successors
            .as_ref()
            .unwrap()
    }

    /// 惰性获取循环森林 (依赖 DominatorTree).
    pub fn loop_forest(&self) -> &LoopForest {
        let cache = self.analysis_mut();
        if cache.loop_forest.is_none() {
            let dt = self.dominator_tree();
            cache.loop_forest = Some(LoopForest::build(self, dt));
        }
        // SAFETY: LoopForest is stored in the cache and returned with
        // the same lifetime as &self.
        unsafe {
            let cache_ref = &*self.analysis.get();
            cache_ref.loop_forest.as_ref().unwrap()
        }
    }

    // ============================================================
    // 便捷查询
    // ============================================================

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
    pub name: String,
    pub ty: TypeId,
    pub init: Option<Vec<u8>>,
    /// Symbol information — linkage, visibility, section, comdat, TLS.
    pub symbol: SymbolInfo,
    pub is_constant: bool,
    pub alignment: u32,
}

impl GlobalVariable {
    pub fn constant(name: &str, ty: TypeId) -> Self {
        Self {
            name: name.to_string(),
            ty,
            init: None,
            symbol: SymbolInfo::new(),
            is_constant: true,
            alignment: 0,
        }
    }

    pub fn mutable(name: &str, ty: TypeId) -> Self {
        Self {
            name: name.to_string(),
            ty,
            init: None,
            symbol: SymbolInfo::new(),
            is_constant: false,
            alignment: 0,
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
    func_names: HashMap<String, FuncRef>,

    /// 全局变量表。
    globals: Vec<GlobalVariable>,
    global_names: HashMap<String, GlobalId>,

    /// 模块级常量池 (跨函数共享)。
    pub constants: ConstantPool,

    /// Comdat 组 — 用于链接时去重 (C++ inline, 模板实例化等)。
    comdats: Vec<Comdat>,
    comdat_names: HashMap<String, ComdatId>,

    /// 别名 — 符号别名映射。
    aliases: Vec<super::symbol::Alias>,

    /// 元数据存储 — 跨函数共享的 metadata 节点。
    pub metadata_store: MetadataStore,

    /// 目标数据布局 — 控制类型大小、对齐、指针宽度。
    pub data_layout: DataLayout,

    /// 目标三元组 — 目标架构、供应商、OS、环境。
    pub target_triple: Option<TargetTriple>,
}

impl Module {
    pub fn new() -> Self {
        Self {
            types: TypeContext::new(),
            functions: Vec::new(),
            func_names: HashMap::new(),
            globals: Vec::new(),
            global_names: HashMap::new(),
            constants: ConstantPool::new(),
            comdats: Vec::new(),
            comdat_names: HashMap::new(),
            aliases: Vec::new(),
            metadata_store: MetadataStore::new(),
            data_layout: DataLayout::default(),
            target_triple: None,
        }
    }

    /// 使用指定的 DataLayout 创建 Module。
    pub fn with_data_layout(data_layout: DataLayout) -> Self {
        let types = TypeContext::with_data_layout(data_layout.clone());
        Self {
            types,
            functions: Vec::new(),
            func_names: HashMap::new(),
            globals: Vec::new(),
            global_names: HashMap::new(),
            constants: ConstantPool::new(),
            comdats: Vec::new(),
            comdat_names: HashMap::new(),
            aliases: Vec::new(),
            metadata_store: MetadataStore::new(),
            data_layout,
            target_triple: None,
        }
    }

    /// 设置目标三元组。
    pub fn set_target_triple(&mut self, triple: &str) {
        self.target_triple = Some(TargetTriple::parse(triple));
    }

    // ============================================================
    // 函数管理
    // ============================================================

    pub fn add_function(&mut self, func: Function) -> FuncRef {
        let name_str = func.name.clone();
        let func_ref = FuncRef(self.functions.len() as u32);
        self.func_names.insert(name_str, func_ref);
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

    pub fn add_global(&mut self, gv: GlobalVariable) -> Result<GlobalId, String> {
        let name = gv.name.clone();
        if self.global_names.contains_key(&name) {
            return Err(format!("global '{}' already exists", name));
        }
        let id = GlobalId(self.globals.len() as u32);
        self.global_names.insert(name, id);
        self.globals.push(gv);
        Ok(id)
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

    /// 通过 FuncRef 获取函数签名。
    pub fn get_function_signature(&self, fr: FuncRef) -> Option<FunctionSignature> {
        let func = self.get_function(fr);
        Some(self.types.get_signature(func.signature))
    }

    // ============================================================
    // Comdat 管理
    // ============================================================

    /// 添加一个 comdat 组，返回 ComdatId。
    pub fn add_comdat(&mut self, name: &str, kind: ComdatKind) -> Result<ComdatId, String> {
        if self.comdat_names.contains_key(name) {
            return Err(format!("comdat '{}' already exists", name));
        }
        let id = ComdatId(self.comdats.len() as u32);
        self.comdat_names.insert(name.to_string(), id);
        self.comdats.push(Comdat {
            name: name.to_string(),
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

    // ============================================================
    // 别名管理
    // ============================================================

    /// 添加一个别名。
    pub fn add_alias(&mut self, alias: super::symbol::Alias) {
        self.aliases.push(alias);
    }

    /// 获取所有别名。
    pub fn iter_aliases(&self) -> impl Iterator<Item = &super::symbol::Alias> {
        self.aliases.iter()
    }

    /// 别名数量。
    pub fn alias_count(&self) -> usize {
        self.aliases.len()
    }

    // ============================================================
    // 迭代
    // ============================================================

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
            name.to_string(),
            sig_ref,
            vec![],
            vec![],
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
}
