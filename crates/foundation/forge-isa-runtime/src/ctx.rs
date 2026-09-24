//! 生成物 lowering 上下文与内存引用（v19 V1 由 forge-codegen 迁入）。

use forge_ir::entity::map::SecondaryMap;
use forge_ir::*;
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};

/// 内存引用 —— DSL `MemRef` 字段的 Rust 表示（基址 + 偏移 + 数据宽度）。
///
/// 供指令字段携带"内存操作数"（如 load/store 的地址）使用：lowering
/// 阶段从 `StackAddr` / `Iconst` 基址解析填充，编码阶段读取 `base`/`offset`
/// 发射地址。`width` 表示被访问数据的字节宽度。
///
/// 这是架构无关的通用结构（不引用任何 ISA 寄存器名），`base` 为物理
/// 寄存器编号（PReg index），由各 ISA 的 lowering 负责映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemRef {
    /// 基址物理寄存器编号（PReg index；未解析时为 [`MemRef::UNRESOLVED_BASE`]）。
    pub base: u32,
    /// 字节偏移（可为负）。
    pub offset: i32,
    /// 数据字节宽度（1/2/4/8/16）。
    pub width: u8,
}

impl MemRef {
    /// 未解析基址的哨兵值。
    pub const UNRESOLVED_BASE: u32 = u32::MAX;

    /// 构造已解析的内存引用。
    pub fn new(base: u32, offset: i32, width: u8) -> Self {
        Self {
            base,
            offset,
            width,
        }
    }

    /// 未解析占位（供 DSL 默认字段值 / 汇编解析未定基址使用）。
    pub fn unresolved(width: u8) -> Self {
        Self {
            base: Self::UNRESOLVED_BASE,
            offset: 0,
            width,
        }
    }

    /// 是否已解析出基址寄存器。
    pub fn is_resolved(&self) -> bool {
        self.base != Self::UNRESOLVED_BASE
    }
}

impl Default for MemRef {
    fn default() -> Self {
        Self::unresolved(8)
    }
}

