//! 类型化操作数中间表示（ISA 无关）。
//!
//! lalrpop parser（每 ISA 生成）把汇编文本解析为 `Vec<AsmLine>`：
//! 每行可含标签定义与一条指令的中间表示 `RawInst`（inst_idx + 操作数值）。
//! 由 DSL 生成的 bind 层按字段类型签名把操作数绑定为 `Inst` 字段。
//!
//! `OperandValue` 保留 token 类别（寄存器/立即数/标签/虚拟操作数…），
//! 供类型检查消歧与 `parse_lines` 中间表示 API 消费。

/// 解析阶段的操作数值。
#[derive(Debug, Clone, PartialEq)]
pub enum OperandValue {
    /// 标识符形式的操作数（物理寄存器名 "RAX"/"R0" 或虚拟操作数 "rd"/"rs1"）——
    /// 是否物理寄存器由 bind 阶段按 `[reg.*]` 查表决定。
    Ident(String),
    /// 临时寄存器：`%t`、`%t0`（lower/emit 指令包）。
    TmpReg(String),
    /// 虚拟寄存器字面量：`VReg(N)`（保留的过渡语法）。
    VReg(u32),
    /// 整型立即数（dec / `0x` hex / aarch64 `#n`；负号已在语法层合并）。
    Imm(i64),
    /// 浮点立即数。
    Float(f64),
    /// 标签引用：`.Lxxx` / `@xxx` / 数字偏移（BlockTarget 字段）。
    Label(String),
    /// 常量池内联：`{const N}`。
    Const(i64),
    /// MemRef 字段的完整内存结构：`[base ± disp]`。
    Mem { base: Option<String>, disp: i64 },
}

impl OperandValue {
    /// 该操作数的期望类型签名（供 AsmError::TypeMismatch 诊断）。
    pub fn ty(&self) -> OperandTy {
        match self {
            OperandValue::Ident(_) | OperandValue::TmpReg(_) | OperandValue::VReg(_) => {
                OperandTy::Reg
            }
            OperandValue::Imm(_) | OperandValue::Const(_) => OperandTy::Imm,
            OperandValue::Float(_) => OperandTy::Float,
            OperandValue::Label(_) => OperandTy::Label,
            OperandValue::Mem { .. } => OperandTy::MemRef,
        }
    }
}

/// 从 Reg 类操作数提取名字（Mem 规则的 base 用）。
pub fn mem_base(op: &OperandValue) -> String {
    match op {
        OperandValue::Ident(n) | OperandValue::TmpReg(n) => n.clone(),
        OperandValue::VReg(i) => format!("VReg({i})"),
        _ => String::new(),
    }
}

/// 从 Imm 类操作数提取整数值（Mem 规则的 disp 用）。
pub fn imm_val(op: &OperandValue) -> i64 {
    match op {
        OperandValue::Imm(v) | OperandValue::Const(v) => *v,
        _ => 0,
    }
}

/// 操作数类型签名——bind 阶段按此做类型检查消歧，错误报告也用它。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandTy {
    /// 寄存器类（Ireg/Freg/GprReg/XmmReg 由 bind 按 ISA 表细分）。
    Reg,
    /// 整型立即数（宽度/符号由 bind 按字段类型检查）。
    Imm,
    /// 浮点立即数。
    Float,
    /// 内存引用（模板展开为多个子字段）。
    MemRef,
    /// 块标签（BlockTarget）。
    Label,
    /// 条件码（CondCode）。
    Cond,
    /// 操作数大小（隐式，不参与解析）。
    Opsize,
}

impl std::fmt::Display for OperandTy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            OperandTy::Reg => "register",
            OperandTy::Imm => "immediate",
            OperandTy::Float => "float",
            OperandTy::MemRef => "memory reference",
            OperandTy::Label => "label",
            OperandTy::Cond => "condition code",
            OperandTy::Opsize => "operand size",
        };
        write!(f, "{s}")
    }
}

/// 解析后的一行汇编：可选标签定义 + 可选指令。
#[derive(Debug, Clone, PartialEq)]
pub struct AsmLine {
    /// 标签定义名（`loop:` / `.Lloop:`），位置由两遍解析计算。
    pub label: Option<String>,
    /// 指令中间表示。
    pub inst: Option<RawInst>,
}

/// 指令中间表示——inst_idx 对应 DSL 生成的 bind 表。
#[derive(Debug, Clone, PartialEq)]
pub struct RawInst {
    /// 指令在 bind 表中的索引（lalrpop action 中由生成器填入常量）。
    pub inst_idx: usize,
    /// 人可读的 mnemonic（诊断用）。
    pub mnemonic: String,
    /// 按模板字段顺序排列的操作数值。
    pub operands: Vec<OperandValue>,
}
