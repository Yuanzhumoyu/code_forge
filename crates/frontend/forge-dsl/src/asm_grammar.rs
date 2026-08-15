//! 每 ISA 专属汇编语法生成器。
//!
//! 从 ISA model（`[inst.*].asm` 模板 + `[cc_names]`）生成：
//! 1. Token 枚举文本（logos derive）——标点/通用 token + 该 ISA 的保留字
//!    （mnemonic / cc 名 / 模板字面标识符如 `RBP`）变体（priority=2，高于 Ident）
//! 2. lalrpop 语法文本——extern 块 + 模板驱动的规则候选，操作数按 FieldType
//!    映射为类型化非终结符（Reg/Imm/Float/Label/Cond/Mem…）
//! 3. 调用 lalrpop 库编译语法为 parser 代码（写 OUT_DIR 缓存，供 include!）
//!
//! 类型系统消歧（用户补充要求）：
//! - 语法层：同 mnemonic 候选按操作数非终结符的首 token 区分（Reg→Ident、
//!   Imm→IntLit、Mem→[、Label→.L/@）——LR lookahead 自动消歧
//! - 语义层：同 mnemonic 同操作数类型序列的候选合并为「候选组」，由生成的
//!   bind 层按字段类型签名（寄存器类别/立即数宽度）过滤
//! - 生成期：保留字 token 变体冲突（pascal 命名碰撞）追加数字后缀避免

use crate::asm_resolver::ParsedTemplate;
use crate::model::{FieldType, IsaModel};
use std::collections::{BTreeMap, BTreeSet};

/// 模板片段（与 asm_resolver::AsmFrag 逐字节同构——直接复用,删除双份定义;
/// 第三十五轮重试,保留 inst_operands.push 等关键行）。
pub use crate::asm_resolver::AsmFrag as Frag;

/// 一个操作数字段的类型签名。
#[derive(Debug, Clone)]
pub struct OperandSpec {
    pub field: String,
    pub ty: FieldType,
}

/// 一条模板展开后的规则候选（组代表）。
#[derive(Debug, Clone)]
pub struct Candidate {
    /// 组索引（规则 action 输出的 inst_idx 语义 = 候选组索引）。
    pub group: usize,
    /// 展开后的 mnemonic 字面（"mov" / "sete" / "fadd.s"…）。
    pub mnemonic: String,
    /// 操作数片段（含字面锚点）。
    pub frags: Vec<Frag>,
}

/// 生成期元数据——bind 层生成所需。
pub struct AsmMeta {
    /// 候选组 → 组内 inst 序号列表（BTreeMap 迭代序）。
    pub groups: Vec<Vec<usize>>,
    /// inst 序号 → inst 名（diagnostic）。
    pub inst_names: Vec<String>,
    /// inst 序号 → 字段类型列表（bind 语义消歧用）。
    pub inst_fields: Vec<Vec<(String, FieldType)>>,
    /// inst 序号 → 模板操作数字段（字段名 + 类型，按模板顺序）——
    /// `OperandSpec.ty` 直接驱动 bind 的类型签名消歧与字段绑定。
    pub inst_operands: Vec<Vec<OperandSpec>>,
    /// 候选组 → mnemonic 内嵌 cond 的值（set{cond}→sete 的 cc 值；无则 None）。
    /// 同组候选共享同一 mnemonic，故 cc 值按组存储。
    pub group_mnemonic_cc: Vec<Option<u8>>,
}

/// 前一个字面留下的前缀标记（决定紧跟字段的非终结符）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrevLit {
    None,
    HexPrefix,  // "0x"
    HashPrefix, // "#"
    DotLPrefix, // ".L"
}