/// 指令选择上下文 — 在 lowering 阶段提供给 [`machine::lowering::TargetLowering`]。
pub struct LowerCtx {
    /// 下一个可用的虚拟寄存器号。
    pub next_vreg: u32,
    /// 临时寄存器（XReg）分配器 — XReg 的唯一受控创建入口。
    pub xregs: forge_ir::XRegAllocator,
    /// XReg → IR 类型映射（用于 `.if` 条件汇编中的类型查询）。
    pub xreg_types: HashMap<XReg, TypeId>,
    /// 当前函数的调用约定（IR 声明的那份标识）。
    pub call_conv: CallConvId,
    /// **解析后的约定名**（宿主注册表里的键；空串 = 还没解析）。
    ///
    /// 与 [`call_conv`](Self::call_conv) 的分工：`call_conv` 是 IR 侧的**标识**
    /// （`Builtin`/`Named`/`Index`），这个名字是**查表用的键**——A3 起用它查
    /// `AbiRules`/`AbiBinding` 发射调用点/入口/序尾声。管线入口
    /// （`CompileState::new`）会解析并 fail-closed，因此 lowering 里读到的名字
    /// 一定是注册过的。
    pub call_conv_name: String,
    /// 当前函数的常量池（用于解析 Iconst/Fconst 的索引）。
    pub constant_pool: Option<forge_ir::ConstantPool>,
    /// VReg → 寄存器类别映射（用于寄存器分配）。
    ///
    /// **密集表**（v3 S2）：`VReg` 是 forge-ir 的密集句柄（已实现 `EntityRef`），
    /// 下标即句柄——不必按句柄哈希。
    pub vreg_classes: SecondaryMap<VReg, RegClass>,
    /// VReg → IR 类型映射（用于 `.if` 条件汇编中的类型查询）。
    pub vreg_types: SecondaryMap<VReg, TypeId>,
    /// VReg → 字节宽度（用于寄存器分配器宽度感知）。
    pub vreg_widths: SecondaryMap<VReg, u8>,
    /// 是否为浮点返回值（影响 Return 降低时使用 RetVal 还是 RetValFloat）。
    pub is_float_return: bool,
    /// 是否 sret 返回（函数返回宽向量 >16 字节——隐藏 sret 指针参数占首
    /// int 槽，move_args 收参从第 2 个 int 槽起；调用方 sret 约定）。
    pub is_sret_return: bool,
    /// 当前指令的默认操作数宽度 (8/16/32/64)，由 IR 类型推导。
    pub default_opsize: u8,
    /// 当前指令的常量索引 (从 Instruction.immediates 中提取)。
    pub current_const_index: u32,
    /// 当前指令的全部 immediates 值（Uint/Int/Const 提取后的 u64 列表；
    /// ShuffleVector 的 mask、Vextract/Vinsert 的 index 等使用）。
    /// SmallVec<[u64; 4]>：大多数指令 immediates ≤ 4 个，免每指令堆分配
    /// （lowering 主循环每指令重建——第二轮基准 CF_CODEGEN_TIMING 显示
    /// lowering 占 codegen 33%，分配是主要开销之一）。
    pub current_immediates: SmallVec<[u64; 4]>,
    /// 当前指令引用的函数 (Call 的 Immediate::Func；供 lowering 生成
    /// cross-function relocation 的符号名 "@N")。
    pub current_func_ref: Option<FuncRef>,
    /// 当前 AtomicRmw 的操作数 (Immediate::Uint(op as u64) 解析)。
    pub current_atomic_op: Option<forge_ir::ir::opcode::AtomicRmwOp>,
    /// 当前指令引用的全局变量 (GlobalAddr 的 Immediate::Global)。
    pub current_global: Option<GlobalId>,
    /// StackAddr 的帧偏移（Immediate::Int）。
    pub current_offset: i64,
    /// 当前 Alloca 的帧槽偏移（预扫描分配；`lea_off rd, alloca_offset` 规则取用）。
    pub current_alloca_offset: i64,
    /// prologue 压入的 callee-saved 寄存器总字节数（局部变量 lea 的基准平移）。
    pub callee_saved_bytes: i32,
    /// 栈槽（StackAddr/Alloca）的帧顶平移字节数。x86 = callee_saved_bytes
    ///（槽在 rbp - callee_saved 之下）；riscv = fp_push_bytes（16，槽在
    /// fp - 16 之下，避开 ra/fp 保存槽）。独立于 callee_saved_bytes——
    /// riscv 的 spill 布局需 callee_saved_bytes=0（spill 槽帧内底部）而
    /// 栈槽平移需 16（fp 基准）。
    pub stack_slot_shift: i32,
    /// StackAddr 局部变量区需求（从 callee-saved 区底向下到最深槽的字节数）。
    /// calculate_frame_size 必须把它算进 sub rsp 的帧大小，否则局部槽落在
    /// rsp 之下（Windows 无 red zone）→ 写栈越界 SEGV（mini_c 参数内联场景）。
    pub max_stack_bytes: u32,
    /// 栈参数区需求（shadow space + 第 5+ 参数槽字节数）：调用方在 call 前
    /// 把超寄存器参数 store 到 [rsp+shadow+off]，帧底之上必须预留该区域
    ///（frame_size 并入；否则写穿 rsp 之下 → SEGV）。缺省 0（无栈参数）。
    pub max_stack_arg_bytes: u32,
    /// 临时 VReg 集合（替代 VReg(96-100) 硬编码）。
    pub temp_vregs: HashSet<VReg>,
    /// 零值 VReg（复用，避免重复分配）。
    zero_vreg: Option<VReg>,
    /// 零值 XReg（复用，避免重复分配）。
    zero_xreg: Option<XReg>,
    /// 当前 lowering 包的写死物理寄存器（来自 insts 自动推导）——
    /// 生成代码在规则 arm 尾部设置，编译器聚合到 inst_clobbers 供分配器避开。
    /// 值 = (物理寄存器编号, 寄存器类)。
    pub current_clobbers: Vec<(u32, RegClass)>,
    /// 当前函数的类型**快照**（v3 S3 余项：lowering 入口取一次）。
    ///
    /// lowering 期间类型表**只读**（新类型都在 IR 构建期 intern 完），所以读路径
    /// 全部走这一份 `TypeStoreRef`——不再每条指令/每个属性各取一次锁。
    /// 由调用方在 lowering 入口用 `func.types.borrow()` 取（见
    /// `pipeline/compiler.rs`）。
    pub type_store: Option<forge_ir::ir::types::TypeStoreRef>,
    /// 宿主「整数值寄存器池」类（`TargetRegInfo::value_gpr_class`）。
    /// lowering 里不能用 `RegClass::GPR64` 字面量——1 字节寄存器 ISA 的值池
    /// 是 GPR(1)。缺省 GPR64（`LowerCtx::new()` 的测试/直连场景）；
    /// `CompileState::new` 会用机器元数据覆盖。
    pub value_gpr_class: RegClass,
    /// 宿主「浮点值寄存器池」类（同 `value_gpr_class`；缺省 FPR64 = f64 值池）。
    pub value_fpr_class: RegClass,
    /// 地址/指针类（`TargetRegInfo::addr_class`；缺省 GPR64）。
    pub addr_class: RegClass,
    /// ABI 栈槽单位（字节，`TargetRegInfo::slot_bytes`；缺省 8）。
    pub slot_bytes: u16,
    /// 向量类字节档位（`TargetRegInfo::vector_tiers`；缺省 `[16,32,64]`）。
    pub vector_tiers: Vec<u16>,
    /// ISA 的 `[types]` 显式类型→类映射（`TargetRegInfo::type_map`）。
    /// lowering 必须与编译入口的门用同一份数据（B2 接口通用化）。
    pub type_map: Vec<(TypeId, RegClass)>,
}

