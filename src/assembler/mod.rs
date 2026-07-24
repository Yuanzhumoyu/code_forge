//! 汇编器运行时类型 — 供 DSL 生成的 `impl Assembler for Isa` 使用。
//!
//! 这些类型在运行时被生成的汇编器代码引用，因此必须定义在主 crate 中。

/// 指令字段类型 — 运行时对应 proc-macro 中的 `model::FieldType`。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FieldType {
    VReg,
    I64,
    U8,
    U32,
    F64,
    BlockTarget,
    Reg,
    CondCode,
}

/// 助记符查找表中的单条条目。
#[derive(Debug, Clone)]
pub struct AsmEntry {
    pub mnemonic: String,
    pub inst_name: String,
    pub template_fields: Vec<(String, FieldType)>,
}

/// 运行时组装器查找表 — 由 DSL 编译期生成，运行时消费。
#[derive(Debug, Clone)]
pub struct AsmLookupTable {
    pub entries: Vec<AsmEntry>,
    /// (expanded_mnemonic, inst_name, cond_val)
    pub cc_entries: Vec<(String, String, u8)>,
    pub inst_names: Vec<String>,
}

/// 操作数运行时值 — 从 ASM 文本解析后的中间表示。
#[derive(Debug, Clone)]
pub enum OperandValue {
    /// 物理寄存器引用: `RAX` → Reg { index: 0 }
    Reg { index: u8 },
    /// 虚拟寄存器引用: `VReg(5)` → VReg(5)
    VReg(u32),
    /// 整数立即数: `42`, `0xFF`
    Imm(i64),
    /// 浮点立即数
    Float(f64),
    /// 标签引用: `.L0`
    Label(String),
    /// 标识符 (lowering 上下文): `rd`, `rs1`
    Ident(String),
}

impl AsmLookupTable {
    /// 按助记符查找候选条目。
    pub fn find(&self, mnemonic: &str) -> Vec<&AsmEntry> {
        self.entries
            .iter()
            .filter(|e| e.mnemonic == mnemonic)
            .collect()
    }

    /// 查找条件码展开条目。
    pub fn find_cc(&self, mnemonic: &str) -> Option<(String, u8)> {
        self.cc_entries
            .iter()
            .find(|(m, ..)| m == mnemonic)
            .map(|(_, inst_name, val)| (inst_name.clone(), *val))
    }

}
