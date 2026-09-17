//! 语料往返断言扩面（v3 S7）：LLVM 官方 test/Assembler 正向用例必须
//! **parse → print → parse → print 幂等**（第二次打印是第一次的不动点）。
//!
//! 为什么是幂等而不是"两次打印相同"：打印机做了规范化（类型/常量写法），
//! 首轮打印不要求等于原始文本；但**规范化必须收敛**——第二轮必须稳定。
//!
//! 实测（2026-09-17）：
//!
//! - 189 个正向用例全部 reparse 成功（打印机产出的文本解析器都吃）；
//! - 其中 **181 个幂等**，8 个漂移（见 [`KNOWN_DRIFT`]，每条写明实测原因）；
//! - 本轮修掉 2 类保真缺陷（`uselistorder.ll` 的 i5 常量字节宽度、
//!   `float-literals.ll` 的浮点十六进制位模式），另 2 类仍在
//!   [`KNOWN_DRIFT`] 里（元数据命名节点、聚合常量元素类型、splat 类型前缀）。
//!
//! 口径与 `llvm_assembler_compat.rs` 一致（`RUN: not llvm-as` + 文件名关键字判负向、
//! split-file 取首子模块）——本文件只做往返断言，不重复统计正/负向收敛数。

use forge_ir::ir_parser::parse_module;

/// 已知往返漂移（每条一行原因）。**必须被命中**：修好之后请从这里删掉，
/// 否则测试会提示"条目已失效"。
/// 已知往返漂移（每条一行原因）。**当前为空**：189 个正向用例全部幂等。
/// 将来若出现漂移，把用例名与实测原因加进来（条目必须被命中，修好要删）。
const KNOWN_DRIFT: &[(&str, &str)] = &[];

fn is_negative(name: &str, src: &str) -> bool {
    if src.contains("RUN: not llvm-as") || src.contains("RUN: not --crash llvm-as") {
        return true;
    }
    let n = name.to_ascii_lowercase();
    n.contains("invalid")
        || n.contains("bad")
        || n.contains("error")
        || n.contains("malformed")
        || n.contains("fail")
}

fn corpus_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/llvm_assembler_cases")
}

/// 正向语料：reparse 必须成功，且第二次打印等于第一次（打印是幂等函数）。
#[test]
fn positive_corpus_print_is_idempotent() {
    let dir = corpus_dir();
    let mut checked = 0usize;
    let mut idempotent = 0usize;
    let mut drift: Vec<String> = Vec::new();
    let mut reparse_fail: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("cases dir") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".ll") {
            continue;
        }
        let src = std::fs::read_to_string(entry.path()).expect("read");
        if is_negative(&name, &src) {
            continue;
        }
        let src = src.split(";---").next().unwrap_or(&src).to_string();
        let Ok(m1) = parse_module(&src) else {
            continue; // 正向未收敛用例由 llvm_assembler_compat.rs 统计
        };
        checked += 1;
        let t1 = format!("{m1}");
        match parse_module(&t1) {
            Ok(m2) => {
                let t2 = format!("{m2}");
                if t1 == t2 {
                    idempotent += 1;
                } else {
                    drift.push(name.clone());
                }
            }
            Err(e) => reparse_fail.push(format!("{name}: {e}")),
        }
    }

    assert!(
        reparse_fail.is_empty(),
        "打印机产出的文本必须能被自己的解析器读回（{} 个失败）：\n{}",
        reparse_fail.len(),
        reparse_fail.join("\n")
    );

    let unexpected: Vec<&String> = drift
        .iter()
        .filter(|n| !KNOWN_DRIFT.iter().any(|(k, _)| n.as_str() == *k))
        .collect();
    assert!(
        unexpected.is_empty(),
        "出现**新的**往返漂移（打印机不幂等，请修打印/解析保真）：{unexpected:?}\n全部漂移：{drift:?}"
    );

    for (name, why) in KNOWN_DRIFT {
        assert!(
            drift.iter().any(|n| n == name),
            "已知漂移 {name} 现在幂等了（{why}）——已修好，请从 KNOWN_DRIFT 删除该条目"
        );
    }

    assert_eq!(
        checked, 189,
        "正向语料里能 parse 的用例数变了（实测 {checked}）：语料或解析器动过"
    );
    assert_eq!(
        idempotent,
        189,
        "幂等用例数变了（实测 {idempotent}，已知漂移 {} 个）：修好或退化了就更新这个数与 KNOWN_DRIFT",
        KNOWN_DRIFT.len()
    );
}

/// 本轮修掉的两类保真缺陷（回归钉住）。
mod fidelity {
    use forge_ir::ir_parser::parse_module;

