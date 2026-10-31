//! v30 M0：**生成码金标** —— 后端重构前后必须逐字节相同。
//!
//! 这套夹具的唯一用途是给"纯人体工学重构"设一条不会说话的护栏：`CompileOptions`、
//! 阶段驱动、合法化批次外提、JIT 拆文件这些改动都不许改动机器码。差异出现时，
//! 要么是重构真的改了行为（必须解释并补用例），要么是夹具本身被误改。
//!
//! ## 为什么只收标量用例
//!
//! 后端的宽向量能力是"谱申报档位 ∩ 宿主 CPU 能力"（`wide_vector_bytes()`）——向量
//! 用例的字节会随跑测试的机器变化。金标只收宿主无关的标量形态：整数/浮点算术、
//! 分支、多参数（迫使溢出）、直接调用。
//!
//! ## 更新夹具
//!
//! ```text
//! FORGE_UPDATE_GOLDEN=1 cargo test -p forge-codegen --test golden_codegen
//! ```
//!
//! 只允许在**确认字节差异有理由**时使用；更新后必须人工 review `tests/fixtures/golden/`。

use std::collections::BTreeMap;
use std::path::PathBuf;

use forge_codegen::CompileSession;
use forge_codegen::machine::target::TargetMachine;
use forge_ir::{
    FuncRef, Function, FunctionBuilder, FunctionSignature, InstFlags, IntCC, Module, TypeContext,
    TypeId,
};

/// 金标用例（顺序即夹具文件里的顺序）。
const CASES: &[&str] = &[
    "ret_i64",
    "add_i64",
    "branch",
    "many_args",
    "direct_call",
    "f64_add",
];

fn fixture_path(isa: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/golden")
        .join(format!("{isa}.txt"))
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// 单函数用例（不含跨函数调用）。
fn build_single(case: &str) -> Function {
    match case {
        "ret_i64" => {
            let sig = FunctionSignature::new(&[], &[TypeId::I64]);
            let mut b = FunctionBuilder::new(case, TypeContext::new(), sig);
            b.create_block_here();
            let v = b.const_int(7i64, TypeId::I64);
            b.ret(&[v]);
            b.finish().expect("build ret_i64")
        }
        "add_i64" => {
            let sig =
                FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
            let mut b = FunctionBuilder::new(case, TypeContext::new(), sig);
            let (entry, p) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
            b.switch_to_block(entry);
            let v = b.iadd(p[0], p[1]);
            b.ret(&[v]);
            b.finish().expect("build add_i64")
        }
        "branch" => {
            let sig =
                FunctionSignature::new(&[(TypeId::I64, "a"), (TypeId::I64, "b")], &[TypeId::I64]);
            let mut b = FunctionBuilder::new(case, TypeContext::new(), sig);
            let (entry, p) = b.create_block_with_params(&[(TypeId::I64, "a"), (TypeId::I64, "b")]);
            b.switch_to_block(entry);
            let cond = b.icmp(IntCC::Slt, p[0], p[1]);
            let then_b = b.create_block();
            let else_b = b.create_block();
            b.branch(cond, then_b, &[], else_b, &[]);
            b.switch_to_block(then_b);
            b.ret(&[p[0]]);
            b.switch_to_block(else_b);
            b.ret(&[p[1]]);
            b.finish().expect("build branch")
        }
        "many_args" => {
            let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
            let params: Vec<(TypeId, &str)> = names.iter().map(|n| (TypeId::I64, *n)).collect();
            let sig = FunctionSignature::new(&params, &[TypeId::I64]);
            let mut b = FunctionBuilder::new(case, TypeContext::new(), sig);
            let (entry, p) = b.create_block_with_params(&params);
            b.switch_to_block(entry);
            let mut acc = p[0];
            for v in &p[1..] {
                acc = b.iadd(acc, *v);
            }
            b.ret(&[acc]);
            b.finish().expect("build many_args")
        }
        "f64_add" => {
            let sig =
                FunctionSignature::new(&[(TypeId::F64, "a"), (TypeId::F64, "b")], &[TypeId::F64]);
            let mut b = FunctionBuilder::new(case, TypeContext::new(), sig);
            let (entry, p) = b.create_block_with_params(&[(TypeId::F64, "a"), (TypeId::F64, "b")]);
            b.switch_to_block(entry);
            let v = b.fadd_with_flags(p[0], p[1], InstFlags::NONE);
            b.ret(&[v]);
            b.finish().expect("build f64_add")
        }
        other => panic!("未知金标用例：{other}"),
    }
}

/// `callee(i64) -> i64` + `caller() -> i64`：覆盖直接调用的 lowering 与重定位占位字节。
fn build_call_module() -> Module {
    let mut m = Module::new();

    let sig_c = FunctionSignature::new(&[(TypeId::I64, "x")], &[TypeId::I64]);
    let mut bc = FunctionBuilder::new("callee", TypeContext::new(), sig_c);
    let (blk, p) = bc.create_block_with_params(&[(TypeId::I64, "x")]);
    bc.switch_to_block(blk);
    let one = bc.const_int(1i64, TypeId::I64);
    let sum = bc.iadd(p[0], one);
    bc.ret(&[sum]);
    m.add_function(bc.finish().expect("build callee"));

    let sig_m = FunctionSignature::new(&[], &[TypeId::I64]);
    let mut bm = FunctionBuilder::new("caller", TypeContext::new(), sig_m);
    bm.create_block_here();
    let five = bm.const_int(5i64, TypeId::I64);
    let r = bm.call(FuncRef::new(0), &[five], &[TypeId::I64]);
    bm.ret(&[r[0]]);
    m.add_function(bm.finish().expect("build caller"));
    m
}

/// 编译一个用例，返回该用例要进金标的机器码。
fn code_for<M: TargetMachine + Clone>(tm: M, case: &str) -> Vec<u8> {
    if case == "direct_call" {
        let m = build_call_module();
        let compiled = CompileSession::for_module(tm, &m)
            .compile_module(&m)
            .expect("编译 direct_call 模块");
        return compiled
            .function("caller")
            .expect("模块里应有 caller")
            .code
            .clone();
    }
    let func = build_single(case);
    CompileSession::new(tm)
        .compile(&func)
        .expect("编译金标用例")
        .code
}

fn actual_for<M: TargetMachine + Clone>(tm: M) -> BTreeMap<String, String> {
    CASES
        .iter()
        .map(|case| ((*case).to_string(), to_hex(&code_for(tm.clone(), case))))
        .collect()
}

fn load_fixture(isa: &str) -> BTreeMap<String, String> {
    let path = fixture_path(isa);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "读不到金标夹具 {}：{e}（先跑 FORGE_UPDATE_GOLDEN=1）",
            path.display()
        )
    });
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (case, hex) = line
            .split_once('\t')
            .unwrap_or_else(|| panic!("夹具行缺少 TAB 分隔：{line}"));
        out.insert(case.to_string(), hex.trim().to_string());
    }
    out
}

