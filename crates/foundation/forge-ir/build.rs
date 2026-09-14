//! 构建脚本：① lalrpop 生成 IR 文本解析器；② 从 `ops.toml` 生成指令元数据。
//!
//! ② 是 v3 方案 S1 的落地：`ops.toml` 是**指令清单与派生属性的单一事实源**，
//! 生成的 `$OUT_DIR/opcode_gen.rs` 被 `src/opcode.rs` `include!`。
//! 生成器 **fail-closed**：字段缺失/类型不对/名字或助记符重复/载荷未知
//! 一律 `panic!` 中断构建（不静默跳过、不给默认名字）。

fn main() {
    // ① lalrpop：src/ir_parser/grammar.lalrpop → grammar.rs
    // （生成代码的 clippy 豁免见 mod.rs 的 lalrpop_mod!(#[allow(...)] ...) 宏参数。）
    lalrpop::process_root().unwrap();
    // ② 指令元数据
    generate_opcode_table();
}

/// 操作数个数约束。
#[derive(PartialEq)]
enum Arity {
    Fixed(u8),
    Variadic,
}

struct OpDef {
    name: String,
    mnemonic: String,
    category: String,
    doc: String,
    arity: Arity,
    results: u8,
    may_ub: bool,
    side_effect: bool,
    /// 比较条件 immediate 的通道类型（`Icmp`/`Fcmp` 专用）：`IntCC` | `FloatCC`。
    ///
    /// 这是**声明式的 immediate 契约**（不是变体载荷）：指令须带一条对应类型的
    /// `Immediate`，verifier 据此检查；宿主 lowering 用 `IntCC::code()`/
    /// `FloatCC::code()` 把它喂给 `current_immediates`。
    cond: Option<String>,
    /// LLVM 文本指令名（display 输出用）。
    llvm: String,
    /// 该 LLVM 文本名是否可**解析回本 opcode**；`false` = 仅 display 用
    /// （别名/精化/常量内联/需要条件等，见 ops.toml 注释）。
    llvm_parse: bool,
    /// 额外可解析名（别名，`llvm` 之外的旧名/宽松名）。
    llvm_aliases: Vec<String>,
    /// 逐指令**类型规则族**（verifier 的操作数类型阶段据此分派；规则本体在
    /// `verify.rs` 实现，这里只声明"该指令属于哪个族"）。
    ///
    /// 词汇表封闭（[`TYPE_RULES`]）：写成未实现的族名 → 构建期报错；
    /// 新增族名会让 `verify.rs` 的穷举 match 编译失败（双向 fail-closed）。
    type_rule: String,
    /// `type_rule = "convert"` 时的事实三元组 `(src 类, dst 类, 位宽关系)`。
    convert: Option<(String, String, String)>,
}

/// 类型规则族的封闭词汇表（与 `verify.rs` 的穷举 match 一一对应）。
const TYPE_RULES: &[&str] = &[
    "none",
    "binop_same",
    "same3",
    "cmp_int",
    "cmp_float",
    "select",
    "convert",
    "cmpxchg_pair",
    "load",
    "store",
    "call",
    "call_indirect",
];
/// 转换指令的源/目标类型类。
const TYPE_CLASSES: &[&str] = &["any", "int", "float", "ptr", "vector"];
/// 转换指令的位宽关系。
const WIDTH_RULES: &[&str] = &["any", "widen", "narrow", "equal_bytes", "equal_total_bits"];

