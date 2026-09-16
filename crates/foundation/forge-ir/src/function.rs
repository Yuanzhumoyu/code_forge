//! 函数和模块 — IR 顶层容器。
//!
//! `Function` 封装 DFG、UseLists、常量池、分析缓存。
//! `Module` 管理多个函数、全局变量、类型系统和模块元数据。

use crate::ImmStr;
use crate::entity_map::SecondaryMap;
use crate::error::IrError;

use super::analysis::{AnalysisManager, AnalysisRevision, DominatorTree};
use super::constant::ConstantPool;
use super::data_layout::DataLayout;
use super::data_layout::TargetTriple;
use super::debug_info::DebugInfo;
use super::dfg::{BlockData, DataFlowGraph, Instruction};
use super::entity::*;
use super::immediate::Immediate;
use super::loop_info::LoopForest;
use super::metadata::{AttachedMetadata, MetadataStore};
use super::opcode::Opcode;
use super::string_pool::InternedStr;
use super::symbol::{Comdat, ComdatId, ComdatKind, SymbolInfo};
use super::terminator::TermKind;
use super::types::{CallConv, FunctionSignature, TypeContext};
use super::use_list::UseLists;
use smallvec::SmallVec;
use std::collections::HashMap;
use std::sync::Arc;

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
// Function
// ============================================================