fn save_fixture(isa: &str, actual: &BTreeMap<String, String>) {
    let path = fixture_path(isa);
    let mut text = String::from(
        "# v30 生成码金标（FORGE_UPDATE_GOLDEN=1 生成；只允许在确认字节差异有理由时更新）\n",
    );
    for (case, hex) in actual {
        text.push_str(case);
        text.push('\t');
        text.push_str(hex);
        text.push('\n');
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("创建夹具目录");
    }
    std::fs::write(&path, text).expect("写夹具");
}

fn first_diff(a: &str, b: &str) -> Option<usize> {
    a.bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .or_else(|| {
            if a.len() == b.len() {
                None
            } else {
                Some(a.len().min(b.len()))
            }
        })
}

fn check_isa<M: TargetMachine + Clone>(isa: &str, tm: M) {
    let actual = actual_for(tm);
    if std::env::var("FORGE_UPDATE_GOLDEN").as_deref() == Ok("1") {
        save_fixture(isa, &actual);
        eprintln!("[{isa}] 已写入 {} 条金标", actual.len());
        return;
    }
    let expected = load_fixture(isa);
    assert_eq!(
        expected.keys().collect::<Vec<_>>(),
        actual.keys().collect::<Vec<_>>(),
        "[{isa}] 金标用例集合与夹具不一致（新增/删除了用例？先更新夹具并 review）"
    );
    for case in CASES {
        let want = &expected[*case];
        let got = &actual[*case];
        assert!(!got.is_empty(), "[{isa}/{case}] 编译产物为空——金标没有意义");
        if want != got {
            let at = first_diff(want, got).unwrap_or(0);
            panic!(
                "[{isa}/{case}] 机器码变了（首个差异在 hex 字符 {at}）\n  期望: {want}\n  实际: {got}\n\
                 若这是有意行为变化：先解释差异，再 FORGE_UPDATE_GOLDEN=1 更新并 review。"
            );
        }
    }
}

#[test]
fn golden_x86() {
    check_isa("x86", forge_codegen::x86::TargetMachine::new());
}

#[test]
fn golden_arm64() {
    check_isa("arm64", forge_codegen::arm64::TargetMachine::new());
}

#[test]
fn golden_riscv64() {
    check_isa("riscv64", forge_codegen::riscv64::TargetMachine::new());
}