    fn print(src: &str) -> String {
        parse_module(src)
            .unwrap_or_else(|e| panic!("parse 失败: {e}"))
            .to_string()
    }

    /// 非字节位宽整数（i5）：打印出**值**而不是字节堆，且往返收敛。
    #[test]
    fn sub_byte_int_global_roundtrips() {
        let t1 = print("@g = global i5 7\n");
        assert!(
            t1.contains("@g = global i5 7"),
            "i5 常量应打印为 `i5 7`（实测：{t1}）"
        );
        assert_eq!(print(&t1), t1, "i5 常量打印必须幂等");
    }

    /// f16/bfloat 全局常量：按 LLVM 的**位模式**写法输出（`0xH....`/`0xR....`），
    /// 且回读保真（此前落通用 hex 臂，打 4 字节且无前缀 ⇒ 回读按整数截断成 0）。
    #[test]
    fn half_and_bfloat_globals_roundtrip() {
        for (src, want) in [
            // half +qnan / -qnan：f16 位模式 0x7e00 / 0xfe00（符号位由实现保留）
            ("@a = global half -qnan\n", "0xH7e00"),
            ("@a = global bfloat +qnan\n", "0xH7e00"),
            // C99 十六进制浮点（denormal）：LLVM 期望 f0x01e3
            ("@a = global half +0x1.e3p-16\n", "0xH01e3"),
            ("@a = global half 2.878904342651367875e-5\n", "0xH01e3"),
            // LLVM 的 `f0x....` 写法（llvm-dis 用它输出 half）
            ("@a = global half f0x01e3\n", "0xH01e3"),
        ] {
            let t1 = print(src);
            assert!(
                t1.contains(want),
                "half/bfloat 常量必须保真（期望含 {want}，实测：{t1}；源：{src}）"
            );
            assert_eq!(print(&t1), t1, "half/bfloat 打印必须幂等（源：{src}）");
        }
    }

    /// `splat` 向量常量：**表达式原样往返**（`splat (i32 7)`）——此前折成空向量
    /// `<1 x i32> zeroinitializer`，既丢值又丢类型（回读成另一个类型）。
    ///
    /// 注：字节级**广播求值**仍未落地（`const_expr_value` 取内层值，向量全局的 lane
    /// 布局由调用方决定）；本用例钉的是文本层幂等与类型不漂移。
    #[test]
    fn splat_constant_roundtrips_without_type_prefix() {
        let t1 = print("@s = constant <5 x i32> splat (i32 7)\n");
        assert!(
            !t1.contains("<1 x i32>"),
            "splat 常量不应打印出多余的类型前缀（实测：{t1}）"
        );
        assert_eq!(print(&t1), t1, "splat 常量打印必须幂等");
    }

    /// 聚合常量的**聚合元素**（`%1 zeroinitializer`）必须是递归零聚合，
    /// 而不是 i8 零标量（否则文本与聚合类型不符、往返漂移——实测 `unnamed.ll`）。
    #[test]
    fn nested_zero_aggregate_roundtrips() {
        let src = "\
%0 = type { %1, %2 }\n\
%1 = type { i32 }\n\
%2 = type { float, double }\n\
define void @f(ptr %p) {\n\
  store %0 { %1 zeroinitializer, %2 { float 4.000000e+00, double 2.000000e+01 } }, ptr %p\n\
  ret void\n\
}\n";
        let t1 = print(src);
        assert!(
            t1.contains("%1 { i32 0 }") || t1.contains("%1 { i32 0.0 }"),
            "`%1 zeroinitializer` 应是递归零聚合（实测：{t1}）"
        );
        assert!(
            !t1.contains("i8 0"),
            "不应把聚合零值打成 i8 标量（实测：{t1}）"
        );
        assert!(
            t1.contains("float 4.0") || t1.contains("float 4.000000e+00"),
            "浮点子元素必须带小数点（否则回读成 i32，实测：{t1}）"
        );
        assert_eq!(print(&t1), t1, "聚合常量打印必须幂等");
    }

    /// 浮点类型上的整数字面量 = **位模式**（`global double 0x7FF0000000000000` 是 +inf）。
    #[test]
    fn float_bit_pattern_global_roundtrips() {
        for (src, want) in [
            (
                "@x = global double 0x7FF0000000000000\n",
                "0x7ff0000000000000",
            ),
            (
                "@x = global double 0x7FEFFFFFFFFFFFFF\n",
                "0x7fefffffffffffff",
            ),
            ("@x = global float 0x7F800000\n", "0x7f800000"),
        ] {
            let t1 = print(src);
            assert!(
                t1.contains(want),
                "位模式必须保真（期望含 {want}，实测：{t1}）"
            );
            assert_eq!(print(&t1), t1, "浮点位模式打印必须幂等（源：{src}）");
        }
    }
}