/// `snake_case` → `PascalCase`（生成枚举变体名用；词汇表已限定字符集）。
fn pascal(s: &str) -> String {
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn generate_opcode_table() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR 未设置");
    let toml_path = std::path::Path::new(&manifest).join("ops.toml");
    println!("cargo:rerun-if-changed={}", toml_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    let source = std::fs::read_to_string(&toml_path)
        .unwrap_or_else(|e| panic!("读不到 {}：{e}", toml_path.display()));
    let doc: toml::Value = toml::from_str(&source)
        .unwrap_or_else(|e| panic!("{} 不是合法 TOML：{e}", toml_path.display()));

    let version = doc
        .get("version")
        .and_then(|v| v.as_integer())
        .unwrap_or_else(|| panic!("{} 缺 `version`（整数）", toml_path.display()));
    assert_eq!(version, 1, "ops.toml version 只支持 1，实际 {version}");

    let ops = doc
        .get("op")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("{} 缺 `[[op]]` 列表", toml_path.display()));
    assert!(!ops.is_empty(), "ops.toml 的 `[[op]]` 列表为空");

    let mut defs: Vec<OpDef> = Vec::with_capacity(ops.len());
    for (idx, op) in ops.iter().enumerate() {
        let table = op
            .as_table()
            .unwrap_or_else(|| panic!("ops.toml 第 {idx} 项不是表"));
        let req = |key: &str| -> String {
            table
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("ops.toml 第 {idx} 项缺字符串字段 `{key}`"))
                .to_string()
        };
        let name = req("name");
        let mnemonic = req("mnemonic");
        let category = req("category");
        let doc_text = req("doc");
        assert!(
            !doc_text.trim().is_empty(),
            "ops.toml `{name}` 的 doc 为空——每个 opcode 都要有可读文档"
        );
        let arity = match table.get("operands") {
            Some(toml::Value::Integer(n)) => {
                assert!(
                    (0..=255).contains(n),
                    "ops.toml `{name}` 的 operands={n} 越界（0..=255）"
                );
                Arity::Fixed(*n as u8)
            }
            Some(toml::Value::String(s)) if s == "variadic" => Arity::Variadic,
            other => {
                panic!("ops.toml `{name}` 的 operands 须为整数或 \"variadic\"，实际 {other:?}")
            }
        };
        let results = match table.get("results") {
            None => 1u8,
            Some(toml::Value::Integer(n)) => {
                assert!(
                    (0..=255).contains(n),
                    "ops.toml `{name}` 的 results={n} 越界（0..=255）"
                );
                *n as u8
            }
            other => panic!("ops.toml `{name}` 的 results 须为整数，实际 {other:?}"),
        };
        let flag = |key: &str| -> bool {
            match table.get(key) {
                None => false,
                Some(toml::Value::Boolean(b)) => *b,
                other => panic!("ops.toml `{name}` 的 {key} 须为布尔，实际 {other:?}"),
            }
        };
        let cond = match table.get("cond") {
            None => None,
            Some(toml::Value::String(s)) if s == "IntCC" || s == "FloatCC" => Some(s.clone()),
            other => panic!("ops.toml `{name}` 的 cond 只支持 IntCC/FloatCC，实际 {other:?}"),
        };
        // 条件通道与指令身份是绑定的：`Icmp` 只能是 IntCC、`Fcmp` 只能是 FloatCC
        // （写反了会让 ISA 规则按错的条件码分派，生成期就拦住）。
        match name.as_str() {
            "Icmp" => assert_eq!(
                cond.as_deref(),
                Some("IntCC"),
                "ops.toml Icmp 必须声明 cond = \"IntCC\""
            ),
            "Fcmp" => assert_eq!(
                cond.as_deref(),
                Some("FloatCC"),
                "ops.toml Fcmp 必须声明 cond = \"FloatCC\""
            ),
            _ => assert!(
                cond.is_none(),
                "ops.toml `{name}` 不是比较指令，不应声明 cond"
            ),
        }
        // flag 闭包借 `name`（错误消息用）——先取值再构造结构体，避免同时借用与移动。
        let may_ub = flag("may_ub");
        let side_effect = flag("side_effect");

        // LLVM 文本名（display 用）+ 是否可解析回本 opcode + 别名。
        let llvm = req("llvm");
        assert!(
            !llvm.trim().is_empty() && !llvm.contains(' '),
            "ops.toml `{name}` 的 llvm 名须为非空单词（不含空格），实际 {llvm:?}"
        );
        let llvm_parse = match table.get("llvm_parse") {
            None => true,
            Some(toml::Value::Boolean(b)) => *b,
            other => panic!("ops.toml `{name}` 的 llvm_parse 须为布尔，实际 {other:?}"),
        };
        let llvm_aliases: Vec<String> = match table.get("llvm_alias") {
            None => Vec::new(),
            Some(toml::Value::Array(items)) => items
                .iter()
                .map(|it| {
                    it.as_str()
                        .unwrap_or_else(|| {
                            panic!("ops.toml `{name}` 的 llvm_alias 元素须为字符串，实际 {it:?}")
                        })
                        .to_string()
                })
                .collect(),
            other => panic!("ops.toml `{name}` 的 llvm_alias 须为字符串数组，实际 {other:?}"),
        };

        // 逐指令类型规则族（封闭词汇表）+ 转换指令的事实三元组
        let type_rule = match table.get("type_rule") {
            Some(toml::Value::String(s)) if TYPE_RULES.contains(&s.as_str()) => s.clone(),
            other => {
                panic!("ops.toml `{name}` 的 type_rule 须是 {TYPE_RULES:?} 之一，实际 {other:?}")
            }
        };
        let convert = match table.get("convert") {
            None => None,
            Some(toml::Value::Table(t)) => {
                let field = |k: &str, allowed: &[&str]| -> String {
                    match t.get(k).and_then(|v| v.as_str()) {
                        Some(s) if allowed.contains(&s) => s.to_string(),
                        other => panic!(
                            "ops.toml `{name}` 的 convert.{k} 须是 {allowed:?} 之一，实际 {other:?}"
                        ),
                    }
                };
                Some((
                    field("src", TYPE_CLASSES),
                    field("dst", TYPE_CLASSES),
                    field("width", WIDTH_RULES),
                ))
            }
            other => panic!("ops.toml `{name}` 的 convert 须是内联表，实际 {other:?}"),
        };
        // 规则与事实表必须配套（`convert` 规则没有三元组就无法校验；反之声明了
        // 三元组的其它族也不会被 verifier 读取——两种都拦在构建期）。
        match (type_rule.as_str(), &convert) {
            ("convert", None) => panic!(
                "ops.toml `{name}` 声明了 type_rule = \"convert\"，必须给 convert = {{ src, dst, width }}"
            ),
            ("convert", Some(_)) => {}
            (other, Some(_)) => panic!(
                "ops.toml `{name}` 的 type_rule = \"{other}\" 不需要 convert 三元组（只有 convert 族读它）"
            ),
            (_, None) => {}
        }
        defs.push(OpDef {
            name,
            mnemonic,
            category,
            doc: doc_text,
            arity,
            results,
            may_ub,
            side_effect,
            cond,
            llvm,
            llvm_parse,
            llvm_aliases,
            type_rule,
            convert,
        });
    }

    // 唯一性（生成期就拦住，而不是等到运行期查表返回第一个）。
    //
    // 全部用 `HashMap` 做 **O(1) 探测**（总数 n 条、解析名 m 条 ⇒ O(n+m)）：
    // 之前是"每个变体与其余全部比较"的 O(n²) 双重循环 + `seen.iter().find`，
    // 与"查找应当是 O(1)"的同一原则相悖（虽然只在构建期跑一次）。
    // 冲突信息也更有用：直接点名"哪两条"。
    {
        use std::collections::HashMap;
        let mut by_name: HashMap<&str, &str> = HashMap::with_capacity(defs.len());
        let mut by_mnemonic: HashMap<&str, &str> = HashMap::with_capacity(defs.len());
        let mut by_parse_name: HashMap<&str, &str> = HashMap::with_capacity(defs.len() * 2);
        for d in &defs {
            if let Some(prev) = by_name.insert(d.name.as_str(), d.name.as_str()) {
                panic!("ops.toml 变体名重复：{}", prev);
            }
            if let Some(prev) = by_mnemonic.insert(d.mnemonic.as_str(), d.name.as_str()) {
                panic!(
                    "ops.toml 助记符重复：{}（{prev} 与 {}）",
                    d.mnemonic, d.name
                );
            }
            // 解析名唯一：`llvm_parse` 的 llvm 名 + 全部别名
            // （否则 `from_llvm_name` 的"查第一个"就变成隐式优先级，正是要消灭的约定）
            if d.llvm_parse
                && let Some(prev) = by_parse_name.insert(d.llvm.as_str(), d.name.as_str())
            {
                panic!(
                    "ops.toml 解析名冲突：`{}` 同时属于 {prev} 与 {}",
                    d.llvm, d.name
                );
            }
            for a in &d.llvm_aliases {
                if let Some(prev) = by_parse_name.insert(a.as_str(), d.name.as_str()) {
                    panic!("ops.toml 解析名冲突：`{a}` 同时属于 {prev} 与 {}", d.name);
                }
            }
        }
    }

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR 未设置");
    let out_path = std::path::Path::new(&out_dir).join("opcode_gen.rs");
    std::fs::write(&out_path, render(&defs))
        .unwrap_or_else(|e| panic!("写 {} 失败：{e}", out_path.display()));
}

