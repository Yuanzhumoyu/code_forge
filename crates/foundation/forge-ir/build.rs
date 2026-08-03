fn main() {
    // 用 lalrpop 生成 LLVM IR 语法解析器（src/ir_parser/grammar.lalrpop → grammar.rs）
    // 生成代码的 clippy 豁免见 mod.rs 的 lalrpop_mod!(#[allow(...)] ...) 宏参数。
    lalrpop::process_root().unwrap();
}