/// 从 model 构建候选组与元数据。
pub fn build_meta(model: &IsaModel) -> Result<(Vec<Candidate>, AsmMeta), String> {
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut group_map: BTreeMap<(String, String), usize> = BTreeMap::new();
    let inst_names: Vec<String> = model.inst.keys().cloned().collect();
    let inst_fields: Vec<Vec<(String, FieldType)>> = model
        .inst
        .values()
        .map(|i| {
            i.fields
                .iter()
                .map(|f| (f.name.clone(), f.field_type.clone()))
                .collect()
        })
        .collect();
    let mut inst_operands: Vec<Vec<OperandSpec>> = Vec::with_capacity(inst_fields.len());
    let mut group_mnemonic_cc: Vec<Option<u8>> = Vec::new();

    for (inst_idx, inst) in model.inst.values().enumerate() {
        let asm = strip_comment(&inst.asm);
        // 第一个空白切 mnemonic（保留点号 mnemonic 与 {cond}），剩余为操作数文本
        let (head, rest) = match asm.find(char::is_whitespace) {
            Some(p) => (asm[..p].trim().to_string(), asm[p..].trim().to_string()),
            None => (asm.trim().to_string(), String::new()),
        };
        let fields: Vec<(String, FieldType)> = inst
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.field_type.clone()))
            .collect();
        // 操作数片段：独立解析（与 mnemonic 无关）
        let op_template = ParsedTemplate::parse(&rest, &fields);
        let op_frags: Vec<Frag> = op_template.frags.clone();
        let operands: Vec<OperandSpec> = op_frags
            .iter()
            .filter_map(|f| match f {
                Frag::Field { name, ty } if !matches!(ty, FieldType::Opsize) => Some(OperandSpec {
                    field: name.clone(),
                    ty: ty.clone(),
                }),
                _ => None,
            })
            .collect();
        inst_operands.push(operands.clone());

        // mnemonic 字面：含 {cond} 则按 cc_names 展开（记录 cc 值），否则原样
        let mnemonics: Vec<(String, Option<u8>)> = if head.contains('{') {
            model
                .cc_names
                .iter()
                .map(|(hex_key, cc_name)| {
                    let cc_val = u64::from_str_radix(
                        hex_key.trim_start_matches("0x").trim_start_matches("0X"),
                        16,
                    )
                    .unwrap_or(0) as u8;
                    (head.replace("{cond}", cc_name), Some(cc_val))
                })
                .collect()
        } else {
            vec![(head.clone(), None)]
        };

        let ty_key = shape_key(&op_frags);
        for (m, cc_val) in &mnemonics {
            let key = (m.clone(), ty_key.clone());
            let gi = match group_map.get(&key) {
                Some(&gi) => gi,
                None => {
                    let gi = groups.len();
                    groups.push(vec![inst_idx]);
                    group_mnemonic_cc.push(*cc_val);
                    group_map.insert(key, gi);
                    candidates.push(Candidate {
                        group: gi,
                        mnemonic: m.clone(),
                        frags: op_frags.clone(),
                    });
                    gi
                }
            };
            if !groups[gi].contains(&inst_idx) {
                groups[gi].push(inst_idx);
            }
        }
    }

    let meta = AsmMeta {
        groups,
        inst_names,
        inst_fields,
        inst_operands,
        group_mnemonic_cc,
    };
    Ok((candidates, meta))
}

