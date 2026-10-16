//! 生成物 lowering 上下文与内存引用（v19 V1 由 forge-codegen 迁入）。

use forge_ir::*;
use smallvec::SmallVec;
use std::collections::HashMap;

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

/// **调用约定在 lowering 期的全部状态**（v20）。
///
/// 为什么分组：这一层原本有 6 个散落在 `LowerCtx` 顶层的字段（`call_conv` / `call_conv_name` /
/// `call_layout` / `conv_clobbers` / `conv_ret_gpr` / `module_sigs`），读起来分不清哪些是
/// "IR 声明的"、哪些是"宿主算好的"、哪些是"本函数的"。收进一个结构体后，生成物与管线里
/// 的读口子自解释：`ctx.conv.layout`、`ctx.conv.ret_gpr`、`ctx.conv.variadic_of(f)`。
#[derive(Debug, Clone, Default)]
pub struct CallConvCtx {
    /// 当前函数的调用约定（**IR 声明的那份标识**：`Builtin`/`Named`/`Index`）。
    pub id: CallConvId,
    /// **解析后的约定名**（宿主注册表里的键 `AbiRules::name`；空串 = 还没解析）。
    ///
    /// 与 [`id`](Self::id) 的分工：`id` 是 IR 侧标识，这个名字是**查表用的键**——管线入口
    /// （`CompileState::new`）解析并 fail-closed，因此 lowering 里读到的名字一定注册过。
    pub name: String,
    /// **本函数的调用布局**（由管线从 `forge_abi::AbiPlan` 转好塞进来）：生成物的收参/
    /// 传参/序尾声读它——运行时因此不依赖 forge-abi，只认这份数据。
    ///
    /// `None` = 管线还没接上（v20 A6 起"算不出 plan"是**编译错误**，所以正常路径恒为 `Some`）。
    pub layout: Option<forge_abi_model::AbiPlan>,
    /// **变参外壳**（v23 M2）：a_list 的字段初值等只有编译入口算得出的东西。
    pub va: Option<crate::machine::call_layout::VaInfo>,
    /// **约定级破坏集**：调用点被破坏的寄存器 `(类内号, 类)`，由宿主按 `(ISA, 约定)` 算好。
    ///
    /// 与 [`layout`](Self::layout) 的分工：后者是**本函数**的完整布局（含落点），这份是
    /// **签名无关**的约定级事实（`AbiPlan::clobbers` = 可用池 − callee-saved − pinned）。
    /// 空 = 宿主没给约定数据 ⇒ 生成物在调用点 **fail-closed**（不按谱面里那几个寄存器猜）。
    pub clobbers: Vec<(u32, forge_ir::RegClass)>,
    /// **约定级整数返回槽** `(类内号, 类)`：这份约定把标量整数返回放哪个寄存器
    /// （x86 RAX = 0、riscv a0 = X10、arm64 x0 = 0）。
    ///
    /// 与 [`layout`](Self::layout) 的 `.ret` 的分工：那是**本函数**的返回位（含 sret/浮点/
    /// 聚合各种形态），这份是**约定级**标量返回槽——`Return` 优先用前者，`Call` 读**被调方**
    /// 返回值时用这份（调用点看不到被调方签名）。空 = 宿主没给 ⇒ 生成物 fail-closed。
    pub ret_gpr: Option<(u32, forge_ir::RegClass)>,
    /// **模块级签名表**（[`forge_ir::ModuleSigs`]）：`FuncRef::index()` → 签名事实。
    ///
    /// 调用点只看得见实参形状，判不出"未命名实参在只走栈的约定里要改判到栈"——宿主编译
    /// 模块时手上有每个函数的签名，填在这里。`None` = 没有这份表（单函数编译）。
    ///
    /// 存 `Arc` 是因为它按函数传递（每个函数的 `LowerCtx` 都要一份，克隆 O(1)）。
    pub module_sigs: Option<std::sync::Arc<forge_ir::ModuleSigs>>,
}

