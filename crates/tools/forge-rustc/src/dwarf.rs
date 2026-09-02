//! DWARF v4 生成（C1 line-tables-only + C2 full 变量级）。
//!
//! 段：
//! - `.debug_line`：行号程序（v4 头，clang `--dwarf=rawline` 对照：
//!   opcode_base=13 + standard_opcode_lengths；每函数函数级条目 +
//!   per-statement 条目（B1）——每条目自成一个 sequence）
//! - `.debug_info`：单 CU（name=源文件路径、low_pc/high_pc/stmt_list）
//!   + 每函数 `DW_TAG_subprogram`（C2：frame_base + 变量/参数 DIE +
//!   base_type/pointer_type）
//! - `.debug_aranges`：单 CU 地址范围（gdb 16 cooked index 的 pc→CU 映射）
//! - `.debug_abbrev`：缩写表（字符串属性用 DW_FORM_string 内联，无 strp）
//!
//! 地址属性（low_pc/set_address/aranges）用相对占位 + reloc，由
//! codegen_crate 在对象写入时按函数符号解析（ADDR64 重定位）。
//! 结构错误历史（WA-31）：version u16、DIE 值流误写属性名、行程序
//! 头不合法（opcode_base）、CU 范围/aranges 缺失、文件条目缺字段等。

/// DWARF 行号程序操作码（DWARF v4）。
const DW_LNS_COPY: u8 = 0x01;
const DW_LNS_ADVANCE_LINE: u8 = 0x03;
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
/// 每函数条目：`DW_LNE_set_address <addr>` + `DW_LNS_set_file 1` +
/// `DW_LNS_advance_line` + `DW_LNS_copy` + `DW_LNE_end_sequence`。
/// addr 是相对地址占位（reloc 补——COFF addend 隐式：占位字节即偏移，
/// 链接后 = 符号地址 + 占位值）。file_names[1] = 源文件/库名。
/// 每个函数：(符号, 函数声明行, per-statement (指令偏移, 行) 列表——
/// B1 行号细化：每语句一个 set_address(偏移) + 行条目，调试器可精确
/// 行断点/单步；空列表 = 仅函数级条目（C1 兼容）。
///
/// 头部布局与 clang -gdwarf 对齐（DWARF4 line v4，`objdump --dwarf=rawline`
/// 对照验证）：version u16、min_inst/max_ops/is_stmt、line_base=-5（有符号
/// 字节 0xfb，clang 同款——bfd 按有符号字节读）、line_range=14、
/// opcode_base=13 + 12 个 standard_opcode_lengths（opcode 1..12）。
/// opcode_base 若 < 5，copy/advance_line/set_file（opcode 1/3/4）会被当
/// special opcode 解码（行/地址错乱）；每条目自成一个 sequence——
/// end_sequence 后行寄存器复位为 1，故每行 advance_line 目标 = L - 1。
///
/// 返回 (段字节, reloc: (数据内偏移, 符号名))。
pub fn gen_debug_line(
    fns: &[(String, u32, Vec<(u32, u32)>)],
    file_name: &str,
) -> (Vec<u8>, Vec<(usize, String)>) {
    let mut buf: Vec<u8> = Vec::new();
    let unit_len_pos = 0usize;
    buf.extend_from_slice(&0u32.to_le_bytes()); // unit_length 占位
    // version 是 u16——只写 1 字节会让高位吃到 header_length 首字节。
    buf.push(4); // version（DWARF v4）
    buf.push(0); // version 高位
    let hl_pos = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes()); // header_length 占位
    buf.push(1); // minimum_instruction_length
    buf.push(1); // maximum_ops_per_instruction（DWARF v4）
    buf.push(1); // default_is_stmt
    buf.push(0xfb); // line_base = -5（sleb128 单字节，有符号读）
    buf.push(14); // line_range
    buf.push(13); // opcode_base——standard opcodes 1..12
    // standard_opcode_lengths（opcode 1..12）：
    // copy0 advance_pc1 advance_line1 set_file1 set_column1 negate0
    // basic_block0 const_add_pc0 fixed_advance_pc1 prologue0 epilogue0 set_isa1
    buf.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
    // include_directories：空（目录表终止符 = 1 个 NUL）
    buf.push(0);
    // file_names：file[1] = 源文件/库名。每文件条目 = name\0 +
    // dir_index(uleb) + mtime(uleb) + length(uleb)，表终止符 = 1 个 NUL。
    // 缺 dir/mtime/length 会让读者把行程序操作码当文件元数据顺序消费
    //（v4 头无 padding 可跳）——行号错乱/段错位。
    buf.extend_from_slice(file_name.as_bytes());
    buf.push(0);
    buf.push(0); // dir_index = 0（""，无目录表条目）
    buf.push(0); // last_modification_time = 0
    buf.push(0); // length = 0
    buf.push(0); // file_names 表终止符
    let header_end = buf.len();
    let hl = header_end - (hl_pos + 4);
    buf[hl_pos..hl_pos + 4].copy_from_slice(&(hl as u32).to_le_bytes());

    let mut relocs: Vec<(usize, String)> = Vec::new();
    for (sym, line, stmts) in fns {
        // 函数起点条目（reloc 占位 0 = 符号基址）
        push_line_entry(&mut buf, &mut relocs, sym, 0, *line as i64);
        // per-statement：set_address(指令偏移) + 行
        for (off, ln) in stmts {
            push_line_entry(&mut buf, &mut relocs, sym, *off as i64, *ln as i64);
        }
    }
    let unit_len = buf.len() - (unit_len_pos + 4);
    buf[unit_len_pos..unit_len_pos + 4].copy_from_slice(&(unit_len as u32).to_le_bytes());
    (buf, relocs)
}