/// 语法形状签名：mnemonic 之外的分组键。
///
/// 使用**语法层**类别（Reg/Imm/HexImm/HashImm/Label/Mem/Cond/Float + 字面类别）
/// 而非细粒度 FieldType：`(Ireg,Ireg)` 与 `(Freg,Freg)` 在语法上同为
/// `Reg,Reg`，必须合并为同一候选组（由 bind 按字段类型语义消歧）；
/// `Reg,Reg` 与 `Reg,HashImm` 语法不同（token 首字符不同）→ 不同组。
fn shape_key(frags: &[Frag]) -> String {
    let mut s = String::new();
    let mut prev = PrevLit::None;
    for f in frags {
        s.push('|');
        match f {
            Frag::Lit(lit) => {
                if lit == "+" || lit == "-" {
                    s.push_str("PM");
                    continue;
                }
                let bytes: Vec<char> = lit.chars().collect();
                let mut i = 0usize;
                while i < bytes.len() {
                    let c = bytes[i];
                    if c.is_whitespace() {
                        i += 1;
                        continue;
                    }
                    if c == '0'
                        && i + 1 < bytes.len()
                        && (bytes[i + 1] == 'x' || bytes[i + 1] == 'X')
                    {
                        s.push_str("0X");
                        prev = PrevLit::HexPrefix;
                        i += 2;
                        continue;
                    }
                    if c == '#' {
                        let mut j = i + 1;
                        while j < bytes.len() && bytes[j].is_ascii_digit() {
                            j += 1;
                        }
                        if j > i + 1 {
                            s.push_str("HASHINT");
                            i = j;
                        } else {
                            s.push_str("HASH");
                            prev = PrevLit::HashPrefix;
                            i += 1;
                        }
                        continue;
                    }
                    if c == '.' && i + 1 < bytes.len() && bytes[i + 1] == 'L' {
                        s.push_str("DOTL");
                        prev = PrevLit::DotLPrefix;
                        i += 2;
                        continue;
                    }
                    if c.is_ascii_digit() {
                        s.push_str("INT");
                        while i < bytes.len() && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                        continue;
                    }
                    if c.is_ascii_alphabetic() || c == '_' || c == '$' {
                        let start = i;
                        while i < bytes.len()
                            && (bytes[i].is_ascii_alphanumeric()
                                || bytes[i] == '_'
                                || bytes[i] == '.'
                                || bytes[i] == '$')
                        {
                            i += 1;
                        }
                        let id: String = bytes[start..i].iter().collect();
                        s.push_str(&id);
                        continue;
                    }
                    let tok = match c {
                        ',' => "Comma",
                        '[' => "LBracket",
                        ']' => "RBracket",
                        '(' => "LParen",
                        ')' => "RParen",
                        '+' => "Plus",
                        '-' => "Minus",
                        '*' => "Star",
                        '!' => "Bang",
                        ':' => "Colon",
                        _ => "?",
                    };
                    s.push_str(tok);
                    i += 1;
                }
            }
            Frag::Field { ty, .. } => {
                let nt = match ty {
                    FieldType::Ireg | FieldType::Freg | FieldType::GprReg | FieldType::XmmReg => {
                        "Reg"
                    }
                    FieldType::MemRef => "Mem",
                    FieldType::BlockTarget => "Label",
                    FieldType::CondCode => "Cond",
                    FieldType::F32 | FieldType::F64 => "Float",
                    FieldType::Opsize => "Opsize",
                    _ => match prev {
                        PrevLit::HexPrefix => "HexImm",
                        PrevLit::HashPrefix => "HashImm",
                        PrevLit::DotLPrefix => "Label",
                        PrevLit::None => "Imm",
                    },
                };
                s.push_str(nt);
                if !matches!(prev, PrevLit::None) {
                    prev = PrevLit::None;
                }
            }
        }
    }
    s
}

/// 收集保留字：mnemonic 字面 + cc 名 + 操作数字面中的标识符（如 RBP）。
pub fn collect_reserved(model: &IsaModel, candidates: &[Candidate]) -> BTreeSet<String> {
    let mut words: BTreeSet<String> = BTreeSet::new();
    for c in candidates {
        words.insert(c.mnemonic.clone());
        for f in &c.frags {
            if let Frag::Lit(s) = f {
                for id in lit_idents(s) {
                    words.insert(id);
                }
            }
        }
    }
    for cc_name in model.cc_names.values() {
        words.insert(cc_name.clone());
    }
    words
}

/// 提取字面文本中的标识符序列（字母/下划线/$ 开头，含内部点号）。
fn lit_idents(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$' {
            cur.push(c);
        } else {
            if !cur.is_empty()
                && cur
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if !cur.is_empty()
        && cur
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
    {
        out.push(cur);
    }
    out
}

/// 保留字 → 变体名（全局唯一）。
pub fn variant_names(reserved: &BTreeSet<String>) -> BTreeMap<String, String> {
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for word in reserved {
        let base = pascal_ident(word);
        let mut name = base.clone();
        let mut n = 2;
        while names.values().any(|v| v == &name) {
            name = format!("{base}{n}");
            n += 1;
        }
        names.insert(word.clone(), name);
    }
    names
}