/// 生成 `opcode_gen.rs` 正文。
fn render(defs: &[OpDef]) -> String {
    let mut s = String::with_capacity(defs.len() * 512);
    s.push_str(
        "// @generated by build.rs from ops.toml —— 不要手改本文件。\n\
         // 改指令清单/属性：编辑 crates/foundation/forge-ir/ops.toml（v3 方案 S1）。\n\n",
    );

    // === 枚举 ===
    s.push_str(
        "/// IR 操作码。\n\
         ///\n\
         /// 变体清单、助记符与全部派生属性来自 `ops.toml`（生成期同源，不可能漂移）。\n\
         /// 类型信息、常量、块引用等经 `Immediate` 传递；`Icmp`/`Fcmp` 的比较条件\n\
         /// 同样走 immediate 通道（`Immediate::IntCC`/`FloatCC`，见 `OpcodeInfo.cond`）。\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\n\
         pub enum Opcode {\n",
    );
    // 分节注释里的条数：预聚合一次（O(n)），而不是每换一个分组就线性数一遍（O(n·c)）。
    let category_counts: std::collections::HashMap<&str, usize> = {
        let mut m: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for d in defs {
            *m.entry(d.category.as_str()).or_insert(0) += 1;
        }
        m
    };
    let mut cur_cat = "";
    for d in defs {
        if d.category != cur_cat {
            cur_cat = &d.category;
            let n = category_counts
                .get(cur_cat)
                .copied()
                .unwrap_or_else(|| unreachable!("分组 {cur_cat} 未统计"));
            let _ = std::fmt::Write::write_fmt(
                &mut s,
                format_args!("    // === {} ({n}) ===\n", cur_cat),
            );
        }
        let _ =
            std::fmt::Write::write_fmt(&mut s, format_args!("    /// {}\n    {}", d.doc, d.name));
        s.push_str(",\n");
    }
    s.push_str("}\n\n");

    // === 操作数元数 ===
    s.push_str(
        "/// 值操作数个数约束。\n\
         ///\n\
         /// [`Opcode::expected_operand_count`] 对 `Variadic` 返回 `0`（历史契约\n\
         /// `0` = 不检查）；真正的零操作数指令声明为 `Fixed(0)`，两者可区分。\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq)]\n\
         pub enum OperandArity {\n\
         \x20   /// 固定个数（含 0：常量、地址、屏障等无值操作数指令）。\n\
         \x20   Fixed(u8),\n\
         \x20   /// 个数不定（调用、GEP、向量重排等），不做个数检查。\n\
         \x20   Variadic,\n\
         }\n\n",
    );

    // === OpcodeInfo ===
    s.push_str(
        "/// 单个 opcode 的元数据（`ops.toml` 一行的运行期投影）。\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq)]\n\
         pub struct OpcodeInfo {\n\
         \x20   /// 变体名（ISA TOML 的 `op = \"Iadd\"` 契约）。\n\
         \x20   pub name: &'static str,\n\
         \x20   /// 规范助记符（display 用；全局唯一）。\n\
         \x20   pub mnemonic: &'static str,\n\
         \x20   /// 分组名（枚举分节注释同源）。\n\
         \x20   pub category: &'static str,\n\
         \x20   /// 变体文档。\n\
         \x20   pub doc: &'static str,\n\
         \x20   /// 值操作数个数约束。\n\
         \x20   pub arity: OperandArity,\n\
         \x20   /// 结果值个数。\n\
         \x20   pub result_count: u8,\n\
         \x20   /// 是否可能有未定义行为。\n\
         \x20   pub may_ub: bool,\n\
         \x20   /// 是否有副作用（不可被 DCE 删除）。\n\
         \x20   pub side_effect: bool,\n\
         \x20   /// 比较条件 immediate 的通道（`Icmp`/`Fcmp` 有值，其余为 `None`）。\n\
         \x20   pub cond: Option<CondKind>,\n\
         \x20   /// LLVM 文本指令名（display 输出的 base 名；`icmp`/`fcmp` 的条件由\n\
         \x20   /// display 层从 immediate 拼上）。\n\
         \x20   pub llvm: &'static str,\n\
         \x20   /// 该文本名可解析回的 opcode（`None` = 仅 display 用：别名/精化/\n\
         \x20   /// 常量内联/需要条件等，见 `ops.toml` 注释）。\n\
         \x20   pub llvm_parse: Option<&'static str>,\n\
         \x20   /// 额外可解析名（`llvm` 之外的旧名/宽松名）。\n\
         \x20   pub llvm_aliases: &'static [&'static str],\n\
         \x20   /// 逐指令类型规则族（verifier 的操作数类型阶段据此分派）。\n\
         \x20   pub type_rule: TypeRule,\n\
         \x20   /// `type_rule == TypeRule::Convert` 时的事实三元组（其余族为 `None`）。\n\
         \x20   pub convert: Option<ConvertRule>,\n\
         }\n\n\
         /// 比较条件所在的 immediate 通道类型。\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq)]\n\
         pub enum CondKind {\n\
         \x20   /// 条件经 `Immediate::IntCC` 传递（`Icmp`）。\n\
         \x20   IntCC,\n\
         \x20   /// 条件经 `Immediate::FloatCC` 传递（`Fcmp`）。\n\
         \x20   FloatCC,\n\
         }\n\n\
         /// 逐指令**类型规则族**。\n\
         ///\n\
         /// 族名声明在 `ops.toml`（`type_rule`），规则本体实现在 `verify.rs` 的\n\
         /// `check_operand_types`；那里对本枚举做**穷举 match**（无 `_` 臂），\n\
         /// 因此\"声明了新族却没实现\"或\"实现了却没声明\"都会编译/构建失败。\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq)]\n\
         pub enum TypeRule {\n\
         \x20   /// 该指令没有类型规则（`ops.toml` 注明是\"检查在别处\"还是\"待补\"）。\n\
         \x20   None,\n\
         \x20   /// 前两个值操作数同类型，且不是聚合类型。\n\
         \x20   BinopSame,\n\
         \x20   /// 三个操作数同类型（`Fma`）。\n\
         \x20   Same3,\n\
         \x20   /// 两个操作数同类型且为整型（`Icmp`）。\n\
         \x20   CmpInt,\n\
         \x20   /// 两个操作数同类型且为浮点（`Fcmp`）。\n\
         \x20   CmpFloat,\n\
         \x20   /// `select`：cond 为 bool、两个分支同类型且结果同类型。\n\
         \x20   Select,\n\
         \x20   /// 转换指令：按 [`ConvertRule`] 校验源/目标类与位宽。\n\
         \x20   Convert,\n\
         \x20   /// `cmpxchg`：cmp 与 new 操作数同类型。\n\
         \x20   CmpxchgPair,\n\
         \x20   /// `load`：地址为指针、结果类型有大小。\n\
         \x20   Load,\n\
         \x20   /// `store`：地址为指针。\n\
         \x20   Store,\n\
         \x20   /// `call`：有被调操作数。\n\
         \x20   Call,\n\
         \x20   /// `call_indirect`：有被调操作数且为指针。\n\
         \x20   CallIndirect,\n\
         }\n\n\
         /// 转换指令的源/目标类型类。\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq)]\n\
         pub enum TypeClass {\n\
         \x20   /// 不限制类别。\n\
         \x20   Any,\n\
         \x20   /// 整数（含 bool）。\n\
         \x20   Int,\n\
         \x20   /// 浮点。\n\
         \x20   Float,\n\
         \x20   /// 指针。\n\
         \x20   Ptr,\n\
         \x20   /// 向量。\n\
         \x20   Vector,\n\
         }\n\n\
         /// 转换指令的位宽关系。\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq)]\n\
         pub enum WidthRule {\n\
         \x20   /// 不检查位宽。\n\
         \x20   Any,\n\
         \x20   /// 目标严格宽于源（sext/zext/fpext）。\n\
         \x20   Widen,\n\
         \x20   /// 目标严格窄于源（ireduce/fptrunc）。\n\
         \x20   Narrow,\n\
         \x20   /// 字节大小相同（bitcast）。\n\
         \x20   EqualBytes,\n\
         \x20   /// 向量总位宽相同（vbitcast；只在两侧都是向量时比较）。\n\
         \x20   EqualTotalBits,\n\
         }\n\n\
         /// 转换指令的类型事实（`ops.toml` 的 `convert` 内联表）。\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq)]\n\
         pub struct ConvertRule {\n\
         \x20   /// 源操作数类型类。\n\
         \x20   pub src: TypeClass,\n\
         \x20   /// 目标（结果）类型类。\n\
         \x20   pub dst: TypeClass,\n\
         \x20   /// 位宽关系。\n\
         \x20   pub width: WidthRule,\n\
         }\n\n",
    );

    // === impl ===
    s.push_str("impl Opcode {\n");
    s.push_str(
        "    /// **全部 opcode 变体**（`ops.toml` 声明序）。\n\
         \x20   ///\n\
         \x20   /// 覆盖率矩阵、一致性守卫、名字查找都从它/`INFOS` 派生，\n\
         \x20   /// 不再有第二份手写清单。\n\
         \x20   pub const ALL: &'static [Opcode] = &[\n",
    );
    for d in defs {
        let _ = std::fmt::Write::write_fmt(&mut s, format_args!("        Opcode::{},\n", d.name));
    }
    s.push_str("    ];\n\n");

    s.push_str("    /// 与 [`Opcode::ALL`] 同序的元数据表。\n    pub const INFOS: &'static [OpcodeInfo] = &[\n");
    for d in defs {
        let arity = match &d.arity {
            Arity::Fixed(n) => format!("OperandArity::Fixed({n})"),
            Arity::Variadic => "OperandArity::Variadic".to_string(),
        };
        let cond = match d.cond.as_deref() {
            None => "None".to_string(),
            Some(ty) => format!("Some(CondKind::{ty})"),
        };
        let llvm_parse = if d.llvm_parse {
            format!("Some({:?})", d.llvm)
        } else {
            "None".to_string()
        };
        let aliases = if d.llvm_aliases.is_empty() {
            "&[]".to_string()
        } else {
            let items: Vec<String> = d.llvm_aliases.iter().map(|a| format!("{a:?}")).collect();
            format!("&[{}]", items.join(", "))
        };
        let type_rule = format!("TypeRule::{}", pascal(&d.type_rule));
        let convert = match &d.convert {
            None => "None".to_string(),
            Some((src, dst, width)) => format!(
                "Some(ConvertRule {{ src: TypeClass::{}, dst: TypeClass::{}, width: WidthRule::{} }})",
                pascal(src),
                pascal(dst),
                pascal(width)
            ),
        };
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!(
                "        OpcodeInfo {{ name: {:?}, mnemonic: {:?}, category: {:?}, doc: {:?}, \
                 arity: {arity}, result_count: {}, may_ub: {}, side_effect: {}, cond: {cond}, \
                 llvm: {:?}, llvm_parse: {llvm_parse}, llvm_aliases: {aliases}, \
                 type_rule: {type_rule}, convert: {convert} }},\n",
                d.name, d.mnemonic, d.category, d.doc, d.results, d.may_ub, d.side_effect, d.llvm
            ),
        );
    }
    s.push_str("    ];\n\n");

    s.push_str(
        "    /// 本 opcode 的元数据。\n\
         \x20   ///\n\
         \x20   /// 生成的穷举 match（与 `ALL`/`INFOS` 同源同序），无 `_` 兜底臂。\n\
         \x20   pub fn info(&self) -> &'static OpcodeInfo {\n\
         \x20       match self {\n",
    );
    for (i, d) in defs.iter().enumerate() {
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!("            Opcode::{} => &Self::INFOS[{i}],\n", d.name),
        );
    }
    s.push_str("        }\n    }\n\n");

    s.push_str(
        "    /// 变体名（PascalCase；ISA TOML 的 `op` / `pattern.match` 名字契约）。\n\
         \x20   pub fn name(&self) -> &'static str {\n\
         \x20       self.info().name\n\
         \x20   }\n\n\
         \x20   /// 分组名。\n\
         \x20   pub fn category(&self) -> &'static str {\n\
         \x20       self.info().category\n\
         \x20   }\n\n\
         \x20   /// 变体文档（`ops.toml` 的 `doc`）。\n\
         \x20   pub fn doc(&self) -> &'static str {\n\
         \x20       self.info().doc\n\
         \x20   }\n\n\
         \x20   /// 规范助记符。\n\
         \x20   pub fn mnemonic(&self) -> &'static str {\n\
         \x20       self.info().mnemonic\n\
         \x20   }\n\n\
         \x20   /// 结果值个数。\n\
         \x20   pub fn result_count(&self) -> u8 {\n\
         \x20       self.info().result_count\n\
         \x20   }\n\n\
         \x20   /// 是否可能有未定义行为。\n\
         \x20   pub fn may_ub(&self) -> bool {\n\
         \x20       self.info().may_ub\n\
         \x20   }\n\n\
         \x20   /// 是否必然有副作用（不可被 DCE 删除）。\n\
         \x20   pub fn has_side_effect(&self) -> bool {\n\
         \x20       self.info().side_effect\n\
         \x20   }\n\n\
         \x20   /// 比较条件 immediate 的通道（`Icmp`/`Fcmp` 有值，其余 `None`）。\n\
         \x20   ///\n\
         \x20   /// 有值时该指令**必须**带一条对应类型的 immediate（`Verifier` 检查）。\n\
         \x20   pub fn cond_kind(&self) -> Option<CondKind> {\n\
         \x20       self.info().cond\n\
         \x20   }\n\n\
         \x20   /// 逐指令类型规则族（verifier 的操作数类型阶段据此分派）。\n\
         \x20   pub fn type_rule(&self) -> TypeRule {\n\
         \x20       self.info().type_rule\n\
         \x20   }\n\n\
         \x20   /// 期望的值操作数数量。\n\
         \x20   ///\n\
         \x20   /// **契约（历史沿革，勿改）**：`0` 表示\"不检查\"——变长指令与\n\
         \x20   /// 真正零操作数指令都返回 0。要区分二者请读 [`Opcode::info`] 的\n\
         \x20   /// `arity`。\n\
         \x20   pub fn expected_operand_count(&self) -> usize {\n\
         \x20       match self.info().arity {\n\
         \x20           OperandArity::Fixed(n) => n as usize,\n\
         \x20           OperandArity::Variadic => 0,\n\
         \x20       }\n\
         \x20   }\n",
    );

    // === 名字查找：生成的 `match`（不是线性扫 ALL）===
    //
    // 三种查找都是"文本 → opcode"的解析路径，必须走编译期决策树（字符串 match
    // 被 LLVM 编成"按长度分组 + 逐字节比较"），而不是 `ALL.iter().find(...)`：
    // 后者是 109 次 `&str` 比较/次（本机 debug 实测 2.77 µs/次 对 0.02 µs/次，
    // 见提交说明）。名字唯一性由本文件生成期断言，因此 `match` 不会有重复臂。
    s.push_str(
        "    /// 按变体名查找（ISA TOML 的 `op = \"Iadd\"` 契约）。\n\
         \x20   pub fn from_name(name: &str) -> Option<Opcode> {\n\
         \x20       Some(match name {\n",
    );
    for d in defs {
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!("            {:?} => Opcode::{},\n", d.name, d.name),
        );
    }
    s.push_str("            _ => return None,\n        })\n    }\n\n");

    s.push_str(
        "    /// 按规范助记符查找（[`Opcode::mnemonic`] 的逆）。\n\
         \x20   pub fn from_mnemonic(mnemonic: &str) -> Option<Opcode> {\n\
         \x20       Some(match mnemonic {\n",
    );
    for d in defs {
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!("            {:?} => Opcode::{},\n", d.mnemonic, d.name),
        );
    }
    s.push_str("            _ => return None,\n        })\n    }\n\n");

    s.push_str(
        "    /// 按 LLVM 文本名查找（解析用）。\n\
         \x20   ///\n\
         \x20   /// 命中 [`OpcodeInfo::llvm_parse`]（display 名可反向解析者）或\n\
         \x20   /// [`OpcodeInfo::llvm_aliases`]（`callbr`/`ptrtoaddr` 等旧名）。\n\
         \x20   /// 名字唯一性由 `build.rs` 在生成期断言（无\"查第一个\"的隐式优先级）；\n\
         \x20   /// `icmp`/`fcmp` 是 `llvm_parse = false`——它们需要条件，由调用方特判。\n\
         \x20   pub fn from_llvm_name(name: &str) -> Option<Opcode> {\n\
         \x20       Some(match name {\n",
    );
    for d in defs {
        if d.llvm_parse {
            let _ = std::fmt::Write::write_fmt(
                &mut s,
                format_args!("            {:?} => Opcode::{},\n", d.llvm, d.name),
            );
        }
        for a in &d.llvm_aliases {
            let _ = std::fmt::Write::write_fmt(
                &mut s,
                format_args!("            {a:?} => Opcode::{},\n", d.name),
            );
        }
    }
    s.push_str("            _ => return None,\n        })\n    }\n\n");

    // 需要条件的比较指令（`cond_kind()` 有值者）的 LLVM 文本名：单独一张 O(1) 表。
    // 解析器不能把 `icmp` 直接解析成无条件指令，`llvm_mapping::opcode` 用它给出
    // "需要条件"的解析错误——同样不写成线性扫 `ALL`。
    s.push_str(
        "    /// 按「需要条件的比较指令」的 LLVM 文本名查找（`opcode()` 的错误分派用）。\n\
         \x20   ///\n\
         \x20   /// 只有 [`Opcode::cond_kind`] 有值的指令（当前 `Icmp`/`Fcmp`）出现在这里：\n\
         \x20   /// 它们的文本名不能解析成无条件指令。\n\
         \x20   pub fn from_cond_llvm_name(name: &str) -> Option<Opcode> {\n\
         \x20       Some(match name {\n",
    );
    for d in defs.iter().filter(|d| d.cond.is_some()) {
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!("            {:?} => Opcode::{},\n", d.llvm, d.name),
        );
    }
    s.push_str("            _ => return None,\n        })\n    }\n}\n\n");
    s.push_str(
        "// 表与清单等长由生成期保证；这里补一条编译期断言防手工改动生成物。\n\
         const _: () = assert!(Opcode::ALL.len() == Opcode::INFOS.len());\n",
    );

    s
}