impl LowerCtx {
    pub fn new() -> Self {
        Self {
            next_vreg: 0,
            xregs: forge_ir::XRegAllocator::new(),
            xreg_types: HashMap::new(),
            call_conv: CallConvId::default(),
            call_conv_name: String::new(),
            constant_pool: None,
            is_float_return: false,
            is_sret_return: false,
            current_const_index: 0,
            current_immediates: SmallVec::new(),
            current_func_ref: None,
            current_atomic_op: None,
            current_global: None,
            current_offset: 0,
            current_alloca_offset: 0,
            callee_saved_bytes: 0,
            stack_slot_shift: 0,
            max_stack_bytes: 0,
            max_stack_arg_bytes: 0,
            vreg_classes: SecondaryMap::new(),
            vreg_types: SecondaryMap::new(),
            vreg_widths: SecondaryMap::new(),
            default_opsize: 64,
            temp_vregs: HashSet::new(),
            zero_vreg: None,
            zero_xreg: None,
            current_clobbers: Vec::new(),
            value_gpr_class: RegClass::GPR64,
            value_fpr_class: RegClass::FPR64,
            addr_class: RegClass::GPR64,
            slot_bytes: 8,
            vector_tiers: vec![16, 32, 64],
            type_map: Vec::new(),
            type_store: None,
        }
    }

    /// 分配一个新的临时寄存器（XReg），类型由 `class` 严格限定。
    /// 被动语义：XReg 只能作为后续指令的 def（结果）被赋予值。
    pub fn alloc_xreg(&mut self, class: RegClass) -> XReg {
        self.xregs.alloc_default(class)
    }