/// 生成 Token 枚举的 Rust 代码文本。
pub fn gen_token_enum_text(model: &IsaModel) -> Result<String, String> {
    let (candidates, _) = build_meta(model)?;
    let reserved = collect_reserved(model, &candidates);
    let names = variant_names(&reserved);

    let mut out = String::new();
    out.push_str("// 自动生成：该 ISA 的汇编 token 枚举（forge-dsl asm_grammar）\n");
    out.push_str("#[derive(::logos::Logos, Debug, Clone, PartialEq)]\n");
    out.push_str("#[logos(skip(r\"[ \\t\\r]+|//[^\\n]*|;[^\\n]*|#[^\\n\\d][^\\n]*\", allow_greedy = true))]\n");
    out.push_str("pub enum Token {\n");
    for (word, name) in &names {
        let esc = escape_lit(word);
        out.push_str(&format!("    /// 保留字 `{esc}`\n"));
        out.push_str(&format!("    #[token(\"{esc}\", priority = 2)]\n"));
        out.push_str(&format!("    {name},\n"));
    }
    out.push_str("    /// 标识符（寄存器名/虚拟操作数/点号 mnemonic），priority=1 让位于保留字\n");
    out.push_str(
        "    #[regex(r\"[A-Za-z_][A-Za-z0-9_.]*\", |l| l.slice().to_string(), priority = 1)]\n",
    );
    out.push_str("    Ident(String),\n");
    out.push_str("    /// 局部标签操作数 `.Lxxx`\n");
    out.push_str("    #[regex(r\"\\.[A-Za-z_][A-Za-z0-9_.]*\", |l| l.slice().to_string())]\n");
    out.push_str("    DotLabel(String),\n");
    out.push_str("    /// 内部符号 / emit 宏标签 `@xxx`\n");
    out.push_str("    #[regex(r\"@[A-Za-z_][A-Za-z0-9_.]*\", |l| l.slice().to_string())]\n");
    out.push_str("    AtLabel(String),\n");
    out.push_str("    /// 临时寄存器 `%t`\n");
    out.push_str("    #[regex(r\"%[A-Za-z_][A-Za-z0-9_.]*\", |l| l.slice().to_string())]\n");
    out.push_str("    TmpReg(String),\n");
    out.push_str("    /// 虚拟寄存器字面量 `VReg(N)`\n");
    out.push_str("    #[regex(r\"[vV][rR]eg\\([0-9]+\\)\", |l| l.slice()[l.slice().find('(').unwrap_or(0) + 1..l.slice().len() - 1].parse().unwrap_or(0))]\n");
    out.push_str("    VRegLit(u32),\n");
    out.push_str("    /// 常量池内联 `{const N}`\n");
    out.push_str("    #[regex(r\"\\{const -?[0-9]+\\}\", |l| l.slice()[7..l.slice().len() - 1].trim().parse().unwrap_or(0))]\n");
    out.push_str("    ConstLit(i64),\n");
    out.push_str(
        "    /// 十六进制立即数 `0xFF`（超出 i64 时按 u64 位模式截断：0xFFFFFFFFFFFFFFFF = -1）\n",
    );
    out.push_str("    #[regex(r\"0[xX][0-9a-fA-F]+\", |l| u64::from_str_radix(&l.slice()[2..], 16).unwrap_or(0) as i64)]\n");
    out.push_str("    HexLit(i64),\n");
    out.push_str("    /// 十进制立即数\n");
    out.push_str("    #[regex(r\"[0-9]+\", |l| l.slice().parse().unwrap_or(0))]\n");
    out.push_str("    IntLit(i64),\n");
    out.push_str("    /// aarch64 立即数前缀 `#5`\n");
    out.push_str(
        "    #[regex(r\"#[0-9]+\", |l| l.slice()[1..].parse().unwrap_or(0), priority = 2)]\n",
    );
    out.push_str("    HashInt(i64),\n");
    out.push_str("    /// 浮点立即数\n");
    out.push_str("    #[regex(r\"[0-9]+\\.[0-9]+([eE][+-]?[0-9]+)?\", |l| l.slice().parse().unwrap_or(0.0))]\n");
    out.push_str("    FloatLit(f64),\n");
    let punct: &[(&str, &str)] = &[
        ("Comma", ","),
        ("LBracket", "["),
        ("RBracket", "]"),
        ("LParen", "("),
        ("RParen", ")"),
        ("Plus", "+"),
        ("Minus", "-"),
        ("Star", "*"),
        ("Bang", "!"),
        ("Colon", ":"),
        ("Newline", "\n"),
    ];
    for (name, lit) in punct {
        let esc = escape_lit(lit);
        out.push_str(&format!("    /// `{esc}`\n"));
        out.push_str(&format!("    #[token(\"{esc}\")]\n"));
        out.push_str(&format!("    {name},\n"));
    }
    out.push_str("}\n");
    // lalrpop 的 ParseError Display 需要 Token: Display（解析错误报告用）
    out.push_str("impl std::fmt::Display for Token {\n");
    out.push_str("    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n");
    out.push_str("        write!(f, \"{self:?}\")\n");
    out.push_str("    }\n");
    out.push_str("}\n");
    Ok(out)
}

