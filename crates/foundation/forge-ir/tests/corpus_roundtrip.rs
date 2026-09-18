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

use forge_ir::text::parser::parse_module;

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
    use forge_ir::text::parser::parse_module;

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
    #[test]
    fn splat_constant_roundtrips_without_type_prefix() {
        let t1 = print("@s = constant <5 x i32> splat (i32 7)\n");
        assert!(
            !t1.contains("<1 x i32>"),
            "splat 常量不应打印出多余的类型前缀（实测：{t1}）"
        );
        assert_eq!(print(&t1), t1, "splat 常量打印必须幂等");
    }

    /// 向量**元素类型名**必须按 LLVM 写法识别（v3 S7）。
    ///
    /// 改前 `VecTy` 的 lexer 动作按首字母猜（`strip_prefix('i')/('b')/('f')`，其余落
    /// `Ptr`）：`float`→`f64`（`"loat"` 解析失败取兜底 64）、`half`/`double`→`ptr`、
    /// `bfloat`→`i32`、`fp128`→`f64`——**静默类型损坏**，而往返测试只比对 m1/m2 两边
    /// （两边错得一样）所以看不出来。这里按**打印形态**钉住（用户可见的口径）。
    ///
    /// 注：`x86_fp80` 沿用既有标量约定映射为 `f128`；`bfloat` 向量元素与 `half` 同
    /// `Float(16)`（标量 bfloat 另有 `BFloat` 条目，向量元素尚未区分——如实记录现状）。
    #[test]
    fn vector_element_type_names_are_not_guessed() {
        for (elem, want) in [
            ("float", "f32"),
            ("double", "f64"),
            ("half", "f16"),
            ("bfloat", "f16"),
            ("fp128", "f128"),
            ("x86_fp80", "f128"),
            ("ptr", "ptr"),
            ("i33", "i33"),
            ("b7", "i7"),
        ] {
            let src = format!("@v = external global <2 x {elem}>\n");
            let t1 = print(&src);
            assert!(
                t1.contains(&format!("<2 x {want}>")),
                "`<2 x {elem}>` 应打印为 `<2 x {want}>`（实测：{t1}）"
            );
            assert_eq!(print(&t1), t1, "向量类型打印必须幂等（元素 {elem}）");
        }
    }

    ///
    /// 此前 `ConstExpr::Splat` 在求值口返回宽松 0 ⇒ `@s = global <4 x i32> splat (i32 7)`
    /// 的 lane 全是 0（文本往返看不出，字节才是判据）。内层类型与向量元素类型不符、
    /// 或类型不是向量，都必须报错。
    #[test]
    fn splat_global_broadcasts_lane_value() {
        let want: Vec<u8> = [7u32, 7, 7, 7]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let src = "@s = global <4 x i32> splat (i32 7)\n";
        let m = parse_module(src).expect("parse");
        let (_, g) = m.iter_globals().next().expect("global");
        assert_eq!(g.init.as_deref(), Some(&want[..]), "lane 必须广播该值");
        // 打印-回读后字节不变（表达式原样往返 + 广播一致）
        let t1 = m.to_string();
        assert!(t1.contains("splat (i32 7)"), "表达式应原样打印：{t1}");
        let m2 = parse_module(&t1).expect("reparse");
        let (_, g2) = m2.iter_globals().next().expect("global");
        assert_eq!(g2.init.as_deref(), Some(&want[..]), "回读后 lane 仍须广播");
        // 非零标量亦可（i64 元素）
        let want64: Vec<u8> = [(-1i64), -1].iter().flat_map(|v| v.to_le_bytes()).collect();
        let m3 = parse_module("@s = global <2 x i64> splat (i64 -1)\n").expect("parse");
        let (_, g3) = m3.iter_globals().next().expect("global");
        assert_eq!(g3.init.as_deref(), Some(&want64[..]));
        // 非法：内层类型与元素类型不符 / 用在非向量类型上
        for bad in [
            "@s = global <4 x i32> splat (i8 7)\n",
            "@s = global i32 splat (i32 7)\n",
        ] {
            assert!(
                parse_module(bad).is_err(),
                "非法 splat 初值必须被拒绝（源：{bad}）"
            );
        }
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

    /// 向量字面量全局初值（`global <4 x i32> <i32 1, …>`）：**lane 值必须进字节**。
    ///
    /// 改前实测两处都不行：①语法层把 `<N x T> <lanes>` 整段当一个 `VecConstLit`
    /// token，`TypeAndInit` 只拆了 `zeroinitializer` 两种形态 ⇒ 这种全局**直接解析
    /// 失败**；②`parse_vec_lanes` 把整段 `i32 3` 喂给 `parse::<i64>()`（前缀必然使
    /// 解析失败）⇒ 即便能进也**每个 lane 静默变 0**。
    #[test]
    fn vector_literal_global_keeps_lane_values() {
        let src = "@v = global <4 x i32> <i32 1, i32 2, i32 3, i32 4>\n";
        let m = parse_module(src).expect("向量字面量初值应可解析");
        let (_, g) = m.iter_globals().next().expect("global");
        assert_eq!(
            g.init.as_deref(),
            Some(&[1u8, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0][..]),
            "lane 值必须按元素宽度 LE 进字节（不是全 0）"
        );
        let t1 = m.to_string();
        assert!(
            t1.contains("<i32 1, i32 2, i32 3, i32 4>"),
            "lane 值必须原样打印（实测：{t1}）"
        );
        assert_eq!(print(&t1), t1, "向量初值打印必须幂等");
        // 元素类型前缀取自向量类型（不是写死 i32）：i16 向量 lane 打 i16
        let t2 = print("@w = global <4 x i16> <i16 -1, i16 -1, i16 -1, i16 -1>\n");
        assert!(
            t2.contains("<i16 -1, i16 -1, i16 -1, i16 -1>"),
            "i16 向量的 lane 前缀必须是 i16（实测：{t2}）"
        );
        assert_eq!(print(&t2), t2, "i16 向量初值打印必须幂等");
    }

    /// 向量初值的非法形态必须**报错**（fail-closed，不静默补零）：lane 数不符、
    /// lane 值类别与元素类型不符。
    #[test]
    fn malformed_vector_initializer_is_rejected() {
        for src in [
            // lane 数不足
            "@v = global <4 x i32> <i32 1, i32 2>\n",
            // 浮点元素配整数 lane
            "@v = global <2 x float> <i32 1, i32 2>\n",
        ] {
            assert!(
                parse_module(src).is_err(),
                "非法向量初值必须被拒绝（源：{src}）"
            );
        }
    }

    /// `c"…"` 字符串常量里**转义的收尾引号**不能丢：`c"T\22"` 是两字节 `T` + `"`，
    /// 打印成 `c"T\""` 后必须回读成同样两字节。
    ///
    /// 改前实测：`decode_c_string` 用 `trim_end_matches('"')`（复数）剥引号，把
    /// **转义的收尾引号一起吃掉** ⇒ `[84, 34]` 静默变 `[84]`。
    #[test]
    fn escaped_trailing_quote_in_c_string_survives() {
        for src in [
            "@g = global [2 x i8] c\"T\\22\"\n",
            "@g = global [2 x i8] c\"T\\\"\"\n",
            "@g = global [2 x i8] [i8 0x54, i8 0x22]\n",
        ] {
            let m = parse_module(src).unwrap_or_else(|e| panic!("parse 失败（{src}）：{e}"));
            let (_, g) = m.iter_globals().next().expect("global");
            assert_eq!(
                g.init.as_deref(),
                Some(&[84u8, 34][..]),
                "转义收尾引号必须保真（源：{src}）"
            );
            let t1 = m.to_string();
            assert!(
                t1.contains("c\"T\\\"\""),
                "应打印成 c-string 形态（实测：{t1}）"
            );
            let m2 = parse_module(&t1).expect("打印结果必须可回读");
            let (_, g2) = m2.iter_globals().next().expect("global");
            assert_eq!(
                g2.init.as_deref(),
                Some(&[84u8, 34][..]),
                "打印-回读后字节必须不变（源：{src}；打印：{t1}）"
            );
            assert_eq!(print(&t1), t1, "c-string 打印必须幂等（源：{src}）");
        }
    }
}
