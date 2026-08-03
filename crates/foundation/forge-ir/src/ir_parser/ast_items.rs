//! 解析中间表示（AST）—— grammar.lalrpop 的 action 产物，semantics 消费。

use super::lexer::ElemTy;

// ── 模块级 ──

/// 模块级项：target 设置或函数。
pub enum ParsedItem {
    Target(String, String),
    Function(ParsedFunction),
}

pub struct ParsedModule {
    pub items: Vec<ParsedItem>,
}

// ── 函数/块/指令 ──

pub struct ParsedFunction {
    pub name: String,
    pub ret_ty: ParsedType,
    pub params: Vec<(ParsedType, String)>,
    pub blocks: Vec<ParsedBlock>,
    pub declare: bool,
}

pub struct ParsedBlock {
    pub label: String,
    pub insts: Vec<ParsedInst>,
    pub terminator: ParsedTerminator,
}

pub struct ParsedInst {
    /// 结果名（`%r = ...`）；None 表示无结果指令。
    pub result: Option<String>,
    /// LLVM 指令名（add/load/...）。
    pub opcode: String,
    /// icmp/fcmp 条件（eq/slt/...）。
    pub cond: Option<String>,
    /// 类型化操作数（call 的首个操作数是 callee，load/store 地址也在此）。
    pub args: Vec<ParsedOperand>,
}

impl ParsedInst {
    pub fn with_result(mut self, name: String) -> Self {
        self.result = Some(name);
        self
    }
}

pub struct ParsedOperand {
    pub ty: ParsedType,
    pub op: Operand,
}

#[derive(Clone)]
pub enum Operand {
    Local(String),
    Global(String),
    Int(i64),
    UInt(u64),
    Float(f64),
    Bool(bool),
    Null,
    Undef,
    Poison,
}

pub enum ParsedTerminator {
    Return(Vec<ParsedOperand>),
    Jump(String),
    Branch(Option<ParsedOperand>, String, String),
    Switch(ParsedOperand, String, Vec<(i64, String)>),
    Unreachable,
}

// ── 类型树（语义层转 forge TypeId）──
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParsedType {
    Int(u16),
    Float(u16),
    Ptr,
    Void,
    Label,
    Metadata,
    Vec(u32, Box<ParsedType>),
    Array(u64, Box<ParsedType>),
    Struct(Vec<ParsedType>),
}

impl From<ElemTy> for ParsedType {
    fn from(e: ElemTy) -> Self {
        match e {
            ElemTy::Int(bits) => ParsedType::Int(bits),
            ElemTy::Float(bits) => ParsedType::Float(bits),
            ElemTy::Ptr => ParsedType::Ptr,
        }
    }
}