/// 追加一个行号条目（自成一个 sequence）：set_address(addr 占位 + reloc)
/// + set_file 1 + advance_line(L-1) + copy + end_sequence。行寄存器在
/// end_sequence 后复位为 1，故每个新 sequence 的首行 delta = L - 1。
fn push_line_entry(
    buf: &mut Vec<u8>,
    relocs: &mut Vec<(usize, String)>,
    sym: &str,
    addr: i64,
    line: i64,
) {
    buf.push(0); // extended
    buf.push(1 + 8); // operand length
    buf.push(DW_LNE_SET_ADDRESS);
    let addr_pos = buf.len();
    // 占位写 addr（COFF addend 隐式——链接后 = 符号地址 + addr）
    buf.extend_from_slice(&(addr as u64).to_le_bytes());
    relocs.push((addr_pos, sym.to_string()));
    buf.push(DW_LNS_SET_FILE);
    buf.push(1); // file[1]——DWARF 文件号从 1 开始，0 非法
    buf.push(DW_LNS_ADVANCE_LINE);
    encode_sleb128(buf, line - 1); // 行寄存器 sequence 起点 = 1 → 目标 L
    buf.push(DW_LNS_COPY);
    buf.push(0); // extended
    buf.push(1);
    buf.push(DW_LNE_END_SEQUENCE);
}

