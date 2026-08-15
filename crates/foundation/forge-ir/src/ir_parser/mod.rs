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

// lalrpop 生成的语法模块（build.rs 处理 grammar.lalrpop）——
// 生成代码标准豁免:type_complexity(4595 条,规则多字段元组)/未用绑定
// (变体参数全弃)/unreachable_patterns(多变体共享前缀)
lalrpop_mod!(
    #[allow(
        clippy::redundant_field_names,
        clippy::type_complexity,
        unused_variables,
        unreachable_patterns,
        clippy::too_many_arguments,
        unused_mut
    )]
    pub grammar,
    "/ir_parser/grammar.rs"
);
mod semantics;

// ============================================================
// Parse Error
// ============================================================

pub mod ast_items;
use crate::error::IrError;
use lalrpop_util::lalrpop_mod;

// ============================================================
// 公开 API（签名与旧解析器保持一致）
// ============================================================

/// 解析 LLVM IR module（可含 target/define/declare 多个函数）。
pub fn parse_module(source: &str) -> Result<crate::function::Module, IrError> {
    semantics::parse_module(source)
}

/// 解析单个 LLVM IR 函数定义。
pub fn parse_function(source: &str) -> Result<crate::function::Function, IrError> {
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
        assert_eq!(f.blocks[0].label, "entry");
        assert_eq!(f.blocks[0].insts.len(), 1);
        let inst = &f.blocks[0].insts[0];
        assert_eq!(inst.opcode, "add");
        assert_eq!(inst.result.as_deref(), Some("%s"));
        assert_eq!(inst.args.len(), 2);
        assert!(matches!(inst.args[0].op, Operand::Local(_)));
        assert!(matches!(
            f.blocks[0].terminator,
            ParsedTerminator::Return(..)
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
            ParsedTerminator::Branch(..)
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
            ParsedTerminator::Switch(..)
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
        assert_eq!(f.name.as_str(), "add");
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
        assert!(m.get_function(crate::entity::FuncRef(0)).name.as_str() == "add");
        assert!(m.get_function(crate::entity::FuncRef(1)).name.as_str() == "main");
    }

    #[test]
    fn ssa_duplicate_detected() {
        let r = parse_function(
            "define i32 @f() {\n  %entry:\n    %x = add i32 1, 2\n    %x = add i32 3, 4\n    ret i32 %x\n}\n",
        );
        assert!(r.is_err(), "duplicate %x should fail");
    }

    #[test]
    fn typed_operand_undefined_value_lenient() {
        // 第十一轮语义变更：未定义 % 值引用（use-list 测试的故意引用）宽松
        // 为 undef 占位——不再报错
        let r = parse_function(
            "define i32 @f() {\n  %entry:\n    %x = add i32 1, %undefined\n    ret i32 %x\n}\n",
        );
        assert!(
            r.is_ok(),
            "undefined %undefined should be lenient (undef placeholder)"
        );
    }

    #[test]
    fn declare_and_forward_ref() {
        // declare 外部函数 + define 函数前向引用 call（foo 定义在 main 之后）
        let m = parse_module(
            "declare i32 @puts(ptr %s)\n\
             define i32 @main() {\n  %entry:\n    %r = call i32 @foo()\n    %q = call i32 @puts(ptr null)\n    ret i32 %r\n}\n\
             define i32 @foo() {\n  %entry:\n    ret i32 42\n}\n",
        )
        .expect("parse");
        assert_eq!(m.function_count(), 3, "declare + 2 define");
        // 函数表顺序：puts(0), main(1), foo(2)——前向引用解析到 foo
        let names: Vec<String> = m.iter_functions().map(|f| f.name.to_string()).collect();
        assert_eq!(names, vec!["puts", "main", "foo"]);
        // declare 函数无 body（空壳）
        let decl = m.get_function(crate::entity::FuncRef(0));
        assert!(decl.layout.block_order.is_empty(), "declare has no body");
    }

    #[test]
    fn datalayout_parsed() {
        // target datalayout 真正解析：32 位布局 → 指针 4 字节
        let m = parse_module(
            "target datalayout = \"e-p:32:32-i64:64\"\n\
             define i32 @f() {\n  %entry:\n    ret i32 0\n}\n",
        )
        .expect("parse");
        assert_eq!(m.data_layout.pointer_size(0), 4, "p:32 → 4-byte pointer");
        // 关键：types 内嵌布局必须与 data_layout 同步（历史 bug：parse
        // 只写字段，types 仍默认 x86_64）——size/align 查询走 types
        assert_eq!(
            m.types.size_bytes(crate::TypeId::PTR),
            4,
            "TypeStore size_bytes must reflect parsed layout"
        );
        assert_eq!(m.types.alignment(crate::TypeId::PTR), 4);
        // display 应输出该 datalayout（非默认 x86_64）
        let text = format!("{}", m);
        assert!(
            text.contains("target datalayout"),
            "datalayout round-trips to text, got: {text}"
        );
    }

    #[test]
    fn declare_round_trip() {
        // parse（declare + define）→ display → 再 parse：declare 保持 declare
        let src = "declare i32 @puts(ptr %s)\n\
                   define i32 @main() {\n  %entry:\n    ret i32 0\n}\n";
        let m = parse_module(src).expect("parse");
        let text = format!("{}", m);
        assert!(text.contains("declare"), "declare preserved: {text}");
        let m2 = parse_module(&text).expect("re-parse");
        assert_eq!(m2.function_count(), 2, "declare + define round-trip");
    }

    fn first_vconst_bytes(func: &crate::function::Function) -> Vec<u8> {
        for inst in &func.dfg.insts {
            if matches!(inst.opcode, crate::Opcode::Vconst)
                && let Some(crate::Immediate::Const(cid)) = inst.immediates.first()
            {
                return func.constants.get_vector(*cid).unwrap_or(&[]).to_vec();
            }
        }
        Vec::new()
    }

    #[test]
    fn vconst_round_trip_bytes() {
        // 修复历史缺陷：vconst 数据曾静默丢失（parse 后变零向量）
        let src = "define <4 x f32> @v() {\n  %entry:\n    %v = vconst <4 x f32> [1.5, 2.5, -3.5, 4.25]\n    ret <4 x f32> %v\n}\n";
        let m = parse_module(src).expect("parse");
        let text = format!("{}", m);
        assert!(
            text.contains("<1.5, 2.5, -3.5, 4.25>"),
            "lane data in text: {text}"
        );
        let m2 = parse_module(&text).expect("re-parse");
        let f1 = m.get_function(crate::entity::FuncRef(0));
        let f2 = m2.get_function(crate::entity::FuncRef(0));
        let expect: Vec<u8> = [1.5f32, 2.5, -3.5, 4.25]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        assert_eq!(first_vconst_bytes(f1), expect, "source bytes");
        assert_eq!(first_vconst_bytes(f2), expect, "round-trip bytes");
    }

    #[test]
    fn vconst_round_trip_big_endian() {
        // 大端向量常量：display 输出 `big` 标记，round-trip 保留字节序
        let src = "define <2 x f64> @v() {\n  %entry:\n    %v = vconst <2 x f64> [1.5, 2.5] big\n    ret <2 x f64> %v\n}\n";
        let m = parse_module(src).expect("parse");
        let text = format!("{}", m);
        assert!(text.contains("big"), "endian marker in text: {text}");
        let m2 = parse_module(&text).expect("re-parse");
        let f1 = m.get_function(crate::entity::FuncRef(0));
        let f2 = m2.get_function(crate::entity::FuncRef(0));
        let expect: Vec<u8> = [1.5f64, 2.5].iter().flat_map(|x| x.to_be_bytes()).collect();
        assert_eq!(first_vconst_bytes(f1), expect, "source big-endian bytes");
        assert_eq!(
            first_vconst_bytes(f2),
            expect,
            "round-trip big-endian bytes"
        );
    }

    #[test]

    fn global_round_trip() {
        // global/constant 定义 + 函数内 @g 直接引用（LLVM 标准，无 global_addr
        // 指令行）+ display round-trip
        let src = "@g = global i32 42\n\
                   @c = constant f32 1.5\n\
                   @n = global ptr\n\
                   @a = global i64 -7, align 16\n\
                   define i32 @main() {\n  %entry:\n    %p = load i32, ptr @g\n    ret i32 %p\n}\n";
        let m = parse_module(src).expect("parse");
        assert_eq!(m.global_count(), 4, "4 globals");
        let text = format!("{}", m);
        for frag in [
            "@g = global i32 42",
            "@c = constant f32 1.5",
            "@n = global ptr",
            "align 16",
            "load i32, ptr @g",
        ] {
            assert!(text.contains(frag), "missing {frag:?} in: {text}");
        }
        let m2 = parse_module(&text).expect("re-parse");
        assert_eq!(m2.global_count(), 4, "globals survive round-trip");
        for (id, gv) in m2.iter_globals() {
            let g1 = m.get_global(id).expect("same id");
            assert_eq!(g1.name, gv.name, "name");
            assert_eq!(g1.is_constant, gv.is_constant, "constness");
            assert_eq!(g1.alignment, gv.alignment, "alignment");
            assert_eq!(g1.init, gv.init, "init bytes");
        }
    }
}
