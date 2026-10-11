//! `schema` — ISA-DSL TOML 的 **JSON Schema**（v18 S7c，手写发射器）。
//!
//! 为什么手写：方案 §10.4 明确"不新增依赖"（不引 `schemars`）。手写的代价是**可能与
//! 模型漂移**，因此配一条三方守卫
//! （`crates/frontend/forge-isa-dsl/tests/schema_guard.rs`）：
//!
//! 1. **模型 ↔ schema**：本文件的键清单逐节与 `dsl/model.rs` 的结构体 `pub` 字段
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
    /// 与 `dsl/model.rs` 里哪个结构体对照（空 = 自由形态，不走模型对照）。
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
];

/// 全部节（含嵌套子表）。守卫按 `model` 字段与模型结构体逐键对照。
pub const SECTIONS: &[Section] = &[
    Section {
        path: "<root>",
        model: "IsaModel",
        required: &["meta"],
        optional: &[
            "include",
            "override",
            "encoding",
            "reg",
            "conventions",
            "operand",
            "enum",
            "form",
            "inst",
            "family",
            "lower",
            "asm",
            "pred",
            "machine",
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
            "vector_tiers",
        ],
        flatten: &[],
        additional: false,
        doc: "元信息 + 文本约定（宽度只在 [reg.<名>] 表头里，见 [machine] 的类指针键）",
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
        optional: &[
            "names",
            "prefix",
            "base_index",
            "count",
            "alloc_count",
            "aliases",
        ],
        flatten: &[],
        additional: false,
        doc: "寄存器组；组名的数字 = **位**宽（`gpr64` = 64 位）；aliases = { 别名 = 组内下标 }",
    },
    Section {
        path: "[machine.stack]",
        model: "MachineStack",
        required: &[],
        optional: &["slot", "align", "fp_save"],
        flatten: &[],
        additional: false,
        doc: "栈槽单位/对齐/帧指针保存槽（v21 W9：从 `[stack]` 迁入，**单位是位**，解析期 ÷8；缺省全部派生）",
    },
    Section {
        path: "[machine.classes]",
        model: "",
        required: &[],
        optional: &[],
        flatten: &[],
        additional: true,
        doc: "类型 → 寄存器组名（或 \"unsupported\"）的显式映射（v21 W9：从 `[types]` 迁入）；键 = 类型名",
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
        path: "[conventions.bitsets.<table>]",
        model: "",
        required: &[],
        optional: &[],
        flatten: &[],
        additional: true,
        doc: "命名位集合表（kind = \"bits\" 的槽用）：表名 → （名字 → 位）——源文本是名字拼接，编码取位或",
    },
    Section {
        path: "[conventions.imm_names.<table>]",
        model: "",
        required: &[],
        optional: &[],
        flatten: &[],
        additional: true,
        doc: "命名立即数表（kind = \"imm\" 的槽用 `names` 指名）：表名 → （名字 → 值）——一个名字 = 一个值；解析认名字也认数字，渲染时值在表里就写名字",
    },
    Section {
        path: "[conventions.mem]",
        model: "MemTemplate",
        required: &["templates"],
        optional: &[],
        flatten: &[],
        additional: false,
        doc: "内存操作数文本模板列表（第 0 条 = 渲染形态，其余解析专用备选；占位符 base/index/scale/disp/size）",
    },
    Section {
        // v21 W3b：命名取值表（键名即表名）——`[enum.<表名>]`。
        path: "[enum.<表名>]",
        model: "EnumDecl",
        required: &[],
        optional: &["kind"],
        // 条目表是 `#[serde(flatten)]` 的自由表（名字 → 值/`{ code, ir }`），键名任意。
        flatten: &["entries"],
        additional: true,
        doc: "命名取值表：kind = value（名字 = 值；带 `ir` 的那张是条件码表）| bits（名字拼接、按位或）",
    },
    Section {
        // v21 W3：操作数声明是**表**（键名即槽名）——`[operand.<名字>]`。
        path: "[operand.<名字>]",
        model: "OperandDecl",
        required: &["kind"],
        optional: &[
            "bits",
            "signed",
            "range",
            "unit",
            "value",
            "literal",
            "enum",
            "suffix",
            "symbol",
            "byte",
            "zero",
            "sp",
            "text",
            "size_words",
            "encode",
            "fields",
            "roles",
            "class",
            "cond_bank",
        ],
        flatten: &[],
        additional: false,
        doc: "操作数槽：kind = reg | imm | mem | label | cond",
    },
    Section {
        path: "[form.<名字>]",
        model: "Form",
        required: &[],
        optional: &["fields", "segments", "doc"],
        // Form 的编码键是 `#[serde(flatten)]` 的 EncKeys：TOML 里与本节的键同级。
        flatten: ENC_KEYS,
        additional: false,
        doc: "编码形式：可选的键预设（指令可逐键覆盖）；v21 W10：**表头即形式名**（不再写 `name`）",
    },
    Section {
        path: "[inst.<名字>]",
        model: "Instruction",
        required: &["asm"],
        optional: &[
            "form",
            "opcode",
            "match",
            "fields",
            "segments",
            "bind",
            "ops",
            "when",
            "effect",
            "roles",
            "data_width",
            "word_size",
            "implicit_regs",
            "reloc",
            "width",
            "only_variants",
            "fields_variant",
            "doc",
            // TOML 键是 `ref`（模型字段名 `reference` + `#[serde(rename = "group")]`）。
            // 守卫按 `#[serde(rename = …)]` 取键名，所以这里必须写 **用户在 TOML 里
            // 实际写的那个键**——写字段名会让编辑器对每一行 `group = …` 报未知键
            // （2026-09-21 实测：`isa/x86.toml` 的 35 处 `ref` 全部被 Taplo 标红）。
            "group",
        ],
        // 编码键是 `#[serde(flatten)]` 的 EncKeys：直接在指令上写（不写 `enc = {...}`）。
        flatten: ENC_KEYS,
        additional: false,
        doc: "指令（v21 W10：**表头即指令名**，不再写 `name`）：编码键可与 form 预设混用（指令优先）；width = 指令字长（位），data_width = 数据宽度（位，搬运族派生的唯一人写数据）",
    },
    Section {
        path: "[asm.reloc.<名>]",
        model: "RelocBody",
        required: &["semantics", "operand"],
        optional: &["addend", "via"],
        flatten: &[],
        additional: false,
        doc: "重定位（v21 W8：**表头即重定位名**；旧 `[[reloc]]` 已撤）：`semantics` = 宿主语义（absolute/pc_relative）、`operand` = 绑定的 imm 槽名、`addend` = 链接期加减、`via` = 值变换（引用 `[asm.immfn.<名>]`）",
    },
    Section {
        path: "[pred.<名字>]",
        model: "DeriveDef",
        required: &["expr"],
        optional: &[],
        flatten: &[],
        additional: false,
        doc: "命名谓词（v18 S3f 引入时叫 `[[derive]]`；v21 W10：**表头即属性名**，不再写 `name`）",
    },
    Section {
        path: "[asm.pseudo.<名>]",
        model: "PseudoBody",
        required: &["asm", "emit"],
        optional: &["only_variants"],
        flatten: &[],
        additional: false,
        doc: "汇编器伪指令（v21 W7：**表头即名字**；旧 `[[pseudo]]` 已撤）：文本级多指令展开，分派按 `asm` 的前导字面，`{名字}` 就是 `emit` 用的参数",
    },
    Section {
        path: "[asm.immfn.<名>]",
        model: "ImmFnBody",
        required: &["texts", "expr"],
        optional: &[],
        flatten: &[],
        additional: false,
        doc: "立即数修饰（v21 W7：**表头即修饰名**；旧 `[[conventions.imm_fn]]` 待撤）：`texts` = 同一语义的等价写法列表（`{0}` 是内层表达式），`expr` = 值语义（`{0}` 是内层值）",
    },
    Section {
        path: "[[lower.<名字>]]",
        model: "LowerDecl",
        required: &["insts"],
        optional: &[
            "match",
            "when",
            "vary",
            "dispatch",
            "temps",
            "priority",
            "only_variants",
            "move",
            "movef",
        ],
        flatten: &[],
        additional: false,
        doc: "选择层（v21 W6-a：**表头即名字、同名可多块**）——名字是终结 op（Ret/Jmp/Br/Switch/Unreachable/Invoke）⇒ 终结序列（可用 {true}/{false}/{target} 符号值，且只吃 `insts`）；写了 `match` ⇒ 树型多指令匹配（名字是作者起的模式名）；其余 ⇒ op 规则（**名字就是 op 名**；一个 op 多条规则就写多块）",
    },
    Section {
        path: "[machine]",
        model: "MachineSection",
        required: &[],
        optional: &[
            "fixed",
            "scratch",
            "link",
            "gpr",
            "fpr",
            "addr",
            "value_gpr",
            "value_fpr",
            "frame",
            "padding",
            "arg_place",
            "byref_bytes",
            "stack",
            "text",
            "classes",
            "spill",
        ],
        flatten: &[],
        additional: false,
        doc: "机器事实（固定用途寄存器 / 溢出 scratch / 链接寄存器 / 五个**寄存器类指针**（组名，宽度在组名里） / 帧形状 / 帧填充 / 位置计数规则；v21 W9：`fixed`/`scratch`/`link`/`padding`/`arg_place`/`byref_bytes` 由 `fixed_regs`/`spill_scratch`/`link_reg`/`frame_padding`/`arg_slot`/`vector_by_ref_bytes` 改名，接收从 `[stack]`/`[emit]`/`[types]` 迁入的 `stack`/`text`/`classes`，并删除 `callee_saved_gpr`/`callee_save_slots`——callee-saved 完全归约定数据）",
    },
    Section {
        path: "[machine.frame]",
        model: "AbiFrame",
        required: &["sp"],
        optional: &["fp", "layout", "fp_push_bytes", "alloc_neg"],
        flatten: &[],
        additional: false,
        doc: "帧形状：栈指针/帧指针寄存器名 + 布局模式 + 帧指针保存槽（唯一位置；`[abi.frame]` 已删除）",
    },
    Section {
        path: "[machine.text]",
        model: "EmitSection",
        required: &[],
        optional: &["align_pad", "epilogue_label"],
        flatten: &[],
        additional: false,
        doc: "代码对齐填充与尾声标签（v21 W9：从 `[emit]` 迁入；序/尾声由生成器按约定生成，谱里不再写）",
    },
    Section {
        path: "[machine.spill.<模板名>]",
        model: "MachineSpill",
        required: &["class", "load", "store"],
        optional: &["base", "when"],
        flatten: &[],
        additional: false,
        doc: "溢出/回填模板（v21 W9：表头 = 模板名，`class` = 它服务的寄存器类；`{N}` = 寄存器序号占位符）",
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

/// 节的描述文本。
///
/// `[[conventions.prefix_scan]]` 的**效果清单**从 `PrefixEffect::NAMES` 现取：效果名是
/// **闭集**，取值域的唯一来源是那个枚举（解析、诊断、schema 共用一份）——在 schema 的
/// 描述里再抄一遍名字，加一个效果就会漏一处。
fn section_doc(s: &Section) -> String {
    if s.path == "[[conventions.prefix_scan]]" {
        let names: Vec<String> = crate::dsl::model::PrefixEffect::NAMES
            .iter()
            .map(|n| format!("`{n}`"))
            .collect();
        return format!("{}；效果清单 = {}", s.doc, names.join(" "));
    }
    s.doc.to_string()
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
            json_str(&section_doc(s)),
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
        ("conventions", "", false),
        ("operands", "[operand.<名字>]", true),
        ("enums", "[enum.<表名>]", true),
        ("form", "[form.<名字>]", true),
        ("inst", "[inst.<名字>]", true),
        ("pred", "[pred.<名字>]", true),
        ("asm", "", false),
        ("machine", "[machine]", false),
        ("vectors", "[[vectors]]", true),
    ] {
        if path.is_empty() {
            // 自由表（types/conventions/spill）：只声明是表。
            root_props.push(format!(
                "{}:{{\"type\":\"object\",\"description\":{}}}",
                json_str(key),
                json_str(match key {
                    "types" => "类型 → 寄存器组名 / \"unsupported\"",
                    "conventions" => "bitfields / modrm / cond / prefix_scan / bitsets / imm_names / imm_fn / mem",
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
        let doc = section_doc(s).replace('|', "\\|");
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
