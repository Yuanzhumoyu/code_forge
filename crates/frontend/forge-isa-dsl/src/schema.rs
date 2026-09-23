//! `schema` — ISA-DSL TOML 的 **JSON Schema**（v18 S7c，手写发射器）。
//!
//! 为什么手写：方案 §10.4 明确"不新增依赖"（不引 `schemars`）。手写的代价是**可能与
//! 模型漂移**，因此配一条三方守卫
//! （`crates/frontend/forge-isa-dsl/tests/schema_guard.rs`）：
//!
//! 1. **模型 ↔ schema**：本文件的键清单逐节与 `v12/model.rs` 的结构体 `pub` 字段
//!    对照（`#[serde(skip)]` 的内部字段不入 schema，且必须在守卫里显式列出）；
//! 2. **schema ↔ 文档**：schema 里用户可见的键必须在
//!    `docs/reference/isa-dsl.md` 里出现过（未逐键列出的进 `DOCS_PENDING` 白名单，
//!    条目一旦被文档覆盖就必须删除——只减不增）。
//!
//! 用法：`forge-isa schema` 打印；ISA 谱顶部写 `#:schema <路径>` 让编辑器
//! （Taplo 等）自动补全（TOML 注释约定）。

/// 一节的 schema 描述。
pub struct Section {
    /// TOML 路径（人读/文档用；`<name>` 表示键名可变）。
    pub path: &'static str,
    /// 与 `v12/model.rs` 里哪个结构体对照（空 = 自由形态，不走模型对照）。
    pub model: &'static str,
    /// 必填键。
    pub required: &'static [&'static str],
    /// 可选键。
    pub optional: &'static [&'static str],
    /// **serde flatten** 进来的键（TOML 里与本节的键同级）。
    pub flatten: &'static [&'static str],
    /// 允许本节出现未列出的键（自由表：`[types]`/`fields`/模板 `body`/`rows`…）。
    pub additional: bool,
    /// 给编辑器看的一句话说明。
    pub doc: &'static str,
}

/// `EncKeys` 的键（v15 起 form 预设与指令**共用**同一组编码键）。
const ENC_KEYS: &[&str] = &[
    "modrm",
    "modrm_fixed",
    "rex",
    "vex",
    "evex",
    "prefix",
    "opsize",
    "rex_w",
    "opcode_reg",
    "imm",
    "escape",
    "opcode_field",
    "operand_fields",
];