/// 生成 lalrpop 语法文本（含 extern 块与模板驱动规则）。
pub fn gen_grammar_text(model: &IsaModel) -> Result<String, String> {
    let (candidates, _meta) = build_meta(model)?;
    let reserved = collect_reserved(model, &candidates);
    let names = variant_names(&reserved);

    let mut out = String::new();
    // 语法 use：crate:: 绝对路径不被 lalrpop 的 super 重写；Token 经 super 引用
    // （grammar.rs include 于 mod asm_parser 内，super = ISA 模块）。
    out.push_str("use super::Token;\n");
    out.push_str("use crate::assembler::{AsmLine, LexError, OperandValue, RawInst};\n");
    out.push_str("use crate::assembler::{imm_val, mem_base};\n\n");
    out.push_str("grammar;\n\n");
    out.push_str("extern {\n");
    out.push_str("    type Location = usize;\n");
    out.push_str("    type Error = LexError;\n");
    out.push_str("    enum Token {\n");
    for word in &reserved {
        let name = &names[word];
        out.push_str(&format!("        {name} => Token::{name},\n"));
    }
    out.push_str("        Ident => Token::Ident(<String>),\n");
    out.push_str("        DotLabel => Token::DotLabel(<String>),\n");
    out.push_str("        AtLabel => Token::AtLabel(<String>),\n");
    out.push_str("        TmpReg => Token::TmpReg(<String>),\n");
    out.push_str("        VRegLit => Token::VRegLit(<u32>),\n");
    out.push_str("        ConstLit => Token::ConstLit(<i64>),\n");
    out.push_str("        HexLit => Token::HexLit(<i64>),\n");
    out.push_str("        IntLit => Token::IntLit(<i64>),\n");
    out.push_str("        HashInt => Token::HashInt(<i64>),\n");
    out.push_str("        FloatLit => Token::FloatLit(<f64>),\n");
    out.push_str("        Comma => Token::Comma,\n");
    out.push_str("        LBracket => Token::LBracket,\n");
    out.push_str("        RBracket => Token::RBracket,\n");
    out.push_str("        LParen => Token::LParen,\n");
    out.push_str("        RParen => Token::RParen,\n");
    out.push_str("        Plus => Token::Plus,\n");
    out.push_str("        Minus => Token::Minus,\n");
    out.push_str("        Star => Token::Star,\n");
    out.push_str("        Bang => Token::Bang,\n");
    out.push_str("        Colon => Token::Colon,\n");
    out.push_str("        Newline => Token::Newline,\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");

    // ── 顶层 ──
    out.push_str("pub Program: Vec<AsmLine> = {\n");
    out.push_str("    <lines: Line*> => lines,\n");
    out.push_str(
        "    <lines: Line*> <last: TailLine> => { let mut v = lines; v.push(last); v },\n",
    );
    out.push_str("};\n\n");

    out.push_str("Line: AsmLine = {\n");
    out.push_str("    <label: LabelDef> <inst: Inst> Newline => AsmLine { label: Some(label), inst: Some(inst) },\n");
    out.push_str("    <label: LabelDef> Newline => AsmLine { label: Some(label), inst: None },\n");
    out.push_str("    <inst: Inst> Newline => AsmLine { label: None, inst: Some(inst) },\n");
    out.push_str("    Newline => AsmLine { label: None, inst: None },\n");
    out.push_str("};\n\n");

    // 文件末尾无换行的最后一行
    out.push_str("TailLine: AsmLine = {\n");
    out.push_str(
        "    <label: LabelDef> <inst: Inst> => AsmLine { label: Some(label), inst: Some(inst) },\n",
    );
    out.push_str("    <inst: Inst> => AsmLine { label: None, inst: Some(inst) },\n");
    out.push_str("};\n\n");

    out.push_str("LabelDef: String = {\n");
    out.push_str("    <id: Ident> Colon => id,\n");
    out.push_str("    <l: DotLabel> Colon => l,\n");
    out.push_str("};\n\n");

    // ── 操作数非终结符 ──
    out.push_str("Reg: OperandValue = {\n");
    out.push_str("    <id: Ident> => OperandValue::Ident(id),\n");
    out.push_str("    <t: TmpReg> => OperandValue::TmpReg(t),\n");
    out.push_str("    <v: VRegLit> => OperandValue::VReg(v),\n");
    // 模板字面标识符（如 lea_off 的 RBP）是保留字 token，操作数位置须显式接受
    for word in &reserved {
        let name = &names[word];
        out.push_str(&format!(
            "    {name} => OperandValue::Ident(\"{}\".to_string()),\n",
            escape_lit(word)
        ));
    }
    out.push_str("};\n\n");

    out.push_str("Imm: OperandValue = {\n");
    out.push_str("    <i: IntLit> => OperandValue::Imm(i),\n");
    out.push_str("    <h: HexLit> => OperandValue::Imm(h),\n");
    out.push_str("    <h: HashInt> => OperandValue::Imm(h),\n");
    out.push_str("    <c: ConstLit> => OperandValue::Const(c),\n");
    out.push_str("    Minus <i: IntLit> => OperandValue::Imm(i.wrapping_neg()),\n");
    out.push_str("    Minus <h: HexLit> => OperandValue::Imm(h.wrapping_neg()),\n");
    out.push_str("};\n\n");

    // 字面前缀专用：0x{imm} / #{imm}（前缀已并入 lexer token，不能拆开）
    out.push_str("HexImm: OperandValue = {\n");
    out.push_str("    <h: HexLit> => OperandValue::Imm(h),\n");
    out.push_str("    <c: ConstLit> => OperandValue::Const(c),\n");
    out.push_str("};\n\n");
    out.push_str("HashImm: OperandValue = {\n");
    out.push_str("    <h: HashInt> => OperandValue::Imm(h),\n");
    out.push_str("    <c: ConstLit> => OperandValue::Const(c),\n");
    out.push_str("};\n\n");

    out.push_str("Float: OperandValue = {\n");
    out.push_str("    <f: FloatLit> => OperandValue::Float(f),\n");
    out.push_str("};\n\n");

    out.push_str("Label: OperandValue = {\n");
    out.push_str("    <l: DotLabel> => OperandValue::Label(l),\n");
    out.push_str("    <l: AtLabel> => OperandValue::Label(l),\n");
    out.push_str("    <i: IntLit> => OperandValue::Label(i.to_string()),\n");
    out.push_str("    <h: HexLit> => OperandValue::Label(format!(\"0x{:x}\", h)),\n");
    out.push_str("};\n\n");

    out.push_str("Sign: i64 = {\n");
    out.push_str("    Plus => 1,\n");
    out.push_str("    Minus => -1,\n");
    out.push_str("};\n\n");

    // mem 结构内的宽松符号分隔符（[base±index*scale±disp]）
    out.push_str("PM: () = {\n");
    out.push_str("    Plus => (),\n");
    out.push_str("    Minus => (),\n");
    out.push_str("};\n\n");

    // MemRef 字段：`[base]` / `[base ± disp]`
    out.push_str("Mem: OperandValue = {\n");
    out.push_str("    LBracket <b: Reg> RBracket => OperandValue::Mem { base: Some(mem_base(&b)), disp: 0 },\n");
    out.push_str("    LBracket <b: Reg> <s: Sign> <d: Imm> RBracket => {\n");
    out.push_str("        let disp = if s > 0 { imm_val(&d) } else { -imm_val(&d) };\n");
    out.push_str("        OperandValue::Mem { base: Some(mem_base(&b)), disp }\n");
    out.push_str("    },\n");
    out.push_str("};\n\n");

    // 独立 CondCode 字段（csel ..., {cond}）：cc 名保留字 → 返回 cc 名字符串
    out.push_str("Cond: OperandValue = {\n");
    for cc_name in model.cc_names.values() {
        if let Some(name) = names.get(cc_name) {
            out.push_str(&format!(
                "    {name} => OperandValue::Ident(\"{}\".to_string()),\n",
                escape_lit(cc_name)
            ));
        }
    }
    out.push_str("};\n\n");

    // ── 指令规则：按 mnemonic 分组（候选组 gi → inst_idx=gi）──
    out.push_str("Inst: RawInst = {\n");
    // 伪指令：@pop_callee 等 emit 宏（inst_idx = usize::MAX 哨兵）
    out.push_str(
        "    <m: AtLabel> => RawInst { inst_idx: usize::MAX, mnemonic: m, operands: vec![] },\n",
    );
    let mut by_mnemonic: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for c in &candidates {
        by_mnemonic
            .entry(c.mnemonic.clone())
            .or_default()
            .push(c.group);
    }
    for (mnemonic, gis) in &by_mnemonic {
        let tok = &names[mnemonic];
        for &gi in gis {
            let cand = &candidates[gi];
            let mut rule = format!("    {tok}");
            let mut op_names: Vec<String> = Vec::new();
            let frags = &cand.frags;
            let mut n = 0usize;
            let mut prev = PrevLit::None;
            for f in frags {
                rule.push(' ');
                rule.push_str(&frag_to_rule(f, &mut prev, &names, &mut op_names, &mut n)?);
            }
            let ops = op_names.join(", ");
            let mnem_lit = escape_lit(mnemonic);
            rule.push_str(&format!(
                " => RawInst {{ inst_idx: {gi}, mnemonic: \"{mnem_lit}\".to_string(), operands: vec![{ops}] }},\n"
            ));
            out.push_str(&rule);
        }
    }
    out.push_str("};\n\n");

    Ok(out)
}