/// 生成 `.debug_info` 段：单 CU（DW_TAG_compile_unit）+ 每函数一个
/// `DW_TAG_subprogram`。字符串属性用 DW_FORM_string（内联，无 strp）。
/// CU DIE 带 low_pc/high_pc（代码范围，reloc 到首函数符号 + data8 总长）——
/// gdb 无 .debug_aranges 时靠 CU 级范围把 pc 映射到 CU（缺则帧解析落回
/// COFF 符号 "__end__"，info args/locals 全空）。
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
    code_span: u64,
    full: bool,
) -> (Vec<u8>, Vec<(usize, String)>) {
    let mut buf: Vec<u8> = Vec::new();
    let unit_len_pos = 0usize;
    buf.extend_from_slice(&0u32.to_le_bytes()); // unit_length 占位
    buf.push(4); // version（DWARF v4）
    buf.push(0); // version 高位（DWARF version 是 u16——只写 1 字节会让
                 // abbrev_offset 字段错位：addr_size 的 08 混入 → gdb/
                 // objdump 报 bad offset 0x8000000）
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
    // DW_AT_low_pc（addr 占位 + reloc → 首函数符号）；DW_AT_high_pc
    //（data8 = 相对 low_pc 的偏移，覆盖全部函数代码 + 边距）
    let cu_lo = buf.len();
    buf.extend_from_slice(&0u64.to_le_bytes());
    if let Some((sym, _)) = entries.first() {
        relocs.push((cu_lo, sym.clone()));
    }
    buf.extend_from_slice(&code_span.to_le_bytes());
    // DW_AT_stmt_list（data4 = 0）：指向 .debug_line 偏移 0（gcc 同款——
    // gdb 按此定位行号程序）
    buf.extend_from_slice(&0u32.to_le_bytes());

    // 类型 DIE（CU children，subprogram 之前——DW_AT_type(ref4) 记录各
    // 类型 DIE 的段内偏移供 subprogram 变量引用）。顺序：base_type 先
    //（pointer 引用它们），pointer_type 后。key = 变量 ty_desc（去重）。
    let mut type_off_by_desc: std::collections::HashMap<String, u32> = Default::default();
    if full {
        // 收集唯一 ty_desc（可分类为标量/指针的）
        let mut descs: Vec<String> = Vec::new();
        for fv in vars {
            for v in &fv.vars {
                if ty_kind(&v.ty_desc).is_some() && !descs.contains(&v.ty_desc) {
                    descs.push(v.ty_desc.clone());
                }
            }
        }
        // 第一遍：base_type（标量 desc——scalar_base_type 直接命中）
        for desc in &descs {
            if let Some((_, size, enc)) = scalar_base_type(desc) {
                type_off_by_desc.insert(desc.clone(), buf.len() as u32);
                buf.push(6); // abbrev code 6: DW_TAG_base_type
                buf.extend_from_slice(desc.as_bytes()); // DW_AT_name
                buf.push(0);
                buf.push(size); // byte_size
                buf.push(enc); // encoding
            }
        }
        // 第二遍：pointer_type（&T / &mut T / *const T / *mut T——byte_size 8，
        // DW_AT_type 指向内层标量的 base_type；内层非标量（聚合/嵌套指针）
        // = 0 占位，后续扩展）
        for desc in &descs {
            if let Some(TyKind::Ptr { pointee }) = ty_kind(desc) {
                type_off_by_desc.insert(desc.clone(), buf.len() as u32);
                buf.push(7); // abbrev code 7: DW_TAG_pointer_type
                buf.push(8); // DW_AT_byte_size (data1)
                let tpos = buf.len();
                buf.extend_from_slice(&0u32.to_le_bytes()); // DW_AT_type 占位
                if let Some(pn) = pointee
                    && let Some(&po) = type_off_by_desc.get(&pn)
                {
                    buf[tpos..tpos + 4].copy_from_slice(&po.to_le_bytes());
                }
            }
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
            // DW_AT_frame_base → exprloc：DW_OP_reg6（x86 rbp）。属性名 0x40
            // 只存在于 abbrev code 3——DIE 值流不写属性名，否则读者把 0x40
            // 当 exprloc 长度 64，吞掉后续全部字节（变量 DIE 错位）。
            buf.push(1); // exprloc length
            buf.push(0x56); // DW_OP_reg6（0x50 + reg6）
            for v in fn_vars {
                // formal_parameter (abbrev 4) / variable (abbrev 5)
                buf.push(if v.is_arg { 4 } else { 5 });
                // DW_AT_name (0x03) → string
                buf.extend_from_slice(v.name.as_bytes());
                buf.push(0);
                // DW_AT_type (0x49) → ref4（类型 DIE 段内偏移；无匹配 = 0）
                let tpos = buf.len();
                buf.extend_from_slice(&0u32.to_le_bytes());
                if let Some(&off4) = type_off_by_desc.get(&v.ty_desc) {
                    buf[tpos..tpos + 4].copy_from_slice(&off4.to_le_bytes());
                }
                // DW_AT_location（属性名在 abbrev code 4/5 中）→ exprloc：
                // DW_OP_fbreg <sleb128 offset>——同样不写 0x02 属性名。
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

/// 变量类型的 DWARF 表示分类（用于类型 DIE 区生成）：
/// - `Scalar` 由 `scalar_base_type` 命中（base_type DIE）
/// - `Ptr { pointee }`：&T / &mut T / *const T / *mut T → pointer_type
///   （byte_size 8），pointee = 内层标量的 base 名（其 base_type DIE 已
///   生成，pointer 的 DW_AT_type 引用）；内层非标量（聚合/嵌套指针）=
///   None（pointer 无 DW_AT_type，后续扩展）
/// - 其余（聚合/元组/切片等）= None（C2 首版无 type）
#[derive(Clone, Debug)]
enum TyKind {
    Scalar,
    Ptr { pointee: Option<String> },
}

fn ty_kind(desc: &str) -> Option<TyKind> {
    let d = desc.trim();
    if scalar_base_type(d).is_some() {
        return Some(TyKind::Scalar);
    }
    // 指针/引用前缀（&mut 需在 & 前匹配；生命周期 erased 无 "&'a" 形态）
    let pointee_of = |inner: &str| {
        // 内层标量名（其 base_type 区已生成——pointee 引用）
        scalar_base_type(inner.trim()).map(|(n, _, _)| n)
    };
    for pre in ["*const ", "*mut ", "&mut ", "&"] {
        if let Some(inner) = d.strip_prefix(pre) {
            return Some(TyKind::Ptr {
                pointee: pointee_of(inner),
            });
        }
    }
    None
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
    buf.push(0x11); buf.push(0x01); // low_pc → addr（CU 代码范围起点）
    buf.push(0x12); buf.push(0x07); // high_pc → data8（相对 low_pc 偏移）
    buf.push(0x10); buf.push(0x06); // stmt_list → data4（.debug_line 偏移 0）
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
    buf.push(0x40); buf.push(0x18); // frame_base → exprloc（DW_FORM_exprloc=0x18）
    buf.push(0); buf.push(0);
    // code 4：formal_parameter，children no
    buf.push(4);
    buf.push(0x05);
    buf.push(0);
    buf.push(0x03); buf.push(0x08); // name → string
    buf.push(0x49); buf.push(0x06); // type → ref4
    buf.push(0x02); buf.push(0x18); // location → exprloc（DW_FORM_exprloc=0x18）
    buf.push(0x3b); buf.push(0x06); // decl_line → data4
    buf.push(0); buf.push(0);
    // code 5：variable，children no
    buf.push(5);
    buf.push(0x34);
    buf.push(0);
    buf.push(0x03); buf.push(0x08); // name → string
    buf.push(0x49); buf.push(0x06); // type → ref4
    buf.push(0x02); buf.push(0x18); // location → exprloc（DW_FORM_exprloc=0x18）
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
    // code 7：pointer_type，children no（byte_size 8；type ref4 指向内层
    // base_type——聚合/引用内层为 0 占位，后续扩展）
    buf.push(7);
    buf.push(0x0f);
    buf.push(0);
    buf.push(0x0b); buf.push(0x0b); // byte_size → data1
    buf.push(0x49); buf.push(0x06); // type → ref4
    buf.push(0); buf.push(0);
    buf.push(0); // 整个 abbrev 表终止
    buf
}

/// 生成 `.debug_aranges`：单 CU 一个地址范围（low_pc 占位 + reloc 到首
/// 函数符号，length = code_span）。gdb 16（cooked index）的 pc→CU 映射
/// 依赖 aranges——缺则命中 CU 外的 pc 时帧落回 COFF 符号（"__end__"）、
/// info args/locals 全空（实测，w64devkit gdb 16.2）。
/// 返回 (字节, relocs)。
pub fn gen_debug_aranges(
    first_sym: Option<&str>,
    code_span: u64,
) -> (Vec<u8>, Vec<(usize, String)>) {
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(&0u32.to_le_bytes()); // unit_length 占位
    buf.extend_from_slice(&2u16.to_le_bytes()); // version = 2
    buf.extend_from_slice(&0u32.to_le_bytes()); // debug_info_offset = 0（单 CU）
    buf.push(8); // address_size
    buf.push(0); // segment_size
    while buf.len() % 16 != 0 {
        buf.push(0); // 头补齐到 tuple 对齐（2×addr_size）
    }
    let mut relocs: Vec<(usize, String)> = Vec::new();
    let apos = buf.len();
    buf.extend_from_slice(&0u64.to_le_bytes()); // address 占位 → 首函数
    if let Some(sym) = first_sym {
        relocs.push((apos, sym.to_string()));
    }
    buf.extend_from_slice(&code_span.to_le_bytes()); // length
    buf.extend_from_slice(&0u64.to_le_bytes()); // (0,0) 终止 tuple
    buf.extend_from_slice(&0u64.to_le_bytes());
    let ul = buf.len() - 4;
    buf[..4].copy_from_slice(&(ul as u32).to_le_bytes());
    (buf, relocs)
}

/// 生成全部 DWARF 段。
/// `src_file`：源文件路径（CU 名 + .debug_line file 条目——gdb `list`/
/// 源码断点需要真实文件名，crate 名匹配不到磁盘文件）。
/// `code_span`：CU 代码总字节（high_pc 偏移；+0x200 边距盖过函数间
/// 对齐填充与 main 别名副本——单 CU 超范围无害）。
/// 返回 (段名, 字节, 段内 reloc: (偏移, 符号名)) 列表。
pub fn build_dwarf_sections(
    fns: &[(String, u32, Vec<(u32, u32)>)],
    vars: &[FnVarEntries],
    producer: &str,
    src_file: &str,
    code_span: u64,
    full: bool,
) -> Vec<(String, Vec<u8>, Vec<(usize, String)>)> {
    let entries: Vec<(String, u32)> = fns.iter().map(|(s, l, _)| (s.clone(), *l)).collect();
    let span = code_span + 0x200;
    let first_sym = entries.first().map(|(s, _)| s.as_str());
    let (line_bytes, line_relocs) = gen_debug_line(fns, src_file);
    let (info_bytes, info_relocs) = gen_debug_info(&entries, vars, producer, src_file, span, full);
    let (ar_bytes, ar_relocs) = gen_debug_aranges(first_sym, span);
    vec![
        (".debug_line".to_string(), line_bytes, line_relocs),
        (".debug_info".to_string(), info_bytes, info_relocs),
        (".debug_aranges".to_string(), ar_bytes, ar_relocs),
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
        let fns = vec![
            ("main".to_string(), 10u32, vec![]),
            ("helper".to_string(), 20u32, vec![(4u32, 21u32), (12u32, 22u32)]),
        ];
        let (bytes, relocs) = gen_debug_line(&fns, "test_crate");
        assert!(bytes.len() > 24, "line program too small");
        // 函数级 2 + helper 的 2 个 per-statement = 4 个 reloc
        assert_eq!(relocs.len(), 4, "reloc per line entry");
        assert_eq!(relocs[0].1, "main");
        // file_names[0] = 库名（非空）
        assert!(
            bytes.windows(10).any(|w| w == b"test_crate"),
            "file_names[0] should contain crate name"
        );
        // unit_length 非 0
        let unit_len = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        assert_eq!(unit_len as usize, bytes.len() - 4);
    }

    #[test]
    fn per_statement_offsets_in_placeholders() {
        // B1：per-statement 行号的 set_address 占位应写指令偏移（COFF
        // addend 隐式）——helper 的第 2 个条目占位 = 12。
        let fns = vec![("helper".to_string(), 20u32, vec![(12u32, 22u32)])];
        let (bytes, relocs) = gen_debug_line(&fns, "c");
        assert_eq!(relocs.len(), 2);
        // 第 2 个 reloc 的占位（relocs[1].0 处 8 字节）= 12
        let off = relocs[1].0;
        let v = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        assert_eq!(v, 12, "per-statement set_address placeholder = instr offset");
    }

    #[test]
    fn info_has_cu_and_subprograms() {
        let entries = vec![("f".to_string(), 5u32)];
        let (bytes, relocs) = gen_debug_info(&entries, &[], "forge", "test", 0x30, false);
        // relocs：CU low_pc(1) + subprogram low_pc(1)
        assert_eq!(relocs.len(), 2);
        assert_eq!(relocs[0].1, "f", "CU low_pc reloc → first fn");
        // header：unit_length(4) + version(2) + abbr_off(4) + addr_size(1) = 11
        // CU DIE abbrev code = 1 在 index 11（version 是 u16——只写 1 字节会
        // 错位，见 WA-31）
        assert_eq!(bytes[11], 1);
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
        let (bytes, relocs) = gen_debug_info(&entries, &vars, "forge", "test", 0x30, true);
        assert_eq!(relocs.len(), 2, "CU low_pc + fn low_pc relocs");
        // base_type "i32"（code 6 在 CU 后）出现——字节里含 "i32\0"
        assert!(
            bytes.windows(5).any(|w| w == b"i32\0\x04"),
            "base_type i32 byte_size=4 present"
        );
        // 变量名 "x" / "a" 出现（subprogram children——name 后跟 \0）
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("x\u{0}"), "variable name x present");
        assert!(s.contains("a\u{0}"), "param name a present");
        // pointer_type（&i32 的 b——code 7 + byte_size 8）出现
        assert!(
            bytes.windows(2).any(|w| w == [7, 8]),
            "pointer_type (abbrev 7, byte_size 8) present for &i32"
        );
        // fbreg 表达式（0x91 = DW_OP_fbreg）出现
        assert!(bytes.windows(1).any(|w| w[0] == 0x91), "DW_OP_fbreg present");
    }

    #[test]
    fn full_info_structurally_parses() {
        // 结构解码器：按 gen_debug_abbrev 的 code/attr/form 布局从头解析
        // .debug_info，验证所有 DIE 边界对齐且恰好消费完整段。回归：曾把
        // 属性名 0x40（frame_base）/0x02（location）误写进 DIE 值流——读者
        // 把 0x40 当 exprloc 长度 64，吞掉后续全部 DIE（Abbrev 83 不存在）。
        let entries = vec![("f".to_string(), 5u32)];
        let vars = vec![FnVarEntries {
            sym: "f".to_string(),
            vars: vec![
                VarEntry { name: "x".to_string(), slot_offset: -24, ty_desc: "i32".to_string(), is_arg: false, decl_line: 7 },
                VarEntry { name: "a".to_string(), slot_offset: -32, ty_desc: "i32".to_string(), is_arg: true, decl_line: 6 },
                VarEntry { name: "b".to_string(), slot_offset: -16, ty_desc: "&i32".to_string(), is_arg: true, decl_line: 6 },
            ],
        }];
        let (bytes, relocs) = gen_debug_info(&entries, &vars, "forge", "test", 0x30, true);
        assert_eq!(relocs.len(), 2, "CU low_pc + fn low_pc relocs");

        let mut p = 0usize;
        let rd_u32 = |p: &mut usize| {
            let v = u32::from_le_bytes(bytes[*p..*p + 4].try_into().unwrap());
            *p += 4;
            v
        };
        let rd_str = |p: &mut usize| {
            let s0 = *p;
            while bytes[*p] != 0 {
                *p += 1;
            }
            let s = String::from_utf8_lossy(&bytes[s0..*p]).into_owned();
            *p += 1; // 跳过 NUL
            s
        };
        let rd_uleb = |p: &mut usize| {
            let mut v = 0u64;
            let mut sh = 0;
            loop {
                let b = bytes[*p];
                *p += 1;
                v |= ((b & 0x7f) as u64) << sh;
                if b & 0x80 == 0 {
                    break;
                }
                sh += 7;
            }
            v
        };

        let _unit_len = rd_u32(&mut p);
        p += 2; // version
        p += 4; // debug_abbrev_offset
        p += 1; // address_size
        // CU DIE（code 1）：producer str / name str / language data2 /
        // low_pc addr8（reloc 占位）/ high_pc data8 / stmt_list data4
        assert_eq!(bytes[p], 1, "CU abbrev code");
        p += 1;
        rd_str(&mut p);
        rd_str(&mut p);
        p += 2;
        p += 8; // CU low_pc 占位（reloc 指向 f）
        let hi = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
        p += 8;
        assert_eq!(hi, 0x30, "high_pc = code_span");
        rd_u32(&mut p); // stmt_list data4 = 0
        // CU 子项：base_type(6) / pointer_type(7) / subprogram(2|3)，0 终止
        let mut found_base = false;
        let mut found_ptr = false;
        let mut found_sub = false;
        let mut fbreg_offsets: Vec<i64> = Vec::new();
        loop {
            let code = bytes[p];
            if code == 0 {
                p += 1;
                break;
            }
            p += 1;
            match code {
                6 => {
                    // base_type：name str / byte_size d1 / encoding d1
                    rd_str(&mut p);
                    p += 2;
                    found_base = true;
                }
                7 => {
                    // pointer_type：byte_size d1 / type ref4
                    p += 1;
                    rd_u32(&mut p);
                    found_ptr = true;
                }
                2 => {
                    // subprogram：name str / low_pc addr8 / decl_line data4
                    rd_str(&mut p);
                    p += 8;
                    rd_u32(&mut p);
                    found_sub = true;
                }
                3 => {
                    // subprogram（children yes）：name / low_pc / decl_line /
                    // frame_base exprloc + children（4/5）至 0
                    rd_str(&mut p);
                    p += 8;
                    rd_u32(&mut p);
                    let fb_len = rd_uleb(&mut p) as usize;
                    assert_eq!(&bytes[p..p + fb_len], &[0x56], "frame_base = DW_OP_reg6");
                    p += fb_len;
                    loop {
                        let c = bytes[p];
                        if c == 0 {
                            p += 1;
                            break;
                        }
                        p += 1;
                        assert!(c == 4 || c == 5, "child abbrev 4/5, got {c}");
                        rd_str(&mut p);
                        rd_u32(&mut p); // type ref4
                        let loc_len = rd_uleb(&mut p) as usize;
                        assert_eq!(bytes[p], 0x91, "location starts DW_OP_fbreg");
                        fbreg_offsets.push(decode_sleb(&bytes[p + 1..p + loc_len]));
                        p += loc_len;
                        rd_u32(&mut p); // decl_line data4
                    }
                    found_sub = true;
                }
                other => panic!(
                    "unexpected abbrev code {other} at p={p}; bytes: {}",
                    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
                ),
            }
        }
        assert_eq!(p, bytes.len(), "DIE stream must parse to exact end");
        assert!(found_base && found_ptr && found_sub, "all DIE kinds present");
        // fbreg 偏移（sleb128，x = slot_offset）：x=-32 / b=-16 / a=-24
        let mut offs = fbreg_offsets;
        offs.sort_unstable();
        assert_eq!(offs, vec![-32, -24, -16], "fbreg offsets from slot offsets");
    }

    fn decode_sleb(mut v: &[u8]) -> i64 {
        let mut out = 0i64;
        let mut sh = 0u32;
        loop {
            let b = v[0];
            v = &v[1..];
            out |= ((b & 0x7f) as i64) << sh;
            sh += 7;
            if b & 0x80 == 0 {
                if b & 0x40 != 0 {
                    out |= -1i64 << sh;
                }
                break;
            }
        }
        out
    }

    #[test]
    fn line_program_structurally_decodes() {
        // 按 DWARF4 行程序语义解码 gen_debug_line：头部字段（version u16、
        // max_ops、line_base=-5、line_range=14、opcode_base=13 +
        // standard_opcode_lengths、文件条目 dir/mtime/length + 终止符）与
        // 每条目 sequence（ext set_address + set_file 1 + advance_line(L-1)
        // + copy + end_sequence）。回归：version 单字节、opcode_base=1（标准
        // 操作码被当 special）、set_file 0、缺文件字段等错误在此暴露。
        let fns = vec![
            ("main".to_string(), 10u32, vec![(4u32, 11u32), (12u32, 12u32)]),
            ("helper".to_string(), 20u32, vec![]),
        ];
        let (bytes, relocs) = gen_debug_line(&fns, "test_crate");
        // 头部 reloc 数 = 4 条目（main + 2 stmt + helper）
        assert_eq!(relocs.len(), 4);

        let mut p = 0usize;
        let rd_u32 = |p: &mut usize| {
            let v = u32::from_le_bytes(bytes[*p..*p + 4].try_into().unwrap());
            *p += 4;
            v
        };
        let mut rd_uleb = |p: &mut usize| {
            let mut v = 0u64;
            let mut sh = 0;
            loop {
                let b = bytes[*p];
                *p += 1;
                v |= ((b & 0x7f) as u64) << sh;
                if b & 0x80 == 0 {
                    break;
                }
                sh += 7;
            }
            v
        };
        let unit_len = rd_u32(&mut p);
        assert_eq!(unit_len as usize, bytes.len() - 4, "unit_length covers section");
        assert_eq!(bytes[p], 4, "line version lo");
        assert_eq!(bytes[p + 1], 0, "line version hi");
        p += 2;
        let hl = rd_u32(&mut p);
        let header_end = p + hl as usize;
        assert_eq!(bytes[p], 1, "min_inst_length");
        p += 1;
        assert_eq!(bytes[p], 1, "max_ops_per_inst (v4)");
        p += 1;
        assert_eq!(bytes[p], 1, "default_is_stmt");
        p += 1;
        assert_eq!(bytes[p] as i8, -5, "line_base = -5");
        p += 1;
        assert_eq!(bytes[p], 14, "line_range");
        p += 1;
        let opcode_base = bytes[p];
        p += 1;
        assert_eq!(opcode_base, 13, "opcode_base covers std opcodes 1..12");
        p += (opcode_base - 1) as usize; // standard_opcode_lengths
        // 目录表：空 = 1 个 NUL
        assert_eq!(bytes[p], 0);
        p += 1;
        // 文件表：name\0 + dir + mtime + size + 终止符
        while bytes[p] != 0 {
            p += 1;
        }
        p += 1;
        assert_eq!(&bytes[p..p + 4], &[0, 0, 0, 0], "dir/mtime/size + terminator");
        p += 4;
        assert_eq!(p, header_end, "header ends exactly at header_length");

        // 行程序：每条目一个 sequence
        let mut expected: Vec<(u32, i64)> = Vec::new();
        for (sym, line, stmts) in &fns {
            expected.push((0, *line as i64));
            for (off, ln) in stmts {
                expected.push((*off, *ln as i64));
            }
            let _ = sym;
        }
        let mut lines_found: Vec<i64> = Vec::new();
        let mut addrs_found: Vec<u64> = Vec::new();
        for _ in 0..expected.len() {
            assert_eq!(bytes[p], 0, "extended opcode");
            p += 1;
            assert_eq!(bytes[p], 9, "set_address operand len");
            p += 1;
            assert_eq!(bytes[p], 2, "DW_LNE_set_address");
            p += 1;
            addrs_found.push(u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap()));
            p += 8;
            assert_eq!(bytes[p], 4, "DW_LNS_set_file");
            p += 1;
            assert_eq!(bytes[p], 1, "file index 1");
            p += 1;
            assert_eq!(bytes[p], 3, "DW_LNS_advance_line");
            p += 1;
            let s0 = p;
            while bytes[p] & 0x80 != 0 {
                p += 1;
            }
            p += 1; // sleb 末字节
            lines_found.push(decode_sleb(&bytes[s0..p]));
            assert_eq!(bytes[p], 1, "DW_LNS_copy");
            p += 1;
            assert_eq!(bytes[p], 0, "extended");
            p += 1;
            assert_eq!(bytes[p], 1, "end_sequence len");
            p += 1;
            assert_eq!(bytes[p], 1, "DW_LNE_end_sequence");
            p += 1;
        }
        assert_eq!(p, bytes.len(), "program consumed to exact end");
        // 行寄存器每 sequence 起点 = 1 → 行 = 1 + advance_line 操作数 = L
        let row_lines: Vec<i64> = lines_found.iter().map(|op| 1 + op).collect();
        let want_lines: Vec<i64> = expected.iter().map(|(_, l)| *l).collect();
        assert_eq!(row_lines, want_lines, "每 sequence 行 = 目标行");
        let want_addrs: Vec<u64> = expected.iter().map(|(a, _)| *a as u64).collect();
        assert_eq!(addrs_found, want_addrs, "占位 = 指令偏移");
    }

    #[test]
    fn aranges_structurally_parses() {
        // 单 CU 范围：(header, pad 到 16, addr 占位 + reloc, length,
        // (0,0) 终止 tuple)。gdb 16 cooked index 依赖它做 pc→CU 映射。
        let (bytes, relocs) = gen_debug_aranges(Some("f"), 0x30);
        assert_eq!(relocs.len(), 1);
        assert_eq!(relocs[0].1, "f");
        // 头部：unit_length / version 2 / debug_info_offset 0 / addr 8 / seg 0
        let ul = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        assert_eq!(ul, bytes.len() - 4, "unit_length covers section");
        assert_eq!(&bytes[4..6], &[2, 0], "aranges version 2");
        assert_eq!(&bytes[6..10], &[0, 0, 0, 0], "debug_info_offset 0");
        assert_eq!(bytes[10], 8, "address_size");
        assert_eq!(bytes[11], 0, "segment_size");
        // tuple 起点必须 16 对齐（2×address_size）
        let tup = ((4 + 2 + 4 + 1 + 1) + 3) & !3;
        let _ = tup;
        let mut p = 12usize;
        while p % 16 != 0 {
            assert_eq!(bytes[p], 0, "pad byte");
            p += 1;
        }
        assert_eq!(p, relocs[0].0, "addr 占位位置");
        let addr = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
        assert_eq!(addr, 0, "addr 占位 0（reloc 补）");
        p += 8;
        let len = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
        assert_eq!(len, 0x30, "length = code_span");
        p += 8;
        // 终止 tuple (0,0)
        assert_eq!(&bytes[p..p + 8], &[0u8; 8], "terminator addr 0");
        p += 8;
        assert_eq!(&bytes[p..p + 8], &[0u8; 8], "terminator len 0");
        assert_eq!(p + 8, bytes.len(), "exact end");
    }
}