impl CallConvCtx {
    /// 本函数的布局（`None` = 管线没接上；v20 A6 起"算不出"是编译错误）。
    pub fn layout(&self) -> Option<&forge_abi_model::AbiPlan> {
        self.layout.as_ref()
    }

    /// **按 `FuncRef` 查被调方的变参信息**（`None` = 非变参 / 没有表 / 查不到）。
    ///
    /// 生成物在调用点只写 `ctx.conv.variadic_of(__f)`，把"查表 + 越界兜底"的逻辑收在这里
    /// （可被宿主侧测试直接覆盖，生成物里不必再摊开）。
    pub fn variadic_of(&self, f: forge_ir::FuncRef) -> Option<(bool, u32)> {
        self.module_sigs
            .as_ref()
            .and_then(|sigs| sigs.variadic_at(f.index() as usize))
    }
}

/// **一个值的完整描述**（v24 M2 的单一来源）：寄存器类 + 位宽 + IR 类型。
///
/// 为什么要有它：这三件事以前分散在四张表里（一张 XReg 键的与三张 VReg 键的），
/// 消费点取到"类型"后还得各自再推"类"与"位宽"——两处回填口径不同就是这么来的。
/// 现在**一个值只有一份描述**，且**只在两个地方产生**：
///
/// * [`LowerCtx::set_value_desc`]（唯一的写入口，类与位宽都从 IR 类型推导）；
/// * [`LowerCtx::desc_of`] / [`LowerCtx::type_bits_of`] 等访问器（唯一的读入口）。
///
/// 生成物因此**不再需要任何"值→类型"旁表**：它读的是访问器，而不是表本身。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValDesc {
    /// 寄存器类（`GPR`/`FPR`/`VEC`/`KReg`，payload = 位宽）。
    pub class: RegClass,
    /// 该值占用的位宽（标量 = 标量位宽；向量 = 整宽；指针按 DataLayout）。
    pub bits: u16,
    /// 对应的 IR 类型（谓词与规则可继续问类型系统）。
    pub ty: TypeId,
}

impl ValDesc {
    /// 是否浮点寄存器类。
    pub fn is_fp(&self) -> bool {
        self.class.is_fp()
    }

    /// 是否整数寄存器类。
    pub fn is_int(&self) -> bool {
        self.class.is_int()
    }

    /// 是否向量寄存器类。
    pub fn is_vector(&self) -> bool {
        matches!(self.class, RegClass::VEC(_))
    }

    /// 占用字节数（向上取整）。
    pub fn bytes(&self) -> u16 {
        self.bits.div_ceil(8)
    }
}