    /// 位宽感知的寄存器类推导：动态 vector/scalable 类型按字节位宽映射到
    /// **向量档位**（`TargetRegInfo::vector_tiers`，元数据驱动；缺省
    /// `[16, 32, 64]` = x86 XMM/YMM/ZMM），其余回退 `RegClass::from_type_id`。
    ///
    /// **>256 位必须是 VEC(64)**（WA-46）：类的 `reg_width` 决定 spill 槽大小与
    /// spill 搬运宽度，旧实现把 64 字节向量（V512）也归到 VEC(32) ⇒ 槽只有
    /// 32 字节、搬运按 32 字节 → 高半区静默截断（CI 上 V512 by-ref 用例偶发
    /// lane15 错）。
    pub fn reg_class_for(&self, ty: &TypeId) -> RegClass {
        // ① ISA 的 `[types]` 显式映射（B2）：与编译入口的值池门同一份数据。
        if let Some((_, rc)) = self.type_map.iter().find(|(t, _)| t == ty) {
            return *rc;
        }
        if let Some(store) = &self.type_store
            && (store.is_vector(*ty) || store.is_scalable_vector(*ty))
        {
            let bytes = store.size_bytes(*ty);
            // 最小的 ≥ 请求字节数的档位；超出最大档 → 最大档（宽度由类的
            // reg_width 承载，spill/ABI 按类宽工作）。
            let tier = self
                .vector_tiers
                .iter()
                .copied()
                .find(|t| (*t as u32) >= bytes)
                .or_else(|| self.vector_tiers.last().copied())
                // 未声明任何档位（手写后端/测试替身）：按**真实字节数**成类，
                // 不再回退 x86 的常量 64。
                .unwrap_or(bytes.max(1) as u16);
            return RegClass::VEC(tier.max(1));
        }
        RegClass::from_type_id(*ty)
    }

    /// 分配一个指定宽度（8/16/32/64）的临时寄存器。
    pub fn alloc_xreg_with_width(&mut self, class: RegClass) -> XReg {
        self.xregs.alloc(class)
    }

    /// 分配或复用零值临时寄存器（用于比较零值）。
    pub fn alloc_zero_xreg(&mut self) -> XReg {
        if let Some(x) = self.zero_xreg {
            return x;
        }
        let x = self.alloc_xreg(self.value_gpr_class);
        self.zero_xreg = Some(x);
        x
    }

    /// 查询 XReg 的字节宽度（位宽内嵌在值上）。
    pub fn xreg_width(&self, x: XReg) -> u16 {
        x.width()
    }

    /// 查询 VReg 对应的 IR 类型是否为浮点类型。
    pub fn is_float_vreg(&self, vreg: VReg) -> bool {
        self.vreg_types
            .get(vreg)
            .map(|t| self.reg_class_for(t).is_fp())
            .unwrap_or(false)
    }

    /// 从 TypeId 推导寄存器类（统一的集中处理）。
    pub fn reg_class_for_type(ty: TypeId) -> RegClass {
        RegClass::from_type_id(ty)
    }

    /// 从 IR 类型推导操作数宽度 (opsize)。
    ///
    /// 不枚举、不截断：1 字节 → 32（窄类型算术的寄存器安全折中，32 位写全
    /// 低 32 位避免残留高位参与 64 位运算）；2 → 16；4 → 32；u64 → 64；
    /// 自定义非常规宽度（如 12 字节 GPR）→ 真实字节数 ×8 原样传递——
    /// 是否可编码由目标 ISA 的指令定义决定，这里不静默截断成 64。
    ///
    /// ## 窄值高位约定（bool/i8/i16）
    /// 窄值（<32 位）在 64 位 GPR 中的高位由产生方保证清零：
    /// - `setcc` 类（icmp/fcmp/is_null/overflow flag）规则必须前置 `xor rd, rd`；
    /// - 32 位写（窄算术）在 x86/aarch64 硬件上自动零扩展高 32 位；
    /// - 消费方 `[lower.Uextend]` 按源宽度 movzx（x86）或 mov（32 位源）。
    ///   违反约定会在 uextend/64 位运算中携带高位垃圾（历史缺陷，已修复）。
    ///
    /// 位宽事实来源：`TypeContext::scalar_bits`（指针按 DataLayout）→ 内建标量表
    /// → 兜底 64（完全无信息时的寄存器安全默认）。
    fn type_bits_or_default(&self, ty: &TypeId) -> u32 {
        self.type_store
            .as_ref()
            .and_then(|store| store.scalar_bits(*ty))
            .or_else(|| ty.builtin_scalar_bits())
            .unwrap_or(64)
    }

