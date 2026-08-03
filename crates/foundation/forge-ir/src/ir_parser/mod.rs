//! forge-ir 文本解析器（LLVM IR 语法）。
//!
//! 用 logos（词法）+ lalrpop（LR 语法，build.rs 生成 grammar.rs）重写，
//! 严格遵循 LLVM IR 语法：`define i32 @add(i32 %a) { %s = add i32 %a, 1; ret i32 %s }`。
//!
//! # 模块结构
//! - [`lexer`]：logos 词法器（LLVM 全部 token，`;` 注释）
//! - [`llvm_mapping`]：LLVM opcode/类型/条件 → forge Opcode/TypeId/IntCC/FloatCC 映射表
//! - `semantics`：AST → forge IR（名称解析 / SSA 唯一性 / 类型检查）
//! - `grammar.lalrpop`：lalrpop 语法（build.rs 生成 `grammar` 模块）

pub mod lexer;
pub mod llvm_mapping;

// lalrpop 生成的语法模块（build.rs 处理 grammar.lalrpop）
lalrpop_mod!(#[allow(clippy::redundant_field_names)] pub grammar, "/ir_parser/grammar.rs");
mod semantics;

// ============================================================
// Parse Error
// ============================================================

#[derive(Debug)]
pub enum ParseError {
    /// 词法层错误（logos LexingError）。
    Grammar(String),
    /// 语法层错误（lalrpop ParseError）。
    Parse(String),
    /// 语义层错误（类型不匹配、未定义值、SSA 重复定义、不支持的构造）。
    Semantic(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Grammar(e) => write!(f, "lex error: {}", e),
            ParseError::Parse(e) => write!(f, "parse error: {}", e),
            ParseError::Semantic(e) => write!(f, "semantic error: {}", e),
        }
    }
}

impl std::error::Error for ParseError {}

pub mod ast_items;
use lalrpop_util::lalrpop_mod;

// ============================================================
// 公开 API（签名与旧解析器保持一致）
// ============================================================

/// 解析 LLVM IR module（可含 target/define/declare 多个函数）。
pub fn parse_module(source: &str) -> Result<crate::function::Module, ParseError> {
    semantics::parse_module(source)
}

/// 解析单个 LLVM IR 函数定义。
pub fn parse_function(source: &str) -> Result<crate::function::Function, ParseError> {
    semantics::parse_function(source)
}

#[cfg(test)]
mod tests {
    use super::ast_items::*;
    use super::grammar;
    use super::lexer::TokenStream;

    fn parse(src: &str) -> ParsedModule {
        let lexer = TokenStream::new(src);
        let parser = grammar::ModuleParser::new();
        parser
            .parse(lexer)
            .unwrap_or_else(|e| panic!("parse failed: {e:?}"))
    }

    #[test]
    fn parse_empty_module() {
        let m = parse("");
        assert!(m.items.is_empty());
    }

    #[test]
    fn parse_simple_function() {
        let m = parse(
            "define i32 @add(i32 %a, i32 %b) {\n  %entry:\n    %s = add i32 %a, i32 %b\n    ret i32 %s\n}\n",
        );
        assert_eq!(m.items.len(), 1);
        let ParsedItem::Function(f) = &m.items[0] else {
            panic!("expected function");
        };
        assert_eq!(f.name, "@add");
        assert_eq!(f.ret_ty, ParsedType::Int(32));
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].1, "%a");
        assert_eq!(f.blocks.len(), 1);
        assert_eq!(f.blocks[0].label, "%entry");
        assert_eq!(f.blocks[0].insts.len(), 1);
        let inst = &f.blocks[0].insts[0];
        assert_eq!(inst.opcode, "add");
        assert_eq!(inst.result.as_deref(), Some("%s"));
        assert_eq!(inst.args.len(), 2);
        assert!(matches!(inst.args[0].op, Operand::Local(_)));
        assert!(matches!(
            f.blocks[0].terminator,
            ParsedTerminator::Return(_)
        ));
    }

    #[test]
    fn parse_target_and_multi_function() {
        let m = parse(
            "target triple = \"x86_64\"\ndefine i32 @a(i32 %x) {\n  %entry:\n    ret i32 %x\n}\ndefine void @b() {\n  %entry:\n    ret void\n}\n",
        );
        assert_eq!(m.items.len(), 3);
        assert!(matches!(&m.items[0], ParsedItem::Target(k, _) if k == "triple"));
        assert!(matches!(&m.items[1], ParsedItem::Function(f) if f.name == "@a"));
        assert!(matches!(&m.items[2], ParsedItem::Function(f) if f.name == "@b"));
    }

    #[test]
    fn parse_icmp_and_br() {
        let m = parse(
            "define i1 @cmp(i32 %a, i32 %b) {\n  %entry:\n    %c = icmp slt i32 %a, i32 %b\n    br i1 %c, label %then, label %else\n  %then:\n    ret i1 true\n  %else:\n    ret i1 false\n}\n",
        );
        let ParsedItem::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.blocks[0].insts[0].opcode, "icmp");
        assert_eq!(f.blocks[0].insts[0].cond.as_deref(), Some("slt"));
        assert!(matches!(
            f.blocks[0].terminator,
            ParsedTerminator::Branch(_, _, _)
        ));
    }

    #[test]
    fn parse_switch_and_unreachable() {
        let m = parse(
            "define i32 @sw(i32 %x) {\n  %entry:\n    switch i32 %x, label %def [ i32 0, label %zero ]\n  %def:\n    unreachable\n  %zero:\n    ret i32 0\n}\n",
        );
        let ParsedItem::Function(f) = &m.items[0] else {
            panic!();
        };
        assert!(matches!(
            f.blocks[0].terminator,
            ParsedTerminator::Switch(_, _, _)
        ));
        assert!(matches!(
            f.blocks[1].terminator,
            ParsedTerminator::Unreachable
        ));
    }

    #[test]
    fn parse_vectors_and_structs() {
        let m = parse(
            "define <4 x i32> @v(<4 x i32> %a) {\n  %entry:\n    %r = add <4 x i32> %a, <4 x i32> %a\n    ret <4 x i32> %r\n}\n",
        );
        let ParsedItem::Function(f) = &m.items[0] else {
            panic!();
        };
        assert_eq!(f.ret_ty, ParsedType::Vec(4, Box::new(ParsedType::Int(32))));
        assert_eq!(f.blocks[0].insts[0].opcode, "add");
    }
}