/// 指令选择上下文 — 在 lowering 阶段提供给 [`machine::lowering::TargetLowering`]。
pub struct LowerCtx {
    /// 临时寄存器（XReg）分配器 — XReg 的唯一受控创建入口。
    pub xregs: forge_ir::XRegAllocator,
    /// **值的描述表**（v24 M2 的单一来源）：`XReg → ValDesc{类, 位宽, IR 类型}`。
    ///
    /// 以前这里是四张平行表（一张 XReg 键的 + 三张 VReg 键的），类/位宽/类型分散存放，
    /// 消费点还得各自"从类型推类"（两处口径不同 ⇒ 同一条绑定两种回填结果）。现在
    /// **一个值只有一份描述**，且**表本身私有**——读写一律走访问器
    ///（[`LowerCtx::set_value_desc`] / [`LowerCtx::desc_of`] / [`LowerCtx::type_bits_of`]）。
    ///
    /// 键是 `XReg`（`{index, class}`，**含 class 参与相等**——同一 index 配不同类是不同键；
    /// 按 index 做下标的密集表会把它们合并，属语义变化，所以这里仍是哈希表）。
    values: HashMap<XReg, ValDesc>,
    /// **调用约定相关的一切**（IR 标识 / 解析后的名字 / 本函数布局 / 约定级破坏集与
    /// 返回槽 / 模块级签名表）——见 [`CallConvCtx`]。
    pub conv: CallConvCtx,
    /// 当前函数的常量池（用于解析 Iconst/Fconst 的索引）。
    pub constant_pool: Option<forge_ir::ConstantPool>,
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
    /// **当前指令的逐实参声明属性**（`Instruction::param_attrs` 快照；2026-10-08）。
    ///
    /// 生成的 lowering 只看得到 `(op, args, results, ctx)`，读不到指令本身——而调用点
    /// 的**形状**要靠它：`byval(聚合类型)` 的实参在 IR 里是 `ptr`（指向调用方本地副本），
    /// 但 ABI 上它是**按值聚合**（sysv64 `>16B` ⇒ `StackByVal`）。长度与实参个数对齐。
    pub current_param_attrs: Vec<forge_ir::ir::function::ParamAttributes>,
    /// 当前 AtomicRmw 的操作数 (Immediate::Uint(op as u64) 解析)。
    pub current_atomic_op: Option<forge_ir::ir::opcode::AtomicRmwOp>,
    /// 当前指令引用的全局变量 (GlobalAddr 的 Immediate::Global)。
    pub current_global: Option<GlobalId>,
    /// StackAddr 的帧偏移（Immediate::Int）。
    pub current_offset: i64,
    /// 当前 Alloca 的帧槽偏移（预扫描分配；`lea_off rd, alloca_offset` 规则取用）。
    pub current_alloca_offset: i64,
    /// prologue 压入的 callee-saved 寄存器总字节数（局部变量 lea 的基准平移）。
    ///
    /// **v23 M5（C6 帧记账单点）**：它与 `stack_slot_shift`/`min_frame` 一起由编译器在
    /// lowering 之前算**一次**（`frame_layout_info`，唯一来源），此后序言/发射/spill 账目
    /// 全部读这三个字段——不再各自重算（重算口径一漂移就是帧字节数不一致，表现为局部槽
    /// 覆盖 push 槽 → 静默错值）。
    pub callee_saved_bytes: i32,
    /// 帧的**最小字节数**（fp-inside 布局 = `fp_push_bytes + Σcallee_saved×宽`：riscv 的
    /// ra/fp 保存槽要求帧 ≥ 该值，否则 emit 模板的 `{frame_size_mN}` 偏移为负、写坏 sp 下方）。
    /// 与 `callee_saved_bytes`/`stack_slot_shift` 同源、同时刻填。
    pub min_frame: u32,
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
    /// **寄存器保存区**在帧内的偏移（相对帧基址，v20 变参 V3）。
    ///
    /// 需要它的约定（sysv64/aapcs64/riscv）在序言里把参数寄存器存进这一区、`va_list` 的
    /// 保存区字段指向它。区大小/对齐/槽表来自 plan（`CallLayout.va.save`），**偏移由管线
    /// 分配**（与 `Opcode::VaStart` 的对象槽同一条通路：算好偏移、抬 `max_stack_bytes`），
    /// 序言与 `va_start` 只读它。`None` = 本函数没有保存区（非变参 / win64 栈式）。
    pub va_save_off: Option<i64>,
    /// **保存区占用的帧顶字节数**（v20 V7）：> 0 表示"形状要求保存区与栈实参连续"，
    /// 于是保存区被放在帧顶 `[入口 sp - va_top, 入口 sp)`，帧里其余东西（ra/fp、
    /// callee-saved、局部槽）整体下移这一个量。0 = 历史布局（逐字节不变）。
    ///
    /// 生成物读 `AllocResult.va_top`（fp-inside 的存取机制要给自己的偏移减掉它）；
    /// 管线同时把它加进 `stack_slot_shift`（局部槽）与帧尺寸。
    pub va_top: u32,
    /// **`va_list` 对象**在帧内的偏移（相对帧基址，v20 变参 V3）；`None` = 本函数不变参。
    ///
    /// 由管线在**编译入口**预留（与保存区同一条分配通路），IR 展开（`pipeline::va_expand`）
    /// 用 `StackAddr(本偏移)` 取对象地址——不再依赖 lowering 中途的 `current_offset`。
    pub va_obj_off: Option<i64>,
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
            xregs: forge_ir::XRegAllocator::new(),
            values: HashMap::new(),
            conv: CallConvCtx::default(),
            constant_pool: None,
            is_float_return: false,
            is_sret_return: false,
            current_const_index: 0,
            current_immediates: SmallVec::new(),
            current_func_ref: None,
            current_param_attrs: Vec::new(),
            current_atomic_op: None,
            current_global: None,
            current_offset: 0,
            current_alloca_offset: 0,
            callee_saved_bytes: 0,
            // v23 M5（C6）：编译器在 lowering 之前按 plan 填这三个字段（见 compiler.rs）。
            min_frame: 0,
            stack_slot_shift: 0,
            max_stack_bytes: 0,
            max_stack_arg_bytes: 0,
            va_save_off: None,
            va_top: 0,
            va_obj_off: None,
            default_opsize: 64,
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
            // 档位 `vector_tiers` 是**字节**（ISA 数据口径），`RegClass` 的 payload
            // 是**位** ⇒ 构造类时 ×8（v21 W1：单位只在边界换算一次）。
            return RegClass::VEC(tier.max(1) * 8);
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