/// 全部节（含嵌套子表）。守卫按 `model` 字段与模型结构体逐键对照。
pub const SECTIONS: &[Section] = &[
    Section {
        path: "<root>",
        model: "V12Model",
        required: &["meta"],
        optional: &[
            "include",
            "override",
            "encoding",
            "reg",
            "conventions",
            "types",
            "stack",
            "operand_slots",
            "forms",
            "instructions",
            "templates",
            "reloc",
            "derive",
            "pseudo",
            "lowering",
            "pattern",
            "abi",
            "emit",
            "spill",
            "vectors",
        ],
        flatten: &[],
        additional: false,
        doc: "ISA 谱根（`include`/`[[override]]` 为多文件组合键，由 loader 合并后才进模型）",
    },
    Section {
        path: "[meta]",
        model: "Meta",
        required: &["name"],
        optional: &[
            "version",
            "variants",
            "endian",
            "mode",
            "case_insensitive_regs",
            "comment_char",
            "label_suffix",
            "mnemonic_case",
            "imm_prefix",
            "directive_prefix",
            "default_gpr_width",
            "default_fpr_width",
            "addr_width",
            "value_gpr_width",
            "value_fpr_width",
            "vector_tiers",
        ],
        flatten: &[],
        additional: false,
        doc: "元信息 + 宽度元数据（缺省从 [reg.*] 派生）",
    },
    Section {
        path: "[encoding]",
        model: "Encoding",
        required: &["kind"],
        optional: &["bits", "widths", "max_len", "default_opsize"],
        flatten: &[],
        additional: false,
        doc: "指令宽度三态：fixed | mixed | prefix_scan（v18 S4）",
    },
    Section {
        path: "[reg.<name>]",
        model: "RegGroup",
        required: &[],
        optional: &["names", "prefix", "base_index", "count"],
        flatten: &[],
        additional: false,
        doc: "寄存器组；组名的数字 = 字节宽（gpr8 = 64 位）",
    },
    Section {
        path: "[stack]",
        model: "StackSection",
        required: &[],
        optional: &["slot", "align", "fp_save"],
        flatten: &[],
        additional: false,
        doc: "栈槽单位/对齐/帧指针保存槽（缺省全部派生）",
    },
    Section {
        path: "[types]",
        model: "",
        required: &[],
        optional: &[],
        flatten: &[],
        additional: true,
        doc: "类型 → 寄存器组名（或 \"unsupported\"）的显式映射；键 = 类型名",
    },
    Section {
        path: "[conventions.bitfields.<name>]",
        model: "Bitfield",
        required: &[],
        optional: &["offset", "width", "pieces"],
        flatten: &[],
        additional: false,
        doc: "命名位域：offset/width，或 pieces 列出散布位段",
    },
    Section {
        path: "[[conventions.bitfields.<name>.pieces]]",
        model: "BitfieldPiece",
        required: &["offset", "width"],
        optional: &["shift"],
        flatten: &[],
        additional: false,
        doc: "散布位段：`value >> shift` 取 width 位放在 offset",
    },
    Section {
        path: "[conventions.modrm]",
        model: "ModrmConvention",
        required: &[],
        optional: &["reg_field", "rm_field", "force_disp_base"],
        flatten: &[],
        additional: false,
        doc: "ModRM 约定（表存在即启用）：reg/rm 位域名 + 强制位移的 base 寄存器号",
    },
    Section {
        path: "[conventions.cond]",
        model: "CondEntry",
        required: &["code"],
        optional: &["ir"],
        flatten: &[],
        additional: true,
        doc: "条件码表：键 = 汇编可见的条件名（也允许 `名 = <整数>` 简写）",
    },
    Section {
        path: "[[conventions.prefix_scan]]",
        model: "PrefixScanEntry",
        required: &[],
        optional: &["byte", "range", "effects"],
        flatten: &[],
        additional: false,
        doc: "变长前缀扫描表（缺省 = x86 集）",
    },
    Section {
        path: "[conventions.mem]",
        model: "MemTemplate",
        required: &["template"],
        optional: &[],
        flatten: &[],
        additional: false,
        doc: "内存操作数文本模板（缺省 x86 `[{base}+{index}*{scale}+{disp}]`）",
    },
    Section {
        path: "[[operand_slots]]",
        model: "OperandSlot",
        required: &["name", "kind"],
        optional: &[
            "class", "classes", "byte_reg", "width", "signed", "float", "min", "max", "roles",
        ],
        flatten: &[],
        additional: false,
        doc: "操作数槽：kind = reg | imm | mem | label | cond",
    },
    Section {
        path: "[[forms]]",
        model: "Form",
        required: &["name"],
        optional: &[],
        // Form 的编码键是 `#[serde(flatten)]` 的 EncKeys：TOML 里与本节的键同级。
        flatten: ENC_KEYS,
        additional: false,
        doc: "编码形式：可选的键预设（指令可逐键覆盖）",
    },
    Section {
        path: "[[instructions]]",
        model: "Instruction",
        required: &["name", "asm"],
        optional: &[
            "form",
            "opcode",
            "fields",
            "ops",
            "when",
            "effect",
            "roles",
            "implicit_regs",
            "reloc",
            "width",
            "only_variants",
            // TOML 键是 `ref`（模型字段名 `reference` + `#[serde(rename = "ref")]`）。
            // 守卫按 `#[serde(rename = …)]` 取键名，所以这里必须写 **用户在 TOML 里
            // 实际写的那个键**——写字段名会让编辑器对每一行 `ref = …` 报未知键
            // （2026-09-21 实测：`isa/x86_v12.toml` 的 35 处 `ref` 全部被 Taplo 标红）。
            "ref",
        ],
        // 编码键是 `#[serde(flatten)]` 的 EncKeys：直接在指令上写（不写 `enc = {...}`）。
        flatten: ENC_KEYS,
        additional: false,
        doc: "指令：编码键可与 form 预设混用（指令优先）",
    },
    Section {
        path: "[[templates]]",
        model: "Template",
        required: &["rows"],
        optional: &["name", "body"],
        flatten: &[],
        additional: false,
        doc: "唯一指令复用机制：`body` 共享字段 + `rows` 每行一条指令",
    },
    Section {
        path: "[[reloc]]",
        model: "RelocDef",
        required: &["name", "semantics", "slot"],
        optional: &["addend"],
        flatten: &[],
        additional: false,
        doc: "重定位表：semantics = absolute | pc_relative（v18 S3d）",
    },
    Section {
        path: "[[derive]]",
        model: "DeriveDef",
        required: &["name", "expr"],
        optional: &[],
        flatten: &[],
        additional: false,
        doc: "派生谓词属性（v18 S3f）",
    },
    Section {
        path: "[[pseudo]]",
        model: "PseudoDef",
        required: &["name", "params", "emit"],
        optional: &["only_variants"],
        flatten: &[],
        additional: false,
        doc: "汇编器伪指令：文本级多指令展开（v18 S3e）",
    },
    Section {
        path: "[[lowering]]",
        model: "Lowering",
        required: &["op", "insts"],
        optional: &["when", "vary", "priority"],
        flatten: &[],
        additional: false,
        doc: "指令选择规则",
    },
    Section {
        path: "[[pattern]]",
        model: "Pattern",
        required: &["insts"],
        optional: &["when", "match", "priority", "only_variants"],
        flatten: &[],
        additional: false,
        doc: "树型多指令匹配（`match` 是 Rust 关键字，模型里写作 `r#match`）",
    },
    Section {
        path: "[abi]",
        model: "Abi",
        required: &[],
        optional: &[
            "frame_padding",
            "stack_args",
            "arg_class",
            "frame",
            "callee_saved",
            "scratch",
            "ret_regs",
            "call_ret_reg",
            "call_clobbers",
            "reserved",
            "arg_slot",
        ],
        flatten: &[],
        additional: false,
        doc: "调用约定",
    },
    Section {
        path: "[abi.frame]",
        model: "AbiFrame",
        required: &[],
        optional: &["sp", "fp", "layout", "fp_push_bytes", "alloc_neg"],
        flatten: &[],
        additional: false,
        doc: "帧布局",
    },
    Section {
        path: "[abi.stack_args]",
        model: "AbiStackArgs",
        required: &[],
        optional: &[
            "callee_base",
            "caller_base",
            "first_offset_slots",
            "stride_slots",
            "shadow_bytes",
        ],
        flatten: &[],
        additional: false,
        doc: "栈参数布局（全部由 ISA 数据给出）",
    },
    Section {
        path: "[abi.arg_class]",
        model: "ArgClass",
        required: &[],
        optional: &["class", "regs", "strategy", "limit"],
        flatten: &[],
        additional: false,
        doc: "参数寄存器类/顺序/策略/by-value 阈值",
    },
    Section {
        path: "[abi.callee_saved]",
        model: "CalleeSaved",
        required: &[],
        optional: &["gpr", "xmm"],
        flatten: &[],
        additional: false,
        doc: "被调用者保存寄存器名单",
    },
    Section {
        path: "[emit]",
        model: "EmitSection",
        required: &[],
        optional: &["prologue", "epilogue", "align_pad", "epilogue_label"],
        flatten: &[],
        additional: false,
        doc: "序言/尾声块引用",
    },
    Section {
        path: "[spill.<name>]",
        model: "SpillTemplate",
        required: &[],
        optional: &["load", "store", "base", "only_variants"],
        flatten: &[],
        additional: false,
        doc: "溢出/回填模板（`{N}` = 寄存器序号占位符）",
    },
    Section {
        path: "[[vectors]]",
        model: "Vector",
        required: &[],
        optional: &["asm", "bytes", "error", "partial", "comment"],
        flatten: &[],
        additional: false,
        doc: "数据化测试向量（v19 V3）：`{asm, bytes}` 正向 / `{asm, error}` 汇编错误 / `{bytes, error = \"DECODE\", partial}` 解码错误 / `{bytes}` 解码正向",
    },
    // ── 嵌套子表（TOML 内联或子节）──
    Section {
        path: "enc / vex / evex / modrm（内联子表）",
        model: "ModrmMap",
        required: &[],
        optional: &["reg", "rm"],
        flatten: &[],
        additional: false,
        doc: "`modrm = { reg = <名|整数>, rm = <名|\"[base]\"> }`",
    },
    Section {
        path: "vex / evex（内联子表）",
        model: "VexSpec",
        required: &[],
        optional: &["map", "pp", "w", "l", "b", "z", "disp_scale"],
        flatten: &[],
        additional: false,
        doc: "VEX/EVEX 结构键（map/pp/w/l + AVX-512 的 b/z/disp_scale）",
    },
    Section {
        path: "[[templates.rows]]",
        model: "TemplateRow",
        required: &["inst"],
        optional: &[],
        flatten: &[],
        additional: true,
        doc: "模板行：`inst` + 任意指令字段（含 `ref`）",
    },
    Section {
        path: "[emit.<block>]",
        model: "EmitBlock",
        required: &[],
        optional: &["insts", "only_variants"],
        flatten: &[],
        additional: false,
        doc: "序言/尾声块内容",
    },
    Section {
        path: "[[override]]",
        model: "OverrideDef",
        required: &["key", "value"],
        optional: &[],
        flatten: &[],
        additional: false,
        doc: "多文件组合：显式覆盖被包含文件里的键（点分路径 + 新值；由 loader 消费）",
    },
];

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_arr(items: &[&str]) -> String {
    let body: Vec<String> = items.iter().map(|s| json_str(s)).collect();
    format!("[{}]", body.join(","))
}