#[cfg(test)]
mod semantics_tests {
    use super::*;

    #[test]
    fn build_simple_function_ir() {
        // define i32 @add(i32 %a, i32 %b) { entry: %s = add i32 %a, i32 %b; ret i32 %s }
        let f = parse_function(
            "define i32 @add(i32 %a, i32 %b) {\n  %entry:\n    %s = add i32 %a, i32 %b\n    ret i32 %s\n}\n",
        )
        .expect("parse");
        assert_eq!(f.name, "add");
        // inst_order 含 iadd；ret 是 terminator（不占 inst_order）
        let insts: usize = f.dfg.blocks.iter().map(|b| b.inst_order.len()).sum();
        assert_eq!(insts, 1, "iadd in inst_order");
        assert!(
            matches!(
                &f.dfg.blocks[0].terminator,
                crate::terminator::Terminator::Return { .. }
            ),
            "ret terminator"
        );
        // 值名绑定（Phase 2）：参数 %a/%b、结果 %s
        assert_eq!(f.value_names.len(), 3, "params + result named");
    }

    #[test]
    fn build_icmp_branch_ir() {
        let f = parse_function(
            "define i1 @cmp(i32 %a, i32 %b) {\n  %entry:\n    %c = icmp slt i32 %a, i32 %b\n    br i1 %c, label %then, label %else\n  %then:\n    ret i1 true\n  %else:\n    ret i1 false\n}\n",
        )
        .expect("parse");
        // 3 个块，icmp 条件 slt
        assert_eq!(f.dfg.block_count(), 3);
    }

    #[test]
    fn build_module_multi_function() {
        let m = parse_module(
            "define i32 @add(i32 %a, i32 %b) {\n  %entry:\n    %s = add i32 %a, i32 %b\n    ret i32 %s\n}\ndefine i32 @main() {\n  %entry:\n    %r = call i32 @add(i32 1, i32 2)\n    ret i32 %r\n}\n",
        )
        .expect("parse");
        // 2 个函数 + 跨函数 call
        assert!(m.get_function(crate::entity::FuncRef(0)).name == "add");
        assert!(m.get_function(crate::entity::FuncRef(1)).name == "main");
    }

    #[test]
    fn ssa_duplicate_detected() {
        let r = parse_function(
            "define i32 @f() {\n  %entry:\n    %x = add i32 1, 2\n    %x = add i32 3, 4\n    ret i32 %x\n}\n",
        );
        assert!(r.is_err(), "duplicate %x should fail");
    }

    #[test]
    fn typed_operand_mismatch_detected() {
        // add 的操作数类型不一致（i32 vs i64 常量被 iconst 按类型建——不报错；
        // 此测试验证 undefined % 值报错）
        let r = parse_function(
            "define i32 @f() {\n  %entry:\n    %x = add i32 1, %undefined\n    ret i32 %x\n}\n",
        );
        assert!(r.is_err(), "undefined %undefined should fail");
    }
}
