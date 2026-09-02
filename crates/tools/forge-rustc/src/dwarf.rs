//! 最小 DWARF 生成（C1 DebugInfo line-tables-only）。
//!
//! `-C debuginfo=1`（line-tables-only）的最小语义：每个函数一个行号
//! 条目（函数起始地址 → 函数定义源码行）。生成三个标准段：
//! - `.debug_line`：行号程序（DWARF v2 最小形态，单 CU 单文件）
//! - `.debug_info`：单个编译单元 + 每个函数一个 `DW_TAG_subprogram`
//! - `.debug_abbrev`：缩写表（字符串属性用 DW_FORM_string 内联，
//!   避免跨段 strp 偏移依赖——最小可行，无需 .debug_str）
//!
//! 地址属性（low_pc）用相对地址 0 占位 + reloc，由 codegen_crate 在
//! 对象写入时按函数符号解析（ADDR64 重定位）。

/// DWARF 行号程序操作码（DWARF v2/v4）。
const DW_LNS_COPY: u8 = 0x01;
const DW_LNS_SET_FILE: u8 = 0x04;
const DW_LNE_END_SEQUENCE: u8 = 0x01;
const DW_LNE_SET_ADDRESS: u8 = 0x02;

/// C2 debuginfo（-C debuginfo=2 full）：单个源变量的 DIE 输入
///（lower 从 rustc `body.var_debug_info` 采集）。
#[derive(Clone, Debug)]
pub struct VarEntry {
    /// 变量名（rustc var_debug_info 的 Symbol）。
    pub name: String,
    /// 栈槽实际偏移（相对 rbp——含主库 callee-saved shift，直接作
    /// DW_OP_fbreg 操作数）。
    pub slot_offset: i32,
    /// 类型（rustc Debug 形态，如 "i32"/"&i32"，供 base_type 名匹配）。
    pub ty_desc: String,
    /// 是否函数参数（决定 DW_TAG_formal_parameter vs DW_TAG_variable）。
    pub is_arg: bool,
    /// 声明源码行（1-based，DW_AT_decl_line）。
    pub decl_line: u32,
}

/// C2 debuginfo：单函数的源变量表（按函数符号聚合）。
#[derive(Clone, Debug)]
pub struct FnVarEntries {
    /// 函数符号（mangled，与 add_function 的对象符号一致）。
    pub sym: String,
    /// 该函数的源变量列表。
    pub vars: Vec<VarEntry>,
}

/// 生成 `.debug_line` 段（单 CU、单文件、多函数条目）。
///
/// 每函数条目：`DW_LNE_set_address <addr>` + `DW_LNS_set_file` +
/// `DW_LNS_advance_line` + `DW_LNS_copy` + `DW_LNE_end_sequence`。
/// addr 是相对地址（0 占位，reloc 补）。file_names[0] = 源文件/库名
///（C2：当前为空串——调试器按文件索引回溯行号，真实文件名提高可用性）。
///
/// 返回 (段字节, reloc: (数据内偏移, 符号名))。
pub fn gen_debug_line(
    entries: &[(String, u32)],
    file_name: &str,
) -> (Vec<u8>, Vec<(usize, String)>) {
    let mut buf: Vec<u8> = Vec::new();
    let unit_len_pos = 0usize;
    buf.extend_from_slice(&0u32.to_le_bytes()); // unit_length 占位
    buf.push(2); // version（DWARF v2 行号版本，兼容最广）
    let hl_pos = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes()); // header_length 占位
    buf.push(1); // minimum_instruction_length
    buf.push(1); // default_is_stmt
    buf.push(1); // line_base
    buf.push(0); // line_range
    buf.push(1); // opcode_base = 1（只用 extended 操作码）
    // standard_opcode_lengths（opcode_base-1 = 0 个）
    // include_directories：空（单个目录 ""）
    buf.push(0);
    // file_names：file[0] = 源文件/库名（DW_FORM_string 变长）
    buf.extend_from_slice(file_name.as_bytes());
    buf.push(0);
    let header_end = buf.len();
    let hl = header_end - (hl_pos + 4);
    buf[hl_pos..hl_pos + 4].copy_from_slice(&(hl as u32).to_le_bytes());

    let mut relocs: Vec<(usize, String)> = Vec::new();
    let mut first = true;
    let mut prev_line = 0u64;
    for (sym, line) in entries {
        // set_address（extended opcode）
        buf.push(0); // extended
        buf.push(1 + 8); // operand length
        buf.push(DW_LNE_SET_ADDRESS);
        let addr_pos = buf.len();
        buf.extend_from_slice(&0u64.to_le_bytes()); // 占位（reloc 补）
        relocs.push((addr_pos, sym.clone()));
        // set_file 0
        buf.push(DW_LNS_SET_FILE);
        buf.push(0);
        // advance_line：相对上一函数行（delta）
        let delta = if first {
            *line as i64
        } else {
            (*line as i64) - (prev_line as i64)
        };
        encode_sleb128(&mut buf, delta);
        buf.push(DW_LNS_COPY);
        // end_sequence（extended）
        buf.push(0);
        buf.push(1);
        buf.push(DW_LNE_END_SEQUENCE);
        prev_line = *line as u64;
        first = false;
    }
    let unit_len = buf.len() - (unit_len_pos + 4);
    buf[unit_len_pos..unit_len_pos + 4].copy_from_slice(&(unit_len as u32).to_le_bytes());
    (buf, relocs)
}