/// 生成 JSON Schema（draft 2020-12）文本。
pub fn schema_json() -> String {
    // 每个节一个 `$defs` 条目：properties/required/additionalProperties。
    let mut defs: Vec<String> = Vec::new();
    for (i, s) in SECTIONS.iter().enumerate() {
        let mut props: Vec<String> = Vec::new();
        for k in s.required.iter().chain(s.optional.iter()) {
            props.push(format!(
                "{}:{{\"description\":{}}}",
                json_str(k),
                json_str(&format!("{}.{}", s.path, k))
            ));
        }
        for k in s.flatten {
            props.push(format!(
                "{}:{{\"description\":{}}}",
                json_str(k),
                json_str(&format!("{}.{}（编码键，可与 form 预设混用）", s.path, k))
            ));
        }
        defs.push(format!(
            "{}:{{\"title\":{},\"description\":{},\"type\":\"object\",\"properties\":{{{}}},\
             \"required\":{},\"additionalProperties\":{}}}",
            json_str(&format!("section{i}")),
            json_str(s.path),
            json_str(s.doc),
            props.join(","),
            json_arr(s.required),
            if s.additional { "true" } else { "false" }
        ));
    }
    // 根：按 TOML 顶层键把节指过去（数组节用 items 指）。
    let mut root_props: Vec<String> = Vec::new();
    let find = |path: &str| -> usize {
        SECTIONS
            .iter()
            .position(|s| s.path == path)
            .expect("schema 表自洽")
    };
    let ref_of = |i: usize| format!("{{\"$ref\":\"#/$defs/section{i}\"}}");
    let arr_of = |i: usize| format!("{{\"type\":\"array\",\"items\":{}}}", ref_of(i));
    root_props.push(format!("{}:{}", json_str("meta"), ref_of(find("[meta]"))));
    root_props.push(format!(
        "{}:{}",
        json_str("encoding"),
        ref_of(find("[encoding]"))
    ));
    root_props.push(format!(
        "{}:{{\"type\":\"object\",\"additionalProperties\":{}}}",
        json_str("reg"),
        ref_of(find("[reg.<name>]"))
    ));
    for (key, path, array) in [
        ("types", "[types]", false),
        ("conventions", "", false),
        ("stack", "[stack]", false),
        ("operand_slots", "[[operand_slots]]", true),
        ("forms", "[[forms]]", true),
        ("instructions", "[[instructions]]", true),
        ("templates", "[[templates]]", true),
        ("reloc", "[[reloc]]", true),
        ("derive", "[[derive]]", true),
        ("pseudo", "[[pseudo]]", true),
        ("lowering", "[[lowering]]", true),
        ("pattern", "[[pattern]]", true),
        ("abi", "[abi]", false),
        ("emit", "[emit]", false),
        ("spill", "", false),
        ("vectors", "[[vectors]]", true),
    ] {
        if path.is_empty() {
            // 自由表（types/conventions/spill）：只声明是表。
            root_props.push(format!(
                "{}:{{\"type\":\"object\",\"description\":{}}}",
                json_str(key),
                json_str(match key {
                    "types" => "类型 → 寄存器组名 / \"unsupported\"",
                    "conventions" => "bitfields / modrm / cond / prefix_scan / mem",
                    _ => "溢出模板表（键 = 名字）",
                })
            ));
            continue;
        }
        let i = find(path);
        root_props.push(format!(
            "{}:{}",
            json_str(key),
            if array { arr_of(i) } else { ref_of(i) }
        ));
    }
    format!(
        "{{\n  \"$schema\": \"https://json-schema.org/draft/2020-12/schema\",\n  \
         \"$id\": \"https://github.com/Yuanzhumoyu/code_forge/isa-dsl.schema.json\",\n  \
         \"title\": \"ISA-DSL v18 谱（TOML）\",\n  \
         \"description\": \"由 forge-isa-dsl 的 schema 表发射（v18 S7c）；三方守卫见 crates/frontend/forge-isa-dsl/tests/schema_guard.rs\",\n  \
         \"type\": \"object\",\n  \"properties\": {{{}}},\n  \
         \"required\": {},\n  \
         \"$defs\": {{{}\n  }}\n}}\n",
        root_props.join(",\n    "),
        json_arr(&["meta"]),
        defs.join(",\n    ")
    )
}