    /// **登记一个值的描述**（v24 M2 的唯一写入口）：类与位宽都从 IR 类型推导。
    ///
    /// 以前这条信息由调用方分两处写（`pipeline/lowering.rs` 写类型、回填点再推类），
    /// 两边口径不同时同一条绑定会得到两种结果；现在只有这一处产生描述。
    pub fn set_value_desc(&mut self, x: XReg, ty: TypeId) {
        let class = self.reg_class_for(&ty);
        let bits = self.value_bits(&ty, class);
        self.values.insert(x, ValDesc { class, bits, ty });
    }

    /// 值的描述（v24 M2 的唯一读入口之一）；未登记 → `None`。
    pub fn desc_of(&self, x: &XReg) -> Option<ValDesc> {
        self.values.get(x).copied()
    }

    /// 该值是否为浮点类（生成物谓词用；未登记 → 假）。
    pub fn is_fp_value(&self, x: &XReg) -> bool {
        self.desc_of(x).is_some_and(|d| d.is_fp())
    }

    /// 该值是否为向量类（定长或可伸缩都算；未登记 → 假）。
    pub fn is_vector_value(&self, x: &XReg) -> bool {
        self.desc_of(x).is_some_and(|d| {
            self.type_store
                .as_ref()
                .is_some_and(|s| s.is_vector(d.ty) || s.is_scalable_vector(d.ty))
        })
    }

    /// 值 → 标量位宽（生成的 lowering 用）：问类型快照（指针宽度按 DataLayout、
    /// 动态整数位宽也能答）；值未登记类型或非标量 → `None`。
    pub fn type_bits_of(&self, val: &XReg) -> Option<u32> {
        let d = self.desc_of(val)?;
        self.type_store.as_ref().and_then(|s| s.scalar_bits(d.ty))
    }

    /// 值的**对象字节数**（与 `TypeStore::size_bytes` 同源）：生成物的"按值搬运/收参"
    /// 按真实字节数分派时用它——**不要**用 `ValDesc::bytes()`：后者由位宽向上取整，
    /// 对"存储尺寸 ≠ 标量位宽"的类型（如 80 位扩展浮点存 16 字节）会给出不同答案。
    pub fn value_bytes(&self, x: &XReg) -> Option<u32> {
        let d = self.desc_of(x)?;
        self.type_store.as_ref().map(|s| s.size_bytes(d.ty))
    }

    /// 类与位宽的唯一推导处（`set_value_desc` 用）：标量取标量位宽，向量取整宽，
    /// 其余退回该寄存器的池宽。
    fn value_bits(&self, ty: &TypeId, class: RegClass) -> u16 {
        let store = self.type_store.as_ref();
        match store.and_then(|s| s.scalar_bits(*ty)) {
            Some(b) => b as u16,
            None => store
                .map(|s| (s.size_bytes(*ty).saturating_mul(8)) as u16)
                .unwrap_or_else(|| class.bits()),
        }
    }
}

impl Default for LowerCtx {
    fn default() -> Self {
        Self::new()
    }
}
