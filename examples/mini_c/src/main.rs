//! Mini C compiler — CLI entry point.
//!
//! Usage: mini_c <source.c> [--dump-ast]
//!
//! Reads a Mini C source file, JIT-compiles it to x86_64 machine code,
//! calls `main()`, and prints the return value.

use std::env;
use std::fs;
use std::process;

use mini_c::compiler;

fn main() {
    let args: Vec<String> = env::args().collect();

    // Separate flags from positional arguments
    let dump_ast = args.iter().any(|a| a == "--dump-ast");
    let positional: Vec<&String> = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .collect();

    if positional.is_empty() {
        eprintln!("Usage: mini_c <source.c> [--dump-ast]");
        eprintln!();
        eprintln!("  Reads a Mini C source file, compiles it to x86_64 machine code,");
        eprintln!("  calls main(), and prints the return value.");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  --dump-ast   Print the lowered AST tree structure");
        process::exit(1);
    }

    let source_path = positional[0];

    let source = match fs::read_to_string(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error reading '{}': {}", source_path, e);
            process::exit(1);
        }
    };

    if dump_ast {
        match compiler::dump_ast(&source) {
            Ok(tree) => {
                println!("=== AST ===\n{}", tree);
            }
            Err(e) => {
                eprintln!("error dumping AST: {}", e);
                process::exit(1);
            }
        }
    }

    match compiler::compile_and_run(&source) {
        Ok(result) => {
            println!("=> main() returned {}", result);
        }
        Err(e) => {
            eprintln!("error: {}", e);
            process::exit(1);
        }
    }
}