/// schema 的 `(节路径, 键)` 列表（守卫与 CLI 报告用）。
pub fn schema_keys() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for s in SECTIONS {
        for k in s
            .required
            .iter()
            .chain(s.optional.iter())
            .chain(s.flatten.iter())
        {
            out.push((s.path, *k));
        }
    }
    out
}

/// 键速查表（Markdown）：`docs/reference/isa-dsl.md` 里由守卫**逐字校验**的那一段
/// ——文档与 schema 由此机械对齐（改 schema 必须重新生成这一段）。
///
/// 两个 Markdown 细节（踩过）：说明里的 `|` 必须转义成 `\|`（否则表格串列，MD056）；
/// 编码键用 `†` 而不是 `*` 标注（`*` 会被当成强调标记，MD037）。
pub fn markdown_table() -> String {
    let mut out = String::new();
    out.push_str("| 节 | 必填 | 可选（`†` = 编码键，可直接写在指令/form 上） | 说明 |\n");
    out.push_str("| --- | --- | --- | --- |\n");
    for s in SECTIONS {
        let req = if s.required.is_empty() {
            "—".to_string()
        } else {
            s.required
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut opt: Vec<String> = s.optional.iter().map(|k| format!("`{k}`")).collect();
        opt.extend(s.flatten.iter().map(|k| format!("`{k}`†")));
        let opt = if opt.is_empty() {
            "—".to_string()
        } else {
            opt.join(" ")
        };
        let extra = if s.additional {
            "（允许额外键）"
        } else {
            ""
        };
        let doc = s.doc.replace('|', "\\|");
        out.push_str(&format!(
            "| `{}` | {} | {} | {}{} |\n",
            s.path, req, opt, doc, extra
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_structurally_sane() {
        let s = schema_json();
        assert!(s.starts_with('{') && s.trim_end().ends_with('}'), "{s}");
        assert!(s.contains("\"$schema\""), "{s}");
        assert!(s.contains("\"encoding\""), "{s}");
        // 括号/引号配平：完整 JSON 合法性由 `tests/schema_guard.rs` 的校验器覆盖
        // （测试里不引 JSON 库，与"不新增依赖"一致）。
        let (mut braces, mut brackets) = (0i32, 0i32);
        let (mut in_str, mut escaped) = (false, false);
        for c in s.chars() {
            if in_str {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_str = false;
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '{' => braces += 1,
                '}' => braces -= 1,
                '[' => brackets += 1,
                ']' => brackets -= 1,
                _ => {}
            }
            assert!(
                braces >= 0 && brackets >= 0,
                "括号不配平（brace={braces} bracket={brackets}）"
            );
        }
        assert!(!in_str, "字符串未闭合");
        assert_eq!((braces, brackets), (0, 0), "括号不配平");
    }

    #[test]
    fn sections_are_self_consistent() {
        // 每个节的 required 不得同时出现在 optional 里；节路径不重复。
        let mut paths: Vec<&str> = Vec::new();
        for s in SECTIONS {
            for r in s.required {
                assert!(
                    !s.optional.contains(r),
                    "{}: `{r}` 同时在 required 与 optional",
                    s.path
                );
                assert!(!s.flatten.contains(r), "{}: `{r}` 重复", s.path);
            }
            assert!(!paths.contains(&s.path), "节路径重复：{}", s.path);
            paths.push(s.path);
        }
    }

    #[test]
    fn docs_pending_entries_are_in_schema() {
        // 文档欠账白名单已删除（S7c：文档里改为**逐字校验**的键速查表，
        // 由 tests/schema_guard.rs 与 markdown_table() 对照）。留此占位提醒：
        // 若将来需要"暂不文档化"的键，请恢复白名单并保留"只减不增"的断言。
    }
}