    pub fn opsize_from_type(&self, ty: &TypeId) -> u8 {
        match self.type_bits_or_default(ty).div_ceil(8) {
            0..=1 => 32,
            2 => 16,
            4 => 32,
            n => (n as u8).saturating_mul(8),
        }
    }

    /// 内存访问（load/store）的精确宽度——真实字节数，不枚举不截断：
    /// u8 → 8、u16 → 16、u32 → 32、u64 → 64、自定义非常规宽度 → 原样传递。
    /// 与 `opsize_from_type` 不同（后者对 <4 字节返回 32——寄存器安全折中，
    /// 窄类型算术用 32 位避免残留高位参与 64 位运算）；内存访问必须用真实
    /// 宽度，否则 u8 元素 load 读 4 字节（越界读到相邻槽垃圾）、store 写
    /// 4 字节（越界覆盖相邻槽）。
    pub fn mem_opsize_from_type(&self, ty: &TypeId) -> u8 {
        (self.type_bits_or_default(ty).div_ceil(8) as u8).saturating_mul(8)
    }

    /// 内存访问（load/store）的精确宽度——聚合类型（struct/array）的
    /// 位宽视图为 0，`mem_opsize_from_type` 会错误返回 0（store 宽度 0 →
    /// 机器码缺陷/SEGV）；这里优先用 TypeStore::size_bytes（聚合 → 真实
    /// 字节数，如 {i32,i32} → 64 位），基础类型回退到 bits() 路径。
    pub fn mem_opsize_for(&self, ty: &TypeId) -> u8 {
        if let Some(store) = &self.type_store {
            let bytes = store.size_bytes(*ty);
            if bytes > 0 {
                return (bytes as u8).saturating_mul(8).min(64);
            }
        }
        self.mem_opsize_from_type(ty)
    }

    /// 值 → 标量位宽（生成的 lowering 用）：问类型快照（指针宽度按 DataLayout、
    /// 动态整数位宽也能答）；值未登记类型或非标量 → `None`。
    pub fn type_bits_of(&self, val: &XReg) -> Option<u32> {
        let ty = *self.xreg_types.get(val)?;
        self.type_store.as_ref().and_then(|s| s.scalar_bits(ty))
    }

    /// 分配一个新的整数类虚拟寄存器。
    pub fn alloc_vreg(&mut self) -> VReg {
        self.alloc_vreg_with_class(self.value_gpr_class)
    }

    /// 分配一个指定寄存器类别的虚拟寄存器。
    pub fn alloc_vreg_with_class(&mut self, class: RegClass) -> VReg {
        let vreg = VReg::new(self.next_vreg);
        self.next_vreg += 1;
        self.vreg_classes.insert(vreg, class);
        self.vreg_widths.insert(vreg, class.default_width());
        vreg
    }

    /// 分配一个临时 VReg（用于 lowering 中的中间值）。
    /// 替代硬编码的 VReg(96)/VReg(97)/VReg(100)。
    pub fn alloc_temp_vreg(&mut self, class: RegClass) -> VReg {
        let vreg = self.alloc_vreg_with_class(class);
        self.temp_vregs.insert(vreg);
        vreg
    }

    /// 分配或复用零值 VReg（用于比较零值）。
    pub fn alloc_zero_vreg(&mut self) -> VReg {
        if let Some(vreg) = self.zero_vreg {
            return vreg;
        }
        let vreg = self.alloc_temp_vreg(self.value_gpr_class);
        self.zero_vreg = Some(vreg);
        vreg
    }
}

impl Default for LowerCtx {
    fn default() -> Self {
        Self::new()
    }
}
