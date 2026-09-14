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
    /// (载荷类型, `ALL` 里用的默认变体值)——载荷类型必须是本 crate 里手写的
    /// 非 opcode 枚举（`IntCC`/`FloatCC`），默认值须是该类型的合法变体名。
    payload: Option<(String, String)>,
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
            other => panic!("ops.toml `{name}` 的 operands 须为整数或 \"variadic\"，实际 {other:?}"),
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
        let payload = match table.get("payload") {
            None => None,
            Some(toml::Value::String(s)) if s == "IntCC" || s == "FloatCC" => {
                let default = table
                    .get("payload_default")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| {
                        panic!("ops.toml `{name}` 声明了 payload 就必须给 `payload_default`")
                    });
                assert!(
                    default.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
                    "ops.toml `{name}` 的 payload_default 须是变体名（PascalCase），实际 {default:?}"
                );
                Some((s.clone(), default.to_string()))
            }
            other => panic!("ops.toml `{name}` 的 payload 只支持 IntCC/FloatCC，实际 {other:?}"),
        };
        // 载荷与操作数个数是两回事（条件不占值操作数），这里只做一致性提醒：
        // 比较类指令必须带条件载荷，否则 `cond` 无处可放。
        if name == "Icmp" || name == "Fcmp" {
            assert!(
                payload.is_some(),
                "ops.toml `{name}` 必须声明 payload（条件承载在变体上）"
            );
        }
        // flag 闭包借 `name`（错误消息用）——先取值再构造结构体，避免同时借用与移动。
        let may_ub = flag("may_ub");
        let side_effect = flag("side_effect");
        defs.push(OpDef {
            name,
            mnemonic,
            category,
            doc: doc_text,
            arity,
            results,
            may_ub,
            side_effect,
            payload,
        });
    }

    // 唯一性（生成期就拦住，而不是等到运行期查表返回第一个）
    for (i, a) in defs.iter().enumerate() {
        for b in defs.iter().skip(i + 1) {
            assert_ne!(a.name, b.name, "ops.toml 变体名重复：{}", a.name);
            assert_ne!(
                a.mnemonic, b.mnemonic,
                "ops.toml 助记符重复：{}（{} 与 {}）",
                a.mnemonic, a.name, b.name
            );
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
         /// 类型信息、常量、块引用等经 `Immediate` 传递；`Icmp`/`Fcmp` 的条件是变体载荷。\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\n\
         pub enum Opcode {\n",
    );
    let mut cur_cat = "";
    for d in defs {
        if d.category != cur_cat {
            cur_cat = &d.category;
            let n = defs.iter().filter(|x| x.category == cur_cat).count();
            let _ = std::fmt::Write::write_fmt(
                &mut s,
                format_args!("    // === {} ({n}) ===\n", cur_cat),
            );
        }
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!("    /// {}\n    {}", d.doc, d.name),
        );
        match &d.payload {
            None => s.push_str(",\n"),
            Some((ty, _default)) => {
                // 枚举声明只要类型；默认变体值只在 `ALL` 里用。
                let field = match ty.as_str() {
                    "IntCC" => "cond: IntCC",
                    "FloatCC" => "cond: FloatCC",
                    other => unreachable!("未知载荷 {other}"),
                };
                let _ = std::fmt::Write::write_fmt(&mut s, format_args!(" {{ {field} }},\n"));
            }
        }
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
        match &d.payload {
            None => {
                let _ = std::fmt::Write::write_fmt(&mut s, format_args!("        Opcode::{},\n", d.name));
            }
            Some((ty, default)) => {
                let _ = std::fmt::Write::write_fmt(
                    &mut s,
                    format_args!("        Opcode::{} {{ cond: {ty}::{default} }},\n", d.name),
                );
            }
        }
    }
    s.push_str("    ];\n\n");

    s.push_str("    /// 与 [`Opcode::ALL`] 同序的元数据表。\n    pub const INFOS: &'static [OpcodeInfo] = &[\n");
    for d in defs {
        let arity = match &d.arity {
            Arity::Fixed(n) => format!("OperandArity::Fixed({n})"),
            Arity::Variadic => "OperandArity::Variadic".to_string(),
        };
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!(
                "        OpcodeInfo {{ name: {:?}, mnemonic: {:?}, category: {:?}, doc: {:?}, \
                 arity: {arity}, result_count: {}, may_ub: {}, side_effect: {} }},\n",
                d.name, d.mnemonic, d.category, d.doc, d.results, d.may_ub, d.side_effect
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
        let pat = match &d.payload {
            None => format!("Opcode::{}", d.name),
            Some(_) => format!("Opcode::{} {{ .. }}", d.name),
        };
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!("            {pat} => &Self::INFOS[{i}],\n"),
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
         \x20   }\n\n\
         \x20   /// 按变体名查找。\n\
         \x20   pub fn from_name(name: &str) -> Option<Opcode> {\n\
         \x20       Self::ALL.iter().copied().find(|op| op.name() == name)\n\
         \x20   }\n\n\
         \x20   /// 按规范助记符查找（[`Opcode::mnemonic`] 的逆）。\n\
         \x20   pub fn from_mnemonic(mnemonic: &str) -> Option<Opcode> {\n\
         \x20       Self::ALL.iter().copied().find(|op| op.mnemonic() == mnemonic)\n\
         \x20   }\n\
         }\n\n\
         // 表与清单等长由生成期保证；这里补一条编译期断言防手工改动生成物。\n\
         const _: () = assert!(Opcode::ALL.len() == Opcode::INFOS.len());\n",
    );

    s
}