/// 生成 `.debug_info` 段：单 CU（DW_TAG_compile_unit）+ 每函数一个
/// `DW_TAG_subprogram`。字符串属性用 DW_FORM_string（内联，无 strp）。
/// `full=true`（-C debuginfo=2）时 subprogram 带 children：
/// - DW_TAG_formal_parameter / DW_TAG_variable（DW_AT_location =
///   DW_OP_fbreg <槽偏移>，槽相对 rbp——frame base = rbp）
/// - DW_TAG_base_type（标量类型：name/byte_size/encoding，B3）
/// 返回 (字节, relocs: (偏移, 符号名))。
pub fn gen_debug_info(
    entries: &[(String, u32)],
    vars: &[FnVarEntries],
    producer: &str,
    cu_name: &str,
    full: bool,
) -> (Vec<u8>, Vec<(usize, String)>) {
    let mut buf: Vec<u8> = Vec::new();
    let unit_len_pos = 0usize;
    buf.extend_from_slice(&0u32.to_le_bytes()); // unit_length 占位
    buf.push(4); // version（DWARF v4）
    buf.extend_from_slice(&0u32.to_le_bytes()); // debug_abbrev_offset = 0
    buf.push(8); // address_size

    let mut relocs: Vec<(usize, String)> = Vec::new();

    // CU DIE：abbrev code 1（DW_TAG_compile_unit，children yes）
    buf.push(1);
    buf.extend_from_slice(producer.as_bytes());
    buf.push(0);
    buf.extend_from_slice(cu_name.as_bytes());
    buf.push(0);
    buf.extend_from_slice(&0x1cu16.to_le_bytes()); // DW_LANG_Rust

    // B3：base_type DIE（CU children，subprogram 之前——DW_AT_type(ref4)
    // 记录各 base_type 的段内偏移供 subprogram 变量引用）。
    let mut type_off_by_name: std::collections::HashMap<String, u32> = Default::default();
    if full {
        let mut seen: Vec<String> = Vec::new();
        for fv in vars {
            for v in &fv.vars {
                if let Some((dn, ..)) = scalar_base_type(&v.ty_desc)
                    && !seen.contains(&dn)
                {
                    seen.push(dn.clone());
                }
            }
        }
        for dn in &seen {
            let (_, size, enc) = scalar_base_type(dn).unwrap_or((dn.clone(), 8, 7));
            type_off_by_name.insert(dn.clone(), buf.len() as u32);
            buf.push(6); // abbrev code 6: DW_TAG_base_type
            buf.extend_from_slice(dn.as_bytes()); // DW_AT_name (string)
            buf.push(0);
            buf.push(size); // DW_AT_byte_size (data1)
            buf.push(enc); // DW_AT_encoding (data1)
        }
    }

    // 每函数 subprogram：abbrev code 2（无变量，children no）或
    // code 3（有变量，children yes——variable/formal_parameter 作 children）
    for (sym, line) in entries {
        let fn_vars: Vec<&VarEntry> = if full {
            vars
                .iter()
                .find(|fv| fv.sym == *sym)
                .map(|fv| fv.vars.iter().collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        buf.push(if fn_vars.is_empty() { 2 } else { 3 });
        // DW_AT_name (0x03) → string
        buf.extend_from_slice(sym.as_bytes());
        buf.push(0);
        // DW_AT_low_pc (0x11) → addr（占位 + reloc）
        let lp = buf.len();
        buf.extend_from_slice(&0u64.to_le_bytes());
        relocs.push((lp, sym.clone()));
        // DW_AT_decl_line (0x3b) → data4
        buf.extend_from_slice(&line.to_le_bytes());
        if !fn_vars.is_empty() {
            // DW_AT_frame_base (0x40) → exprloc：DW_OP_reg6（x86 rbp）
            buf.push(0x40);
            buf.push(1); // exprloc length
            buf.push(0x56); // DW_OP_reg6（0x50 + reg6）
            for v in fn_vars {
                // formal_parameter (abbrev 4) / variable (abbrev 5)
                buf.push(if v.is_arg { 4 } else { 5 });
                // DW_AT_name (0x03) → string
                buf.extend_from_slice(v.name.as_bytes());
                buf.push(0);
                // DW_AT_type (0x49) → ref4（base_type 段内偏移；无匹配 = 0）
                let tpos = buf.len();
                buf.extend_from_slice(&0u32.to_le_bytes());
                if let Some((dn, ..)) = scalar_base_type(&v.ty_desc)
                    && let Some(&off4) = type_off_by_name.get(&dn)
                {
                    buf[tpos..tpos + 4].copy_from_slice(&off4.to_le_bytes());
                }
                // DW_AT_location (0x02) → exprloc：DW_OP_fbreg <sleb128 offset>
                buf.push(0x02);
                let mut expr: Vec<u8> = Vec::new();
                expr.push(0x91); // DW_OP_fbreg
                encode_sleb128(&mut expr, v.slot_offset as i64);
                buf.push(expr.len() as u8);
                buf.extend_from_slice(&expr);
                // DW_AT_decl_line (0x3b) → data4
                buf.extend_from_slice(&v.decl_line.to_le_bytes());
            }
            buf.push(0); // children terminator
        }
    }
    buf.push(0); // CU children terminator
    let unit_len = buf.len() - (unit_len_pos + 4);
    buf[unit_len_pos..unit_len_pos + 4].copy_from_slice(&(unit_len as u32).to_le_bytes());
    (buf, relocs)
}

/// rustc Debug 类型名 → DWARF base_type（标量：name/size/encoding）。
/// 返回 None 表示非标量（聚合/引用/指针——C2 首版不设 type，gdb 按槽字节
/// 查看；指针/引用后续可加 pointer type）。
/// DW_ATE: address=1 boolean=2 float=4 signed=5 signed_char=6 unsigned=7
/// unsigned_char=8。
fn scalar_base_type(desc: &str) -> Option<(String, u8, u8)> {
    let d = desc.trim();
    let (name, size, enc) = match d {
        "i8" => (d, 1, 5),
        "i16" => (d, 2, 5),
        "i32" => (d, 4, 5),
        "i64" | "isize" => ("i64", 8, 5),
        "i128" => (d, 16, 5),
        "u8" => (d, 1, 7),
        "u16" => (d, 2, 7),
        "u32" => (d, 4, 7),
        "u64" | "usize" => ("u64", 8, 7),
        "u128" => (d, 16, 7),
        "f32" => (d, 4, 4),
        "f64" => (d, 8, 4),
        "bool" => (d, 1, 2),
        "char" => (d, 4, 8),
        "()" => (d, 0, 7),
        _ => return None,
    };
    Some((name.to_string(), size, enc))
}

/// 生成 `.debug_abbrev` 段（C1 + C2 的码）。
/// code 1 CU(yes) / 2 subprogram(no) / 3 subprogram(yes) /
/// 4 formal_parameter(no) / 5 variable(no) / 6 base_type(no)。
pub fn gen_debug_abbrev() -> Vec<u8> {
    let mut buf = Vec::new();
    // code 1：CU，children yes
    buf.push(1);
    buf.push(0x11);
    buf.push(1);
    buf.push(0x25); buf.push(0x08); // producer → string
    buf.push(0x03); buf.push(0x08); // name → string
    buf.push(0x13); buf.push(0x05); // language → data2
    buf.push(0); buf.push(0);
    // code 2：subprogram，children no（无变量函数——C1 兼容）
    buf.push(2);
    buf.push(0x2e);
    buf.push(0);
    buf.push(0x03); buf.push(0x08); // name → string
    buf.push(0x11); buf.push(0x01); // low_pc → addr
    buf.push(0x3b); buf.push(0x06); // decl_line → data4
    buf.push(0); buf.push(0);
    // code 3：subprogram，children yes（有变量）
    buf.push(3);
    buf.push(0x2e);
    buf.push(1);
    buf.push(0x03); buf.push(0x08); // name → string
    buf.push(0x11); buf.push(0x01); // low_pc → addr
    buf.push(0x3b); buf.push(0x06); // decl_line → data4
    buf.push(0x40); buf.push(0x0a); // frame_base → exprloc
    buf.push(0); buf.push(0);
    // code 4：formal_parameter，children no
    buf.push(4);
    buf.push(0x05);
    buf.push(0);
    buf.push(0x03); buf.push(0x08); // name → string
    buf.push(0x49); buf.push(0x06); // type → ref4
    buf.push(0x02); buf.push(0x0a); // location → exprloc
    buf.push(0x3b); buf.push(0x06); // decl_line → data4
    buf.push(0); buf.push(0);
    // code 5：variable，children no
    buf.push(5);
    buf.push(0x34);
    buf.push(0);
    buf.push(0x03); buf.push(0x08); // name → string
    buf.push(0x49); buf.push(0x06); // type → ref4
    buf.push(0x02); buf.push(0x0a); // location → exprloc
    buf.push(0x3b); buf.push(0x06); // decl_line → data4
    buf.push(0); buf.push(0);
    // code 6：base_type，children no
    buf.push(6);
    buf.push(0x24);
    buf.push(0);
    buf.push(0x03); buf.push(0x08); // name → string
    buf.push(0x0b); buf.push(0x0b); // byte_size → data1
    buf.push(0x3e); buf.push(0x0b); // encoding → data1
    buf.push(0); buf.push(0);
    buf.push(0); // 整个 abbrev 表终止
    buf
}

/// 生成全部 DWARF 段。
/// 返回 (段名, 字节, 段内 reloc: (偏移, 符号名)) 列表。
pub fn build_dwarf_sections(
    entries: &[(String, u32)],
    vars: &[FnVarEntries],
    producer: &str,
    cu_name: &str,
    full: bool,
) -> Vec<(String, Vec<u8>, Vec<(usize, String)>)> {
    let (line_bytes, line_relocs) = gen_debug_line(entries, cu_name);
    let (info_bytes, info_relocs) = gen_debug_info(entries, vars, producer, cu_name, full);
    vec![
        (".debug_line".to_string(), line_bytes, line_relocs),
        (".debug_info".to_string(), info_bytes, info_relocs),
        (".debug_abbrev".to_string(), gen_debug_abbrev(), vec![]),
    ]
}

/// 编码 SLEB128。
fn encode_sleb128(buf: &mut Vec<u8>, mut v: i64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        let done = (v == 0 && byte & 0x40 == 0) || (v == -1 && byte & 0x40 != 0);
        buf.push(byte | if done { 0 } else { 0x80 });
        if done {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_program_basic() {
        let entries = vec![
            ("main".to_string(), 10u32),
            ("helper".to_string(), 20u32),
        ];
        let (bytes, relocs) = gen_debug_line(&entries, "test_crate");
        assert!(bytes.len() > 24, "line program too small");
        assert_eq!(relocs.len(), 2, "one reloc per function");
        assert_eq!(relocs[0].1, "main");
        // file_names[0] = 库名（非空——C2 增强）
        assert!(
            bytes.windows(10).any(|w| w == b"test_crate"),
            "file_names[0] should contain crate name"
        );
        // unit_length 非 0
        let unit_len = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        assert_eq!(unit_len as usize, bytes.len() - 4);
    }

    #[test]
    fn info_has_cu_and_subprograms() {
        let entries = vec![("f".to_string(), 5u32)];
        let (bytes, relocs) = gen_debug_info(&entries, &[], "forge", "test", false);
        assert_eq!(relocs.len(), 1);
        // header：unit_length(4) + version(2) + abbr_off(4) + addr_size(1) = 11
        // CU DIE abbrev code = 1 在 index 10
        assert_eq!(bytes[10], 1);
        // CU 后子项 terminator(1) + subprogram abbrev code = 2
        assert!(bytes.windows(1).any(|w| w[0] == 2), "subprogram abbrev 2 present");
    }

    #[test]
    fn full_info_has_vars_and_base_types() {
        // C2：-C debuginfo=2 —— subprogram code 3（children yes）带变量
        // child（code 4/5）+ frame_base（0x40）+ base_type（code 6）区。
        let entries = vec![("f".to_string(), 5u32)];
        let vars = vec![FnVarEntries {
            sym: "f".to_string(),
            vars: vec![
                VarEntry {
                    name: "x".to_string(),
                    slot_offset: -24,
                    ty_desc: "i32".to_string(),
                    is_arg: false,
                    decl_line: 7,
                },
                VarEntry {
                    name: "a".to_string(),
                    slot_offset: -32,
                    ty_desc: "i32".to_string(),
                    is_arg: true,
                    decl_line: 6,
                },
                VarEntry {
                    name: "b".to_string(),
                    slot_offset: -16,
                    ty_desc: "&i32".to_string(),
                    is_arg: true,
                    decl_line: 6,
                },
            ],
        }];
        let (bytes, relocs) = gen_debug_info(&entries, &vars, "forge", "test", true);
        assert_eq!(relocs.len(), 1, "one fn low_pc reloc");
        // base_type "i32"（code 6 在 CU 后）出现——字节里含 "i32\0"
        assert!(
            bytes.windows(5).any(|w| w == b"i32\0\x04"),
            "base_type i32 byte_size=4 present"
        );
        // 变量名 "x" / "a" 出现（subprogram children——name 后跟 \0）
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("x\u{0}"), "variable name x present");
        assert!(s.contains("a\u{0}"), "param name a present");
        // fbreg 表达式（0x91 = DW_OP_fbreg）出现
        assert!(bytes.windows(1).any(|w| w[0] == 0x91), "DW_OP_fbreg present");
    }
}