/// 片段 → lalrpop 规则片段。
fn frag_to_rule(
    f: &Frag,
    prev: &mut PrevLit,
    names: &BTreeMap<String, String>,
    op_names: &mut Vec<String>,
    n: &mut usize,
) -> Result<String, String> {
    match f {
        Frag::Field { name, ty } => {
            let _ = name;
            let (nt, consume_prev) = match ty {
                FieldType::Ireg | FieldType::Freg | FieldType::GprReg | FieldType::XmmReg => {
                    ("Reg", false)
                }
                FieldType::MemRef => ("Mem", false),
                FieldType::BlockTarget => ("Label", false),
                FieldType::CondCode => ("Cond", false),
                FieldType::F32 | FieldType::F64 => ("Float", false),
                FieldType::Opsize => return Ok(String::new()),
                _ => {
                    // 整型立即数：前导 0x → HexImm、# → HashImm、.L → Label
                    let nt = match *prev {
                        PrevLit::HexPrefix => "HexImm",
                        PrevLit::HashPrefix => "HashImm",
                        PrevLit::DotLPrefix => "Label",
                        PrevLit::None => "Imm",
                    };
                    (nt, true)
                }
            };
            if consume_prev {
                *prev = PrevLit::None;
            }
            let bind = format!("o{n}");
            *n += 1;
            op_names.push(bind.clone());
            Ok(format!("<{bind}:{nt}>"))
        }
        Frag::Lit(s) => {
            if s == "+" || s == "-" {
                // mem 内符号宽松：`(+|-)`（用户可写 [RBP-8]）
                return Ok("PM".to_string());
            }
            let mut parts: Vec<String> = Vec::new();
            let bytes: Vec<char> = s.chars().collect();
            let mut i = 0usize;
            while i < bytes.len() {
                let c = bytes[i];
                if c.is_whitespace() {
                    i += 1;
                    continue;
                }
                if c == '0' && i + 1 < bytes.len() && (bytes[i + 1] == 'x' || bytes[i + 1] == 'X') {
                    *prev = PrevLit::HexPrefix;
                    i += 2;
                    continue;
                }
                if c == '#' {
                    // # 后跟数字 → HashInt token（如 `brk #0`）；否则为 #imm 字段前缀
                    let mut j = i + 1;
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j > i + 1 {
                        parts.push("HashInt".to_string());
                        i = j;
                    } else {
                        *prev = PrevLit::HashPrefix;
                        i += 1;
                    }
                    continue;
                }
                if c == '.' && i + 1 < bytes.len() && bytes[i + 1] == 'L' {
                    *prev = PrevLit::DotLPrefix;
                    i += 2;
                    continue;
                }
                if c.is_ascii_digit() {
                    // 模板里的数字字面（无前缀）→ IntLit 宽松匹配
                    parts.push("IntLit".to_string());
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    continue;
                }
                if c.is_ascii_alphabetic() || c == '_' || c == '$' {
                    // 标识符字面（RBP/RAX…）→ 保留字 token
                    let start = i;
                    while i < bytes.len()
                        && (bytes[i].is_ascii_alphanumeric()
                            || bytes[i] == '_'
                            || bytes[i] == '.'
                            || bytes[i] == '$')
                    {
                        i += 1;
                    }
                    let id: String = bytes[start..i].iter().collect();
                    let tok = names.get(&id).ok_or_else(|| {
                        format!("literal identifier '{id}' not registered as reserved word")
                    })?;
                    parts.push(tok.clone());
                    continue;
                }
                // 单字符标点 → token 名
                let tok = match c {
                    ',' => "Comma",
                    '[' => "LBracket",
                    ']' => "RBracket",
                    '(' => "LParen",
                    ')' => "RParen",
                    '+' => "Plus",
                    '-' => "Minus",
                    '*' => "Star",
                    '!' => "Bang",
                    ':' => "Colon",
                    _ => return Err(format!("unsupported literal char '{c}' in asm template")),
                };
                parts.push(tok.to_string());
                i += 1;
            }
            Ok(parts.join(" "))
        }
    }
}

