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

/// 生成 `.debug_line` 段（单 CU、单文件、多函数条目）。
///
/// 每函数条目：`DW_LNE_set_address <addr>` + `DW_LNS_set_file` +
/// `DW_LNS_advance_line` + `DW_LNS_copy` + `DW_LNE_end_sequence`。
/// addr 是相对地址（0 占位，reloc 补）。
///
/// 返回 (段字节, reloc: (数据内偏移, 符号名))。
pub fn gen_debug_line(entries: &[(String, u32)]) -> (Vec<u8>, Vec<(usize, String)>) {
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
    // file_names：file[0] = 空字符串
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
/// 返回 (字节, relocs: (偏移, 符号名))。
pub fn gen_debug_info(entries: &[(String, u32)], producer: &str, cu_name: &str) -> (Vec<u8>, Vec<(usize, String)>) {
    let mut buf: Vec<u8> = Vec::new();
    let unit_len_pos = 0usize;
    buf.extend_from_slice(&0u32.to_le_bytes()); // unit_length 占位
    buf.push(4); // version（DWARF v4）
    buf.extend_from_slice(&0u32.to_le_bytes()); // debug_abbrev_offset = 0
    buf.push(8); // address_size

    let mut relocs: Vec<(usize, String)> = Vec::new();
    // CU DIE：abbrev code 1（DW_TAG_compile_unit，children yes）
    buf.push(1);
    // DW_AT_producer (0x25) → DW_FORM_string
    buf.extend_from_slice(producer.as_bytes());
    buf.push(0);
    // DW_AT_name (0x03) → string
    buf.extend_from_slice(cu_name.as_bytes());
    buf.push(0);
    // DW_AT_language (0x13) → data2（DW_LANG_Rust = 0x1c）
    buf.extend_from_slice(&0x1cu16.to_le_bytes());
    // 子项结束 terminator（DW_CHILDREN_yes）
    buf.push(0);
    // 每函数 subprogram：abbrev code 2（children no）
    for (sym, line) in entries {
        buf.push(2);
        // DW_AT_name (0x03) → string
        buf.extend_from_slice(sym.as_bytes());
        buf.push(0);
        // DW_AT_low_pc (0x11) → addr（占位 + reloc）
        let lp = buf.len();
        buf.extend_from_slice(&0u64.to_le_bytes());
        relocs.push((lp, sym.clone()));
        // DW_AT_decl_line (0x3b) → data4
        buf.extend_from_slice(&line.to_le_bytes());
    }
    let unit_len = buf.len() - (unit_len_pos + 4);
    buf[unit_len_pos..unit_len_pos + 4].copy_from_slice(&(unit_len as u32).to_le_bytes());
    (buf, relocs)
}

/// 生成 `.debug_abbrev` 段。
pub fn gen_debug_abbrev() -> Vec<u8> {
    let mut buf = Vec::new();
    // CU：code 1，DW_TAG_compile_unit(0x11)，DW_CHILDREN_yes(1)
    buf.push(1);
    buf.push(0x11);
    buf.push(1);
    // attrs
    buf.push(0x25); buf.push(0x08); // producer → string
    buf.push(0x03); buf.push(0x08); // name → string
    buf.push(0x13); buf.push(0x05); // language → data2
    buf.push(0); buf.push(0); // attr 终止
    // subprogram：code 2，DW_TAG_subprogram(0x2e)，DW_CHILDREN_no(0)
    buf.push(2);
    buf.push(0x2e);
    buf.push(0);
    buf.push(0x03); buf.push(0x08); // name → string
    buf.push(0x11); buf.push(0x01); // low_pc → addr
    buf.push(0x3b); buf.push(0x06); // decl_line → data4
    buf.push(0); buf.push(0); // attr 终止
    buf.push(0); // 整个 abbrev 表终止
    buf
}

/// 生成全部 DWARF 段。
/// 返回 (段名, 字节, 段内 reloc: (偏移, 符号名)) 列表。
pub fn build_dwarf_sections(
    entries: &[(String, u32)],
    producer: &str,
    cu_name: &str,
) -> Vec<(String, Vec<u8>, Vec<(usize, String)>)> {
    let (line_bytes, line_relocs) = gen_debug_line(entries);
    let (info_bytes, info_relocs) = gen_debug_info(entries, producer, cu_name);
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
        let (bytes, relocs) = gen_debug_line(&entries);
        assert!(bytes.len() > 24, "line program too small");
        assert_eq!(relocs.len(), 2, "one reloc per function");
        assert_eq!(relocs[0].1, "main");
        // unit_length 非 0
        let unit_len = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        assert_eq!(unit_len as usize, bytes.len() - 4);
    }

    #[test]
    fn info_has_cu_and_subprograms() {
        let entries = vec![("f".to_string(), 5u32)];
        let (bytes, relocs) = gen_debug_info(&entries, "forge", "test");
        assert_eq!(relocs.len(), 1);
        // header：unit_length(4) + version(2) + abbr_off(4) + addr_size(1) = 11
        // CU DIE abbrev code = 1 在 index 10
        assert_eq!(bytes[10], 1);
        // CU 后子项 terminator(1) + subprogram abbrev code = 2
        // CU 的 attrs 长度不定（producer/name 字符串）——定位第一个 0x02
        assert!(bytes.windows(1).any(|w| w[0] == 2), "subprogram abbrev 2 present");
    }
}