/// IR 函数 — SSA 函数的主要容器。
/// Clone: compile_raw 需要可变的 IR 副本（Stage 3 pattern isel 就地重写）。
/// `analysis`（惰性分析缓存）不克隆——副本的缓存按需重建。
///
/// # Thread safety
/// `analysis` 是 [`AnalysisManager`]：内部 `RwLock` 槽位，多线程共享 `&Function`
/// 并发调用 `predecessors`/`successors`/`dominator_tree`/`loop_forest` 安全
/// （无 data race；同一份分析可能被并发重算一次，结果等价）。
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
    ///
    /// **单写**（v3 方案 S5）：读 [`Function::metadata`]、写
    /// [`Function::attach_metadata`]。
    pub(crate) metadata: SmallVec<[AttachedMetadata; 2]>,

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
    pub value_names: SecondaryMap<Value, InternedStr>,

    /// 块名称 — 调试和 IR 打印时使用 (e.g., %entry, %loop_body)。
    pub block_names: SecondaryMap<Block, InternedStr>,

    // === 分析缓存 ===
    /// 惰性分析缓存管理器（私有；读写走 [`Function::analysis`] /
    /// [`Function::analysis_mut`]，分析本身走 `predecessors` 等访问器）。
    pub(crate) analysis: AnalysisManager,
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
            analysis: AnalysisManager::default(),
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
            value_names: SecondaryMap::new(),
            block_names: SecondaryMap::new(),
            analysis: AnalysisManager::new(),
        }
    }

    /// 函数附件 metadata（`(kind, node)` 对，按附加序）。
    pub fn metadata(&self) -> &[AttachedMetadata] {
        &self.metadata
    }

    /// 附加一条函数级 metadata —— **函数附件的唯一写入口**（S5：metadata 单写）。
    pub fn attach_metadata(&mut self, metadata: AttachedMetadata) {
        self.metadata.push(metadata);
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
    // 分析访问（快照式惰性缓存）
    // ============================================================

    /// 获取分析缓存管理器（只读；槽位自校验修订号，见 [`AnalysisManager`]）。
    pub fn analysis(&self) -> &AnalysisManager {
        &self.analysis
    }

    /// 获取分析缓存管理器的可变引用（显式失效用）。需要 `&mut self`——
    /// 与其它线程的 `&self` 共享由借用检查互斥。
    pub fn analysis_mut(&mut self) -> &mut AnalysisManager {
        &mut self.analysis
    }

    /// 控制流结构修订号（[`DataFlowGraph::cfg_revision`]）。
    pub fn cfg_revision(&self) -> u64 {
        self.dfg.cfg_revision()
    }

    /// 当前分析修订号 = (结构修订号, 显式失效次数)。
    fn analysis_revision(&self) -> AnalysisRevision {
        self.analysis.revision(self.cfg_revision())
    }

    /// 按 Block 获取 BlockData。
    pub fn block(&self, id: Block) -> &BlockData {
        self.dfg.block(id)
    }

    /// 按 Block 获取可变 BlockData。
    pub fn block_mut(&mut self, id: Block) -> &mut BlockData {
        self.dfg.block_mut(id)
    }

    /// 惰性获取支配树（当前修订号对应的快照）。
    ///
    /// 返回 `Arc` 快照而非 `&DominatorTree`：调用方拿到的整棵树在本次调用内恒定，
    /// 不受随后改写控制流的影响（改图只会让**下一次**读重算，见
    /// [`AnalysisManager`]）。
    pub fn dominator_tree(&self) -> Arc<DominatorTree> {
        let rev = self.analysis_revision();
        self.analysis
            .dominator_tree(rev, || DominatorTree::build(self))
    }

    /// 惰性获取前驱映射: Block → Vec<Block>（快照，语义见 [`Function::dominator_tree`]）。
    pub fn predecessors(&self) -> Arc<SecondaryMap<Block, Vec<Block>>> {
        let rev = self.analysis_revision();
        self.analysis.predecessors(rev, || {
            let mut preds: SecondaryMap<Block, Vec<Block>> = SecondaryMap::new();
            for (block, _bd) in self.dfg.blocks() {
                // CFG 构造容忍未终止块（校验器要在坏 IR 上跑）：无终结符 ⇒ 无出边
                for succ in self.dfg.block_successors(block) {
                    preds.get_mut_or_default(succ).push(block);
                }
            }
            preds
        })
    }

    /// 惰性获取后继映射: Block → Vec<Block>（快照，语义见 [`Function::dominator_tree`]）。
    pub fn successors(&self) -> Arc<SecondaryMap<Block, Vec<Block>>> {
        let rev = self.analysis_revision();
        self.analysis.successors(rev, || {
            let mut succs: SecondaryMap<Block, Vec<Block>> = SecondaryMap::new();
            for (block, _bd) in self.dfg.blocks() {
                succs.insert(block, self.dfg.block_successors(block));
            }
            succs
        })
    }

    /// 入口块。
    ///
    /// 契约：`entry_block` 由构造方（`FunctionBuilder::create_entry_block`、
    /// 文本解析器、lowering）设置。**缺失即编程错误**——这里 fail-closed
    /// panic，而不是像历史上各处那样静默回退 `Block(0)`：静默回退会把
    /// "根本没设入口"伪装成"入口是 0 号块"，支配树/循环分析/CFG 遍历会据此
    /// 算出看似合理但错误的结果（v3 方案 S4 前置清理，2026-09-14）。
    ///
    /// 需要"可能没有入口"语义的调用方请直接读 `entry_block` 字段。
    pub fn entry(&self) -> Block {
        self.entry_block
            .unwrap_or_else(|| panic!("Function {} 未设置 entry_block（入口块）", self.name))
    }

    /// 惰性获取循环森林（依赖支配树；快照，语义见 [`Function::dominator_tree`]）。
    pub fn loop_forest(&self) -> Arc<LoopForest> {
        let rev = self.analysis_revision();
        self.analysis.loop_forest(rev, || {
            let dt = self.dominator_tree();
            LoopForest::build(self, &dt)
        })
    }

    // ============================================================
    // 原子 IR 修改原语（保持 use_lists 新鲜）
    // ============================================================

    /// 设置块终结符并**同步 use-lists**（低层：编码成终结符指令）。
    ///
    /// **crate 内部低层入口**：调用方一律走下面按形式命名的写入口
    /// （`jump`/`branch`/`ret`/…），它们负责拼好 operands/immediates。
    ///
    /// **CFG 变了 ⇒ 惰性分析缓存自动失效**（v3 S6：`invalidate_analysis`）：
    /// 此前只有 `forge-opt` 的 8 处 pass 自己记得 `invalidate()`，任何走公开写
    /// 入口改控制流的调用方读到旧 CFG/支配树都是静默错误。
    pub(crate) fn set_terminator(
        &mut self,
        block: Block,
        opcode: Opcode,
        operands: SmallVec<[Value; 4]>,
        immediates: SmallVec<[crate::Immediate; 4]>,
    ) {
        self.invalidate_analysis();
        // 按**旧**终结符指令的操作数精确摘除 use 项（含结果值防御性清理）
        if let Some(old) = self.dfg.block_terminator(block) {
            self.use_lists.remove_inst(&self.dfg, old);
            for &r in self.dfg.inst_results(old) {
                self.use_lists.remove_value(r);
            }
        }
        self.dfg
            .set_terminator(block, opcode, operands.clone(), immediates);
        let inst = self.dfg.block_terminator(block).expect("刚写入终结符指令");
        self.use_lists.record_inst(inst, &operands);
    }

    /// 控制流被改写后使惰性分析缓存失效（CFG/支配树/循环森林）。
    ///
    /// **优化而非正确性前提**（v3 S6）：缓存槽按 [`AnalysisRevision`] 自校验，
    /// 结构修订号（[`DataFlowGraph::cfg_revision`]）在块增删 / 终结符写入 /
    /// 终结符墓碑化时自动前进 ⇒ 忘记调用这里只会多算一次，不会读到旧图。
    /// 提前释放槽位能省下"改图后没人再读"时的无用重算。
    ///
    /// 幂等、O(1)。改控制流的写入口内部会调它（少一次重算）。
    pub fn invalidate_analysis(&mut self) {
        self.analysis.invalidate();
    }

    /// 终结符被就地改写操作数后重登记其 use 项。
    pub(crate) fn refresh_terminator_inst_uses(&mut self, block: Block) -> usize {
        let Some(inst) = self.dfg.block_terminator(block) else {
            return 0;
        };
        self.refresh_inst_uses(inst)
    }

    // ============================================================
    // 终结符按形式的写入口
    // ============================================================
    //
    // 终结符是一条指令：这里负责把每种形式**编码**成
    // (opcode, operands, immediates)，编码约定与 `DataFlowGraph` 的解码访问器
    // 一一对应（另见 `ops.toml` 的「终结符」节）。
    // 所有写入口统一走 `set_terminator` ⇒ use-def 始终新鲜。

    /// 无条件跳转：`block: jump target(args)`。
    pub fn jump(&mut self, block: Block, target: Block, args: impl AsRef<[Value]>) {
        let operands: SmallVec<[Value; 4]> = args.as_ref().iter().copied().collect();
        let immediates = smallvec::smallvec![Immediate::Block(target)];
        self.set_terminator(block, Opcode::Jmp, operands, immediates);
    }

    /// 条件分支：`block: br cond, then(then_args), else(else_args)`。
    pub fn branch(
        &mut self,
        block: Block,
        cond: Value,
        then_block: Block,
        then_args: impl AsRef<[Value]>,
        else_block: Block,
        else_args: impl AsRef<[Value]>,
    ) {
        let then_args = then_args.as_ref();
        let else_args = else_args.as_ref();
        let mut operands: SmallVec<[Value; 4]> =
            SmallVec::with_capacity(1 + then_args.len() + else_args.len());
        operands.push(cond);
        operands.extend(then_args.iter().copied());
        operands.extend(else_args.iter().copied());
        let immediates = smallvec::smallvec![
            Immediate::Block(then_block),
            Immediate::Block(else_block),
            Immediate::Uint(then_args.len() as u64),
            Immediate::Uint(else_args.len() as u64),
        ];
        self.set_terminator(block, Opcode::Br, operands, immediates);
    }

    /// 返回：`block: ret values`。
    pub fn ret(&mut self, block: Block, values: impl AsRef<[Value]>) {
        let operands: SmallVec<[Value; 4]> = values.as_ref().iter().copied().collect();
        self.set_terminator(block, Opcode::Ret, operands, SmallVec::new());
    }

    /// 改写返回实参（聚合展开用；保留 metadata）。
    pub fn set_return_values(&mut self, block: Block, values: impl AsRef<[Value]>) {
        let Some(inst) = self.dfg.block_terminator(block) else {
            return;
        };
        if self.dfg.term_kind(block) != Some(TermKind::Return) {
            return;
        }
        self.dfg.inst_mut(inst).operands = values.as_ref().iter().copied().collect();
        self.refresh_terminator_inst_uses(block);
    }

    /// 多路分支：`block: switch discriminant, default(default_args)[cases…]`。
    pub fn switch(
        &mut self,
        block: Block,
        discriminant: Value,
        default_block: Block,
        default_args: impl AsRef<[Value]>,
        cases: &[(i64, Block, &[Value])],
    ) {
        let default_args = default_args.as_ref();
        let total: usize =
            default_args.len() + cases.iter().map(|(_, _, a)| a.len()).sum::<usize>();
        let mut operands: SmallVec<[Value; 4]> = SmallVec::with_capacity(1 + total);
        operands.push(discriminant);
        operands.extend(default_args.iter().copied());
        let mut immediates: SmallVec<[Immediate; 4]> = SmallVec::with_capacity(3 + cases.len() * 3);
        immediates.push(Immediate::Block(default_block));
        immediates.push(Immediate::Uint(default_args.len() as u64));
        immediates.push(Immediate::Uint(cases.len() as u64));
        for (val, target, args) in cases {
            operands.extend(args.iter().copied());
            immediates.push(Immediate::Int(*val));
            immediates.push(Immediate::Block(*target));
            immediates.push(Immediate::Uint(args.len() as u64));
        }
        self.set_terminator(block, Opcode::Switch, operands, immediates);
    }

    /// 显式不可达：`block: unreachable`。
    pub fn unreachable(&mut self, block: Block) {
        self.set_terminator(block, Opcode::Unreachable, SmallVec::new(), SmallVec::new());
    }

    /// invoke：`block: invoke callee(args) to normal(normal_args) unwind unwind(unwind_args)`。
    #[allow(clippy::too_many_arguments)]
    pub fn invoke(
        &mut self,
        block: Block,
        callee: FuncRef,
        args: impl AsRef<[Value]>,
        ret_ty: TypeId,
        normal_block: Block,
        normal_args: impl AsRef<[Value]>,
        unwind_block: Block,
        unwind_args: impl AsRef<[Value]>,
    ) {
        let args = args.as_ref();
        let normal_args = normal_args.as_ref();
        let unwind_args = unwind_args.as_ref();
        let mut operands: SmallVec<[Value; 4]> =
            SmallVec::with_capacity(args.len() + normal_args.len() + unwind_args.len());
        operands.extend(args.iter().copied());
        operands.extend(normal_args.iter().copied());
        operands.extend(unwind_args.iter().copied());
        let immediates = smallvec::smallvec![
            Immediate::Func(callee),
            Immediate::Block(normal_block),
            Immediate::Block(unwind_block),
            Immediate::Uint(args.len() as u64),
            Immediate::Uint(normal_args.len() as u64),
            Immediate::Uint(unwind_args.len() as u64),
            Immediate::Type(ret_ty),
        ];
        self.set_terminator(block, Opcode::Invoke, operands, immediates);
    }

    /// resume：`block: resume value`。
    pub fn resume(&mut self, block: Block, value: Value) {
        let operands = smallvec::smallvec![value];
        self.set_terminator(block, Opcode::Resume, operands, SmallVec::new());
    }

    /// 把 `block` 终结符中指向 `old_target` 的目标改为 `new_target`（实参原样保留）。
    pub fn retarget_terminator(&mut self, block: Block, old_target: Block, new_target: Block) {
        self.invalidate_analysis();
        let Some(inst) = self.dfg.block_terminator(block) else {
            return;
        };
        let kind = self.dfg.term_kind(block);
        let imms = &mut self.dfg.inst_mut(inst).immediates;
        // case 表：immediates[3..] 每 3 个一组 (Int, Block, Uint)
        let case_count = match imms.get(2) {
            Some(Immediate::Uint(n)) => *n as usize,
            _ => 0,
        };
        let positions: &[usize] = match kind {
            Some(TermKind::Jump) => &[0],
            Some(TermKind::Branch) => &[0, 1],
            Some(TermKind::Switch) => &[0],
            Some(TermKind::Invoke) => &[1, 2],
            _ => &[],
        };
        for &pos in positions {
            if let Some(Immediate::Block(b)) = imms.get_mut(pos)
                && *b == old_target
            {
                *b = new_target;
            }
        }
        if kind == Some(TermKind::Switch) {
            for c in 0..case_count {
                let pos = 3 + c * 3 + 1;
                if let Some(Immediate::Block(b)) = imms.get_mut(pos)
                    && *b == old_target
                {
                    *b = new_target;
                }
            }
        }
        // 只改了块编号、不动用值：use 项无需变化。
    }

    /// 把 `block` 传给 `target` 的实参整体替换为 `args`（不跳向该块则无操作）。
    pub fn replace_terminator_args(
        &mut self,
        block: Block,
        target: Block,
        args: impl AsRef<[Value]>,
    ) {
        let new_args = args.as_ref();
        match self.dfg.term_kind(block) {
            Some(TermKind::Jump) => {
                if let Some((t, _)) = self.dfg.term_jump(block)
                    && t == target
                {
                    self.jump(block, t, new_args);
                }
            }
            Some(TermKind::Branch) => {
                let Some((cond, t, targs, e, eargs)) = self.dfg.term_branch(block) else {
                    return;
                };
                if t == target {
                    let eargs = eargs.to_vec();
                    self.branch(block, cond, t, new_args, e, &eargs);
                } else if e == target {
                    let targs = targs.to_vec();
                    self.branch(block, cond, t, &targs, e, new_args);
                }
            }
            Some(TermKind::Switch) => {
                let Some(view) = self.dfg.term_switch(block) else {
                    return;
                };
                let disc = view.discriminant;
                let default_block = view.default_block;
                let default_args: Vec<Value> = if default_block == target {
                    new_args.to_vec()
                } else {
                    view.default_args.to_vec()
                };
                let mut cases: Vec<(i64, Block, Vec<Value>)> = view
                    .cases
                    .iter()
                    .map(|c| {
                        let a = if c.target == target {
                            new_args.to_vec()
                        } else {
                            c.args.to_vec()
                        };
                        (c.value, c.target, a)
                    })
                    .collect();
                let case_refs: Vec<(i64, Block, &[Value])> = cases
                    .iter_mut()
                    .map(|(v, b, a)| (*v, *b, a.as_slice()))
                    .collect();
                self.switch(block, disc, default_block, &default_args, &case_refs);
            }
            Some(TermKind::Invoke) => {
                let Some((callee, iargs, ret_ty, n, nargs, u, uargs)) = self.dfg.term_invoke(block)
                else {
                    return;
                };
                let iargs = iargs.to_vec();
                let nargs: Vec<Value> = if n == target {
                    new_args.to_vec()
                } else {
                    nargs.to_vec()
                };
                let uargs: Vec<Value> = if u == target {
                    new_args.to_vec()
                } else {
                    uargs.to_vec()
                };
                self.invoke(block, callee, &iargs, ret_ty, n, &nargs, u, &uargs);
            }
            _ => {}
        }
    }

    /// 重登记该块终结符指令的 use 项（就地改写实参后用）。
    pub fn refresh_terminator_uses(&mut self, block: Block) -> usize {
        self.refresh_terminator_inst_uses(block)
    }

    /// RAUW：把 `old` 的所有使用替换为 `new`，同步更新 DFG 操作数与 use-lists。
    /// 返回被替换的使用数量。
    ///
    /// 终结符就是指令，所以"所有使用"天然包括分支/跳转实参、`switch` 判别值与
    /// case 实参、`ret` 返回值、`invoke`/`resume` 用值（S4 主体之前它们不在
    /// use-lists 里，这里改不到——该缺口已随表示归一消失）。
    /// 需要一次替换多组值时用 [`Function::apply_replacements`]。
    pub fn replace_all_uses(&mut self, old: Value, new: Value) -> usize {
        let uses: Vec<(Inst, u32)> = self
            .use_lists
            .uses(old)
            .iter()
            .map(|u| (u.user, u.operand_idx))
            .collect();
        let count = uses.len();
        for (user, operand_idx) in uses {
            if let Some(inst) = self.dfg.inst_mut_opt(user)
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
        // 删的可能正是终结符指令（CFG 随之变化）⇒ 保守失效分析缓存
        self.invalidate_analysis();
        self.use_lists.remove_inst(&self.dfg, inst);
        for &r in self.dfg.inst_results(inst) {
            self.use_lists.remove_value(r);
        }
        self.dfg.remove_inst(inst);
    }

    /// **就地墓碑化**：标墓碑 + 清空指令内容 + 结果值 VOID 化 + use-lists 重登记，
    /// 但**保留块内顺序表条目**（与 [`Function::kill_inst`] 的区别）。
    ///
    /// 用于"指令槽位留着、内容作废"的重写场景（大聚合展开、`__cp`/`__tomb` 占位
    /// 替换）：占位条目仍在 `inst_order` 里，遍历方靠
    /// [`Instruction::is_tombstone`] 跳过。此前这些调用点各自手写
    /// "`opcode = Nop` + 清三张表 + `refresh_inst_uses`"，附件（metadata/
    /// isel_strategy/param_attrs）留在墓碑上（S2 实测不一致）。
    pub fn tombstone_inst(&mut self, inst: Inst) {
        self.dfg.tombstone_inst_low(inst);
        self.refresh_inst_uses(inst);
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

    // ============================================================
    // 指令创建（保持 use_lists 新鲜）
    // ============================================================

    /// 创建指令并**登记 use-lists**。
    ///
    /// pass / 前端构造 IR 请统一走这里，**不要**直接 `dfg.make_inst`：直接创建会留下
    /// use-def 空洞，而 `Verifier` 的 `UseListInconsistency` 只在 pass 之后才发现；
    /// 期间 DCE / RAUW / GVN 都会基于不完整的 use-list 做出错误判断
    /// （2026-09-14 实测：`gvn_pre`/`pgo`/`inline` 等直接建指令且不登记，见
    /// `docs/plans/forge-ir-v3-plan.md` S6）。
    pub fn make_inst(
        &mut self,
        opcode: Opcode,
        block: Block,
        operands: SmallVec<[Value; 4]>,
        immediates: SmallVec<[super::immediate::Immediate; 4]>,
        result_tys: &[TypeId],
        flags: super::inst_flags::InstFlags,
    ) -> Inst {
        let inst = self.dfg.make_inst(
            opcode,
            block,
            operands.clone(),
            immediates,
            result_tys,
            flags,
        );
        self.use_lists.record_inst(inst, &operands);
        inst
    }

    /// [`Function::make_inst`] 的 mem_flags / metadata / 源码位置版。
    #[allow(clippy::too_many_arguments)]
    pub fn make_inst_with_meta_and_loc(
        &mut self,
        opcode: Opcode,
        block: Block,
        operands: SmallVec<[Value; 4]>,
        immediates: SmallVec<[super::immediate::Immediate; 4]>,
        result_tys: &[TypeId],
        flags: super::inst_flags::InstFlags,
        mem_flags: super::mem_flags::MemFlags,
        metadata: SmallVec<[AttachedMetadata; 2]>,
        loc: Option<super::debug_info::SourceLocation>,
    ) -> Inst {
        let inst = self.dfg.make_inst_with_meta_and_loc(
            opcode,
            block,
            operands.clone(),
            immediates,
            result_tys,
            flags,
            mem_flags,
            metadata,
            loc,
        );
        self.use_lists.record_inst(inst, &operands);
        inst
    }

    /// 就地改写某条指令的操作数后**重登记** use-lists。
    ///
    /// 先按 `user == inst` 忘掉该指令的全部旧记录（`forget_inst`），再登记当前
    /// 操作数——**必须在改写之后调用**（改写前用 `UseLists::remove_inst` 亦可，
    /// 但那要求调用方记住顺序；本方法对顺序不敏感）。返回重登记的操作数个数。
    pub fn refresh_inst_uses(&mut self, inst: Inst) -> usize {
        self.use_lists.forget_inst(inst);
        let operands = self.dfg.inst_data(inst).operands.clone();
        self.use_lists.record_inst(inst, &operands);
        operands.len()
    }

    /// 把 `block` 终结符中指向 `old_target` 的所有跳转目标改为 `new_target`
    /// （参数原样保留；不改变其它块的终结符）。
    /// 删除块的第 `idx` 个参数，并同步清理**所有前驱终结符**传给该位置的
    /// 参数（替代 block_param_coalesce 的手工逐前驱删除）。
    /// 被删除参数值的其它使用（非终结符）由调用方先 RAUW。
    pub fn remove_block_param(&mut self, block: Block, idx: usize) {
        // 1. 先收集前驱（不可变借用结束），再改终结符
        let preds: Vec<Block> = self.predecessors().get(block).cloned().unwrap_or_default();
        // 2. 清理前驱终结符传给该块的实参（删参数会移动后续槽位下标）
        for pred in preds {
            self.remove_terminator_arg(pred, block, idx);
        }
        // 3. 从块参数表删除
        let bd = self.dfg.block_mut(block);
        if idx < bd.params.len() {
            bd.params.remove(idx);
        }
        if idx < bd.param_values.len() {
            bd.param_values.remove(idx);
        }
    }

    /// 从 `block` 终结符传给 `target` 的实参里删掉第 `idx` 个（并同步 arg-count）。
    ///
    /// 终结符是编码成指令的，删一个实参会移动后续下标 ⇒ 与其就地 splice，
    /// 不如**解码 → 改 → 按形式写回**：写入口顺带保证 use-def 新鲜。
    pub(crate) fn remove_terminator_arg(&mut self, block: Block, target: Block, idx: usize) {
        match self.dfg.term_kind(block) {
            Some(TermKind::Jump) => {
                if let Some((t, args)) = self.dfg.term_jump(block)
                    && t == target
                    && idx < args.len()
                {
                    let mut new_args = args.to_vec();
                    new_args.remove(idx);
                    self.jump(block, t, &new_args);
                }
            }
            Some(TermKind::Branch) => {
                let Some((cond, t, targs, e, eargs)) = self.dfg.term_branch(block) else {
                    return;
                };
                let mut new_targs = targs.to_vec();
                let mut new_eargs = eargs.to_vec();
                if t == target && idx < new_targs.len() {
                    new_targs.remove(idx);
                } else if e == target && idx < new_eargs.len() {
                    new_eargs.remove(idx);
                } else {
                    return;
                }
                self.branch(block, cond, t, &new_targs, e, &new_eargs);
            }
            Some(TermKind::Switch) => {
                let Some(view) = self.dfg.term_switch(block) else {
                    return;
                };
                let disc = view.discriminant;
                let default_block = view.default_block;
                let mut default_args = view.default_args.to_vec();
                if default_block == target && idx < default_args.len() {
                    default_args.remove(idx);
                }
                let mut cases: Vec<(i64, Block, Vec<Value>)> = view
                    .cases
                    .iter()
                    .map(|c| {
                        let mut a = c.args.to_vec();
                        if c.target == target && idx < a.len() {
                            a.remove(idx);
                        }
                        (c.value, c.target, a)
                    })
                    .collect();
                let case_refs: Vec<(i64, Block, &[Value])> = cases
                    .iter_mut()
                    .map(|(v, b, a)| (*v, *b, a.as_slice()))
                    .collect();
                self.switch(block, disc, default_block, &default_args, &case_refs);
            }
            Some(TermKind::Invoke) => {
                let Some((callee, iargs, ret_ty, n, nargs, u, uargs)) = self.dfg.term_invoke(block)
                else {
                    return;
                };
                let iargs = iargs.to_vec();
                let mut nargs = nargs.to_vec();
                let mut uargs = uargs.to_vec();
                if n == target && idx < nargs.len() {
                    nargs.remove(idx);
                } else if u == target && idx < uargs.len() {
                    uargs.remove(idx);
                } else {
                    return;
                }
                self.invoke(block, callee, &iargs, ret_ty, n, &nargs, u, &uargs);
            }
            _ => {}
        }
    }

    /// 批量替换：把 `replacements`（old → new）一次性应用到所有指令操作数
    /// （**终结符就是指令，天然包含在内**），**同步维护 use-lists**
    /// （单趟语义：不追溯链式映射——与 pass 现有批量替换行为一致）。
    /// 返回被替换的操作数总数。
    pub fn apply_replacements(&mut self, replacements: &HashMap<Value, Value>) -> usize {
        let mut count = 0;
        // 指令操作数（含终结符指令）+ use-lists 同步
        //（dfg 与 use_lists 为不相交字段，可同时可变借用）
        for (inst_id, inst) in self.dfg.insts_iter_mut() {
            if matches!(inst.opcode, Opcode::Nop) {
                continue;
            }
            for (operand_idx, operand) in inst.operands.iter_mut().enumerate() {
                if let Some(&replacement) = replacements.get(operand) {
                    let old = *operand;
                    *operand = replacement;
                    self.use_lists
                        .replace_operand(old, replacement, inst_id, operand_idx as u32);
                    count += 1;
                }
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
        let block_count = self.dfg.block_count();
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
    ///
    /// **单写**（v3 方案 S5）：读 [`GlobalVariable::metadata`]、写
    /// [`GlobalVariable::attach_metadata`]。
    pub(crate) metadata: Vec<crate::metadata::AttachedMetadata>,
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
    /// 别名尾 metadata 附加（**单写**：读 [`GlobalAlias::metadata`]、写
    /// [`GlobalAlias::attach_metadata`]）。
    pub(crate) metadata: Vec<crate::metadata::AttachedMetadata>,
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

    /// 全局尾 metadata（`(kind, node)` 对，按附加序）。
    pub fn metadata(&self) -> &[crate::metadata::AttachedMetadata] {
        &self.metadata
    }

    /// 附加一条全局尾 metadata —— **全局变量附件的唯一写入口**（S5：metadata 单写）。
    pub fn attach_metadata(&mut self, metadata: crate::metadata::AttachedMetadata) {
        self.metadata.push(metadata);
    }
}

impl GlobalAlias {
    /// 别名尾 metadata（`(kind, node)` 对，按附加序）。
    pub fn metadata(&self) -> &[crate::metadata::AttachedMetadata] {
        &self.metadata
    }

    /// 附加一条别名尾 metadata —— **别名附件的唯一写入口**（S5：metadata 单写）。
    pub fn attach_metadata(&mut self, metadata: crate::metadata::AttachedMetadata) {
        self.metadata.push(metadata);
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
    ///
    /// 开放数据用 [`ImmStr`]（SSO + `Arc<str>` 共享，Clone O(1)）——IR 层的
    /// 字符串字段不出现裸 `String`（v3 方案 S5 第 2 项：开放集合划边界）。
    pub source_filename: Option<ImmStr>,

    /// `module asm "..."`（模块级内联汇编；可多条，按序输出）。
    pub module_asm: Vec<ImmStr>,
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

    /// 按 FuncRef 替换函数体（保留名称/索引——预注册占位后填真实体用；
    /// 递归函数编译期需要自己的 FuncRef 生成自调用 Call）。
    pub fn replace_function(&mut self, fr: FuncRef, func: Function) {
        self.functions[fr.0 as usize] = func;
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
            fb.func.dfg.inst_data(*y_inst).operands.contains(&c),
            "DFG 侧 operands 已更新为 c"
        );
        assert!(
            !fb.func.dfg.inst_data(*y_inst).operands.contains(&x),
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
            let inst = fb.func.dfg.inst_mut(x_inst);
            inst.flags = crate::inst_flags::InstFlags::MAY_UB;
            inst.set_isel_strategy(crate::IselStrategy::from_static("lea_sib:4"));
            inst.loc = Some(crate::debug_info::SourceLocation {
                file: Some(crate::imm_str::ImmStr::from("test.rs")),
                line: Some(42),
                column: Some(7),
            });
        }

        let mut remap: HashMap<Value, Value> = HashMap::new();
        let new_inst = fb.func.dfg.clone_inst(x_inst, target, &mut remap);

        let orig = &fb.func.dfg.inst_data(x_inst);
        let cloned = &fb.func.dfg.inst_data(new_inst);
        assert_eq!(cloned.opcode, orig.opcode, "opcode");
        assert_eq!(cloned.operands, orig.operands, "operands（无映射时保持）");
        assert_eq!(cloned.immediates, orig.immediates, "immediates");
        assert_eq!(cloned.flags, orig.flags, "flags 保留");
        assert_eq!(cloned.mem_flags, orig.mem_flags, "mem_flags 保留");
        assert_eq!(cloned.metadata, orig.metadata, "metadata 保留");
        assert_eq!(cloned.loc, orig.loc, "loc 保留");
        assert_eq!(
            cloned.isel_strategy(),
            orig.isel_strategy(),
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