/// 把 lalrpop 语法文本编译为 parser 代码字符串。
pub fn compile_grammar(
    isa_name: &str,
    grammar_text: &str,
    out_dir: &std::path::Path,
) -> Result<String, String> {
    let gen_dir = out_dir.join("forge_asm_gen").join(isa_name);
    std::fs::create_dir_all(&gen_dir).map_err(|e| format!("create gen dir: {e}"))?;
    let src_path = gen_dir.join("grammar.lalrpop");
    std::fs::write(&src_path, grammar_text).map_err(|e| format!("write grammar file: {e}"))?;

    let mut binding = lalrpop::Configuration::new();
    let cfg = binding
        .set_in_dir(gen_dir.clone())
        .set_out_dir(gen_dir.clone());
    cfg.process().map_err(|e| format!("lalrpop process: {e}"))?;

    let rs_path = gen_dir.join("grammar.rs");
    let code =
        std::fs::read_to_string(&rs_path).map_err(|e| format!("read generated parser: {e}"))?;
    Ok(code)
}

/// 保留字 → pascal 变体名（"mov" → "Mov"、"fadd.s" → "FaddS"）。
pub fn pascal_ident(s: &str) -> String {
    let mut out = String::new();
    let mut cap = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if cap {
                out.extend(c.to_uppercase());
                cap = false;
            } else {
                out.push(c);
            }
        } else {
            cap = true;
        }
    }
    if out.is_empty() {
        out = format!("Kw{}", s.len());
    }
    out
}

/// 转义字符串字面量。
pub fn escape_lit(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// 裁剪模板中的注释：`;;` 及其后内容（wasm 风格）忽略。
fn strip_comment(asm: &str) -> String {
    match asm.find(";;") {
        Some(pos) => asm[..pos].to_string(),
        None => asm.to_string(),
    }
}

// ============================================================
// 单元测试
// ============================================================
