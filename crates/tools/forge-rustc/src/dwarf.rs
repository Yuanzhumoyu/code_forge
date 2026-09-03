//! DWARF v5 生成（C1 line-tables-only + C2 full 变量级）。
//!
//! **版本必须是 v5**：w64devkit gdb 16.2 对 PE 只支持 DWARF5——gcc 同源
//! 代码 -gdwarf-4 编译后 gdb 同样全失效（对照实证），默认 v5 正常。
//!
//! 段：
//! - `.debug_line`：行号程序（v5 头：version u16/address_size/
//!   segment_selector/opcode_base=13 + standard_opcode_lengths；目录表
//!   format_count=0+count=0；文件表 [DW_LNCT_path→DW_FORM_line_strp]，
//!   **两条同串条目**（gcc hack：file 号 1 在 0 基/1 基读者下都命中）；
//!   line_strp 偏移 4 字节。每函数**一个 sequence**（gcc/clang 布局——
//!   每行一个 sequence 会让行零跨度，gdb 丢弃零长度行）；函数级 +
//!   per-statement 行（B1，逐行 set_address reloc + advance_line 累加；
//!   末行 copy 后 advance_pc 到函数真实末地址——终端行不再零跨度）
//! - `.debug_line_str`：路径字符串池（file 经 line_strp 偏移 0 引用）
//! - `.debug_info`：单 CU（v5 头：unit_type/address_size；name=源文件
//!   路径、low_pc/high_pc/stmt_list） + 每函数 `DW_TAG_subprogram`
//!   （name/low_pc/high_pc=函数代码字节/decl_line；C2：frame_base +
//!   变量/参数 DIE + base_type/pointer_type）
//! - `.debug_aranges`：单 CU 地址范围（gdb 16 cooked index 的 pc→CU 映射）
//! - `.debug_abbrev`：缩写表（字符串属性用 DW_FORM_string 内联，无 strp）
//! - `.debug_frame`（M2）：x86_64 CFI——CIE + 每函数 FDE（forge-codegen
//!   对 prologue 扫描所得 `FunctionCfi` 行；initial_location 占位 + reloc
//!   到函数符号）。gdb 16.2 amd64-windows 无 SEH(.pdata) 时落 dwarf2-frame
//!   解 .debug_frame 建栈帧——无 CFI 则 bt/info args 全空（实证）
//!
//! 地址属性（low_pc/set_address/aranges）用相对占位 + reloc，由
//! codegen_crate 在对象写入时按函数符号解析（ADDR64 重定位）。
//! 结构错误历史（WA-31）：version u16、DIE 值流误写属性名、行程序
//! 头不合法（opcode_base/advance_line 漏发/set_file 0）、文件条目缺字段、
//! CU 范围/aranges/stmt_list 缺失、subprogram 缺 high_pc（gdb 建不了
//! function block → 变量 DIE 全丢）、每行一 sequence（零跨度）。

use code_forge::backend::pipeline::cfi::{CfiOp, FunctionCfi};

/// DWARF 行号程序操作码（DWARF v5）。
const DW_LNS_COPY: u8 = 0x01;
const DW_LNS_ADVANCE_PC: u8 = 0x02;
const DW_LNS_ADVANCE_LINE: u8 = 0x03;
const DW_LNS_SET_FILE: u8 = 0x04;
const DW_LNE_END_SEQUENCE: u8 = 0x01;
const DW_LNE_SET_ADDRESS: u8 = 0x02;

/// DW_CFA 操作码（DWARF5 §6.4.2；字节形态对照 mingw gcc -gdwarf-5 的
/// .debug_frame 实证：version=1、aug=""、code_align=1、data_align=-8(0x78)、
/// RA 列 uleb、`0c 07 08`=def_cfa rsp 8、`0x80|reg + uleb`=offset 保存槽）。
const DW_CFA_DEF_CFA: u8 = 0x0c;
const DW_CFA_DEF_CFA_REGISTER: u8 = 0x0d;
const DW_CFA_DEF_CFA_OFFSET: u8 = 0x0e;
const DW_CFA_ADVANCE_LOC: u8 = 0x40; // 主操作码：0x40|delta（delta < 64）
const DW_CFA_OFFSET: u8 = 0x80; // 主操作码：0x80|reg（reg < 64）+ uleb 操作数
const DW_CFA_ADVANCE_LOC1: u8 = 0x02; // delta 为 uleb（≥64 时的长形式）
const DW_CFA_NOP: u8 = 0x00;

/// C2 debuginfo（-C debuginfo=2 full）：单个源变量的 DIE 输入
///（lower 从 rustc `body.var_debug_info` 采集）。
#[derive(Clone, Debug)]
pub struct VarEntry {
    /// 变量名（rustc var_debug_info 的 Symbol）。
    pub name: String,
    /// 栈槽实际偏移（相对 rbp——含主库 callee-saved shift，直接作
    /// DW_OP_fbreg 操作数）。
    pub slot_offset: i32,
    /// 类型（rustc Debug 形态，如 "i32"/"&i32"/"varprobe::Point"，
    /// 供 base_type/pointer_type/structure_type 名匹配）。
    pub ty_desc: String,
    /// 是否函数参数（决定 DW_TAG_formal_parameter vs DW_TAG_variable）。
    pub is_arg: bool,
    /// 声明源码行（1-based，DW_AT_decl_line）。
    pub decl_line: u32,
    /// 聚合（struct）类型成员：非空 = 该变量类型是有名成员 ADT，dwarf
    /// 生成 DW_TAG_structure_type（code 8）+ DW_TAG_member 子项（code 9），
    /// 成员 byte_off 直接作 data_member_location（与槽内布局一致——
    /// forge 全栈槽模型，聚合整体存槽内，字段 = 槽 + 字段偏移）。
    /// 标量/指针变量 = 空。V1：仅非 enum/union 的命名 struct。
    pub members: Vec<VarMember>,
    /// 类型字节大小（structure_type 的 DW_AT_byte_size；标量/指针 = 实际
    /// 宽度，当前未用于 base_type——byte_size 来自 scalar_base_type）。
    pub size: u32,
}

/// 聚合类型成员（structure_type 的 DW_TAG_member 输入）。
#[derive(Clone, Debug)]
pub struct VarMember {
    /// 字段名。
    pub name: String,
    /// 字段类型（rustc Debug 形态，标量/指针可解析；嵌套聚合 = 0 占位）。
    pub ty_desc: String,
    /// 字段相对结构体起点的字节偏移（layout fields 实测——含 repr 重排）。
    pub byte_off: u32,
}

/// C2 debuginfo：单函数的源变量表（按函数符号聚合）。
#[derive(Clone, Debug)]
pub struct FnVarEntries {
    /// 函数符号（mangled，与 add_function 的对象符号一致）。
    pub sym: String,
    /// 该函数的源变量列表。
    pub vars: Vec<VarEntry>,
}

/// C-like 枚举类型（全部 unit 变体，无 payload）：DW_TAG_enumeration_type +
///  DW_TAG_enumerator 子项（name + const_value）。lower 采集判别值
///（rustc `adt.discriminants`），注册到 FuncRefTable 供 dwarf 生成。
#[derive(Clone, Debug)]
pub struct EnumTypeEntry {
    /// 类型（rustc Debug 形态，如 "varprobe::Color"）。
    pub desc: String,
    /// 字节大小（layout）。
    pub size: u32,
    /// (变体名, 判别值)。
    pub variants: Vec<(String, u64)>,
}

/// 生成 `.debug_line` 段（单 CU、单文件、多函数条目）。
///
/// 每函数条目：`DW_LNE_set_address <addr>` + `DW_LNS_set_file 1` +
/// `DW_LNS_advance_line` + `DW_LNS_copy` + `DW_LNE_end_sequence`。
/// addr 是相对地址占位（reloc 补——COFF addend 隐式：占位字节即偏移，
/// 链接后 = 符号地址 + 占位值）。file[1] = 源文件（路径存 .debug_line_str，
/// 条目经 DW_FORM_line_strp 引用，偏移 0）。
/// 每个函数：(符号, 函数声明行, per-statement (指令偏移, 行) 列表——
/// B1 行号细化：每语句一个 set_address(偏移) + 行条目，调试器可精确
/// 行断点/单步；空列表 = 仅函数级条目（C1 兼容）。
///
/// **DWARF v5 头**（gdb 16.2 对 PE 只支持 v5——gcc -gdwarf-4 对照实证）：
/// unit_length | version=5 u16 | address_size | segment_selector_size |
/// header_length | min_inst/max_ops/is_stmt | line_base=-5(0xfb) |
/// line_range=14 | opcode_base=13 + 12 个 standard_opcode_lengths。
/// 目录表 format_count=0（空）；文件表 format_count=1：[DW_LNCT_path=1 →
/// DW_FORM_line_strp=0x1f]，count=1，entry = line_strp 偏移 0。
/// opcode_base 若 < 5，copy/advance_line/set_file（opcode 1/3/4）会被当
/// special opcode 解码（行/地址错乱）；每条目自成一个 sequence——
/// end_sequence 后行寄存器复位为 1，故每行 advance_line 目标 = L - 1。
///
/// 返回 (段字节, reloc: (数据内偏移, 符号名))。
/// `fn_sizes`：符号 → 函数代码字节——末行 copy 后 `DW_LNS_advance_pc` 推进
/// 到函数真实末地址（gcc decodedline 末行 end = fn end 对照），否则末行
/// 区间 [X, X) 零跨度被 gdb 丢弃（终端行 line 0 根因）。缺失符号（未知
/// 大小）跳过 advance（保持旧行为，安全）。
pub fn gen_debug_line(
    fns: &[(String, u32, Vec<(u32, u32)>)],
    fn_sizes: &std::collections::HashMap<String, u64>,
) -> (Vec<u8>, Vec<(usize, String)>) {
    let mut buf: Vec<u8> = Vec::new();
    let unit_len_pos = 0usize;
    buf.extend_from_slice(&0u32.to_le_bytes()); // unit_length 占位
    buf.push(5); // version（DWARF v5）lo
    buf.push(0); // version hi（u16——只写 1 字节会错位）
    buf.push(8); // address_size
    buf.push(0); // segment_selector_size
    let hl_pos = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes()); // header_length 占位
    buf.push(1); // minimum_instruction_length
    buf.push(1); // maximum_operations_per_instruction
    buf.push(1); // default_is_stmt
    buf.push(0xfb); // line_base = -5（有符号字节读）
    buf.push(14); // line_range
    buf.push(13); // opcode_base——standard opcodes 1..12
    // standard_opcode_lengths（opcode 1..12）：
    // copy0 advance_pc1 advance_line1 set_file1 set_column1 negate0
    // basic_block0 const_add_pc0 fixed_advance_pc1 prologue0 epilogue0 set_isa1
    buf.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
    // 目录表：format_count = 0 + dir_count = 0（v5 表 count 前缀——只写
    // format_count=0 会让读者把下一字段当目录 count 读：dir_count=0 空表
    // 才自洽；objdump "format count is zero but table is not empty"）
    buf.push(0);
    buf.push(0);
    // 文件表：format_count = 1，[DW_LNCT_path → DW_FORM_line_strp]。
    // **两个相同条目**（gcc 同款 hack）：file 号 1 在 0 基读者（binutils
    // v5）与 1 基读者（寄存器初值 1）下都解析到同一文件——单条目 +
    // set_file 1 会被 binutils 判 "index 1 >= count 1" 拒。
    buf.push(1);
    buf.push(1); // DW_LNCT_path = 1
    buf.push(0x1f); // DW_FORM_line_strp = 0x1f
    buf.push(2); // file_names_count = 2
    // line_strp 偏移是 **4 字节**（非 uleb——gcc 字节实证 56 00 00 00）；
    // 两条目同指 .debug_line_str 偏移 0（gcc 同款 1 基/0 基 hack）
    buf.extend_from_slice(&0u32.to_le_bytes()); // file[0] strp 偏移 0
    buf.extend_from_slice(&0u32.to_le_bytes()); // file[1] strp 偏移 0
    let header_end = buf.len();
    let hl = header_end - (hl_pos + 4);
    buf[hl_pos..hl_pos + 4].copy_from_slice(&(hl as u32).to_le_bytes());

    let mut relocs: Vec<(usize, String)> = Vec::new();
    for (sym, line, stmts) in fns {
        // 每函数一个 sequence（gcc/clang 布局——**每行一个 sequence 会让
        // 每行零跨度，gdb 丢弃零长度行 → 行号全空**）：函数起点行 + 每
        // 语句行（set_address 逐行 reloc），最后 end_sequence。
        // 行寄存器 sequence 起点 = 1：首行 delta = L - 1；同 sequence 内
        // 后续行 delta = L - 上一行（寄存器持续累加，不复位）。
        let mut prev_line = 1i64;
        let mut first = true;
        for (addr, ln) in std::iter::once((0i64, *line as i64))
            .chain(stmts.iter().map(|(o, l)| (*o as i64, *l as i64)))
        {
            if !first {
                push_line_row(&mut buf, &mut relocs, sym, addr, ln - prev_line);
            } else {
                push_line_row(&mut buf, &mut relocs, sym, addr, ln - 1);
                first = false;
            }
            prev_line = ln;
        }
        // 终端行修复（M2）：末行 copy 后直接 end_sequence → 末行零跨度
        // [X, X)，gdb 丢弃零长度行 → 最后一行行号丢失（line 0）。对照
        // gcc：末行延伸到函数真实末地址 = fn 起点 + fn_sizes。发一条
        // advance_pc 把地址寄存器推到 fn_end（delta=0/大小未知则省略）。
        let fn_end = fn_sizes.get(sym).copied().unwrap_or(0);
        let last_addr = stmts.last().map(|(o, _)| *o as u64).unwrap_or(0);
        if fn_end > last_addr {
            buf.push(DW_LNS_ADVANCE_PC);
            encode_uleb128(&mut buf, fn_end - last_addr);
        }
        // end_sequence（sequence 终止；行寄存器复位 1）
        buf.push(0); // extended
        buf.push(1);
        buf.push(DW_LNE_END_SEQUENCE);
    }
    let unit_len = buf.len() - (unit_len_pos + 4);
    buf[unit_len_pos..unit_len_pos + 4].copy_from_slice(&(unit_len as u32).to_le_bytes());
    (buf, relocs)
}

/// 生成 `.debug_line_str`：file 路径字符串池（file[1] 经 line_strp 引用，
/// 偏移 0——当前单文件 CU，池内只有一个串）。
pub fn gen_debug_line_str(file_name: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(file_name.as_bytes());
    buf.push(0);
    buf
}

/// 追加一行（同 sequence 内）：set_address(addr 占位 + reloc) + set_file 1
/// + advance_line(delta) + copy。delta 由调用方按行寄存器当前值计算。
fn push_line_row(
    buf: &mut Vec<u8>,
    relocs: &mut Vec<(usize, String)>,
    sym: &str,
    addr: i64,
    delta: i64,
) {
    buf.push(0); // extended
    buf.push(1 + 8); // operand length
    buf.push(DW_LNE_SET_ADDRESS);
    let addr_pos = buf.len();
    // 占位写 addr（COFF addend 隐式——链接后 = 符号地址 + addr）
    buf.extend_from_slice(&(addr as u64).to_le_bytes());
    relocs.push((addr_pos, sym.to_string()));
    buf.push(DW_LNS_SET_FILE);
    buf.push(1); // file[1]（v5 0 基 / 1 基 hack：文件表两条同串，均命中）
    buf.push(DW_LNS_ADVANCE_LINE);
    encode_sleb128(buf, delta);
    buf.push(DW_LNS_COPY);
}

/// 生成 `.debug_info` 段：单 CU（DW_TAG_compile_unit）+ 每函数一个
/// `DW_TAG_subprogram`。字符串属性用 DW_FORM_string（内联，无 strp）。
/// CU DIE 带 low_pc/high_pc（代码范围，reloc 到首函数符号 + data8 总长）。
/// subprogram 带 DW_AT_high_pc（data8 = 函数代码字节，gdb 需函数结束地址
/// 建 function block——缺则 children 变量 DIE 被丢，对照 gcc 实证）。
/// `full=true`（-C debuginfo=2）时 subprogram 带 children：
/// - DW_TAG_formal_parameter / DW_TAG_variable（DW_AT_location =
///   DW_OP_fbreg <槽偏移>，槽相对 rbp——frame base = rbp）
/// - DW_TAG_base_type（标量类型：name/byte_size/encoding，B3）
///
/// 返回 (字节, relocs: (偏移, 符号名))。
#[allow(clippy::too_many_arguments)]
pub fn gen_debug_info(
    entries: &[(String, u32)],
    vars: &[FnVarEntries],
    producer: &str,
    cu_name: &str,
    code_span: u64,
    fn_sizes: &std::collections::HashMap<String, u64>,
    enums: &[EnumTypeEntry],
    full: bool,
) -> (Vec<u8>, Vec<(usize, String)>) {
    let mut buf: Vec<u8> = Vec::new();
    let unit_len_pos = 0usize;
    buf.extend_from_slice(&0u32.to_le_bytes()); // unit_length 占位
    // DWARF v5 头：unit_length | version(u16) | unit_type(1) | address_size(1)
    // | debug_abbrev_offset(4)。**必须 v5**：w64devkit gdb 16.2 对 PE 只
    // 支持 DWARF5——gcc 同源代码 -gdwarf-4 编译后 gdb 同样全失效（断点不
    // 解析/无行号），默认 v5 完全正常（对照实证）。
    buf.push(5); // version lo
    buf.push(0); // version hi（u16——只写 1 字节会让后续字段错位）
    buf.push(1); // unit_type = DW_UT_compile
    buf.push(8); // address_size
    buf.extend_from_slice(&0u32.to_le_bytes()); // debug_abbrev_offset = 0

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

    // 类型 DIE 放 **subprogram 之后**（gcc 形态：前向引用——类型紧跟首个
    // 使用者之后）。顺序：base_type 先（pointer 引用它们），pointer_type
    // 后。key = 变量 ty_desc（去重）；subprogram/变量 DIE 的 DW_AT_type
    // 先写占位，类型区生成后统一回填（ref4 = 段内偏移）。
    let mut type_off_by_desc: std::collections::HashMap<String, u32> = Default::default();
    let mut type_ref_patches: Vec<(usize, String)> = Vec::new(); // (占位位, desc)
    // descs：标量/指针类型（base/pointer DIE）；struct_descs：带成员信息的
    // 聚合变量类型（structure_type DIE）。成员的标量/指针类型也要入 descs
    //（否则结构体字段引用解析不到 DIE）。
    let mut descs: Vec<String> = Vec::new();
    let mut struct_descs: Vec<String> = Vec::new();
    if full {
        for fv in vars {
            for v in &fv.vars {
                if ty_kind(&v.ty_desc).is_some() && !descs.contains(&v.ty_desc) {
                    descs.push(v.ty_desc.clone());
                }
                if !v.members.is_empty() && !struct_descs.contains(&v.ty_desc) {
                    struct_descs.push(v.ty_desc.clone());
                }
                for m in &v.members {
                    if ty_kind(&m.ty_desc).is_some() && !descs.contains(&m.ty_desc) {
                        descs.push(m.ty_desc.clone());
                    }
                }
            }
        }
    }

    // 每函数 subprogram：abbrev code 2（无变量，children no）或
    // code 3（有变量，children yes——variable/formal_parameter 作 children）
    for (sym, line) in entries {
        let fn_vars: Vec<&VarEntry> = if full {
            vars.iter()
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
        // DW_AT_decl_file (0x3a) → data1 = 1（文件表 file[1]）
        buf.push(1);
        // DW_AT_low_pc (0x11) → addr（占位 + reloc）
        let lp = buf.len();
        buf.extend_from_slice(&0u64.to_le_bytes());
        relocs.push((lp, sym.clone()));
        // DW_AT_high_pc (0x12) → data8：相对 low_pc 的偏移 = 函数代码
        // 字节（gdb 建 function block 需要结束地址——缺则变量全丢）
        let fn_size = fn_sizes.get(sym).copied().unwrap_or(0);
        buf.extend_from_slice(&fn_size.to_le_bytes());
        // DW_AT_decl_line (0x3b) → data4
        buf.extend_from_slice(&line.to_le_bytes());
        if !fn_vars.is_empty() {
            // DW_AT_frame_base → exprloc：DW_OP_reg6（x86 rbp）。属性名
            // 只存在于 abbrev——DIE 值流不写属性名。
            buf.push(1); // exprloc length
            buf.push(0x56); // DW_OP_reg6（0x50 + reg6）
            for v in fn_vars {
                // formal_parameter (abbrev 4) / variable (abbrev 5)
                buf.push(if v.is_arg { 4 } else { 5 });
                // DW_AT_name (0x03) → string
                buf.extend_from_slice(v.name.as_bytes());
                buf.push(0);
                // DW_AT_decl_file (0x3a) → data1 = 1
                buf.push(1);
                // DW_AT_type (0x49) → ref4 占位（类型区生成后回填）
                let tpos = buf.len();
                buf.extend_from_slice(&0u32.to_le_bytes());
                type_ref_patches.push((tpos, v.ty_desc.clone()));
                // DW_AT_location（属性名在 abbrev code 4/5 中）→ exprloc：
                // DW_OP_fbreg <sleb128 offset>——不写 0x02 属性名。
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

    // 类型区（CU 子项，subprogram 之后——前向引用）：base_type →
    // pointer_type → structure_type（成员引用 base/pointer），再回填占位。
    if full {
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
        // structure_type：取每个 struct desc 的（首）变量成员/大小。
        // DW_AT_name = 类型名（desc 去掉 crate/module 前缀后的末段——
        // DWARF 惯例是裸标识符，路径由 namespace DIE 表达——V1 简化）。
        for desc in &struct_descs {
            let src: Option<&VarEntry> = vars
                .iter()
                .flat_map(|fv| fv.vars.iter())
                .find(|v| v.ty_desc == *desc);
            let Some(entry) = src else { continue };
            type_off_by_desc.insert(desc.clone(), buf.len() as u32);
            buf.push(8); // abbrev code 8: DW_TAG_structure_type
            let simple = desc.rsplit("::").next().unwrap_or(desc.as_str());
            buf.extend_from_slice(simple.as_bytes());
            buf.push(0);
            buf.extend_from_slice(&entry.size.to_le_bytes()); // byte_size data4
            if std::env::var("FORGE_TRACE_DW").is_ok() {
                eprintln!(
                    "[dw-struct] desc={desc} size={} members={:?}",
                    entry.size,
                    entry
                        .members
                        .iter()
                        .map(|m| (&m.name, &m.ty_desc, m.byte_off))
                        .collect::<Vec<_>>()
                );
            }
            for m in &entry.members {
                buf.push(9); // abbrev code 9: DW_TAG_member
                buf.extend_from_slice(m.name.as_bytes());
                buf.push(0);
                let tpos = buf.len();
                buf.extend_from_slice(&0u32.to_le_bytes()); // type 占位
                if let Some(&po) = type_off_by_desc.get(&m.ty_desc) {
                    buf[tpos..tpos + 4].copy_from_slice(&po.to_le_bytes());
                }
                buf.extend_from_slice(&m.byte_off.to_le_bytes()); // data_member_location
            }
            buf.push(0); // structure children terminator
        }
        // enumeration_type：C-like 枚举（注册表）。名称 = desc 末段。
        for e in enums {
            type_off_by_desc.insert(e.desc.clone(), buf.len() as u32);
            buf.push(10); // abbrev code 10: DW_TAG_enumeration_type
            let simple = e.desc.rsplit("::").next().unwrap_or(e.desc.as_str());
            buf.extend_from_slice(simple.as_bytes());
            buf.push(0);
            buf.extend_from_slice(&e.size.to_le_bytes()); // byte_size data4
            for (vn, val) in &e.variants {
                buf.push(11); // abbrev code 11: DW_TAG_enumerator
                buf.extend_from_slice(vn.as_bytes());
                buf.push(0);
                buf.extend_from_slice(&val.to_le_bytes()); // const_value data8
            }
            buf.push(0); // enumeration children terminator
        }
        // 回填变量/参数 DIE 的类型引用
        for (tpos, desc) in &type_ref_patches {
            if let Some(&off4) = type_off_by_desc.get(desc) {
                buf[*tpos..*tpos + 4].copy_from_slice(&off4.to_le_bytes());
            }
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
    vec![
        // code 1：CU，children yes
        1, 0x11, 1, //
        0x25, 0x08, // producer → string
        0x03, 0x08, // name → string
        0x13, 0x05, // language → data2
        0x11, 0x01, // low_pc → addr（CU 代码范围起点）
        0x12, 0x07, // high_pc → data8（相对 low_pc 偏移）
        0x10, 0x06, // stmt_list → data4（.debug_line 偏移 0）
        0, 0, //
        // code 2：subprogram，children no（无变量函数——C1 兼容）
        2, 0x2e, 0, //
        0x03, 0x08, // name → string
        0x3a, 0x0b, // decl_file → data1（gcc 形态）
        0x11, 0x01, // low_pc → addr
        0x12, 0x07, // high_pc → data8（函数代码字节）
        0x3b, 0x06, // decl_line → data4
        0, 0, //
        // code 3：subprogram，children yes（有变量）
        3, 0x2e, 1, //
        0x03, 0x08, // name → string
        0x3a, 0x0b, // decl_file → data1
        0x11, 0x01, // low_pc → addr
        0x12, 0x07, // high_pc → data8
        0x3b, 0x06, // decl_line → data4
        0x40, 0x18, // frame_base → exprloc（DW_FORM_exprloc=0x18）
        0, 0, //
        // code 4：formal_parameter，children no
        4, 0x05, 0, //
        0x03, 0x08, // name → string
        0x3a, 0x0b, // decl_file → data1
        0x49, 0x06, // type → ref4
        0x02, 0x18, // location → exprloc（DW_FORM_exprloc=0x18）
        0x3b, 0x06, // decl_line → data4
        0, 0, //
        // code 5：variable，children no
        5, 0x34, 0, //
        0x03, 0x08, // name → string
        0x3a, 0x0b, // decl_file → data1
        0x49, 0x06, // type → ref4
        0x02, 0x18, // location → exprloc（DW_FORM_exprloc=0x18）
        0x3b, 0x06, // decl_line → data4
        0, 0, //
        // code 6：base_type，children no
        6, 0x24, 0, //
        0x03, 0x08, // name → string
        0x0b, 0x0b, // byte_size → data1
        0x3e, 0x0b, // encoding → data1
        0, 0, //
        // code 7：pointer_type，children no (byte_size 8；type ref4 指向内层
        // base_type——聚合/引用内层为 0 占位，后续扩展)
        7, 0x0f, 0, //
        0x0b, 0x0b, // byte_size → data1
        0x49, 0x06, // type → ref4
        0, 0, //
        // code 8：structure_type，children yes（聚合变量类型——DW_AT_name/
        // byte_size(data4，结构可 >255) + DW_TAG_member 子项）
        8, 0x13, 1, //
        0x03, 0x08, // name → string
        0x0b, 0x06, // byte_size → data4
        0, 0, //
        // code 9: member，children no（DW_AT_name/type ref4/
        // data_member_location data4 = 字节偏移）
        9, 0x0d, 0, //
        0x03, 0x08, // name → string
        0x49, 0x06, // type → ref4
        0x38, 0x06, // data_member_location → data4
        0, 0, //
        // code 10：enumeration_type，children yes（C-like 枚举——name/byte_size
        // data4 + DW_TAG_enumerator 子项）
        10, 0x04, 1, //
        0x03, 0x08, // name → string
        0x0b, 0x06, // byte_size → data4
        0, 0, //
        // code 11：enumerator，children no（name + const_value data8）
        11, 0x28, 0, //
        0x03, 0x08, // name → string
        0x1c, 0x07, // const_value → data8
        0, 0, //
        0, // 整个 abbrev 表终止
    ]
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
    while !buf.len().is_multiple_of(16) {
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

// ============================================================
// .debug_frame（M2：x86_64 CFI）
// ============================================================

/// 生成 `.debug_frame` 段（M2：DWARF CFI——gdb 16.2 amd64-windows 无
/// SEH(.pdata) 时落 dwarf2-frame 解码本段建栈帧；无 CFI 则 bt / info args
/// / info locals 全空，实证根因）。
///
/// 布局对照 mingw gcc -gdwarf-5（objdump --dwarf=frames 净解析，字节实证）：
/// - **CIE**：version=1（.debug_frame 用 v1，不随 DWARF5 主版本）、
///   augmentation=""（无 personality/LSDA）、code_align=1、data_align=-8、
///   RA 列 = **16**（DWARF x86-64 约定；gcc mingw 用 32——PE 异常语义列号，
///   gdb 经 CFA-8 读返回地址不依赖列号）；初始规则 = def_cfa rsp 8 +
///   offset RA 列 @ CFA-8
/// - **每函数一条 FDE**：length 回填 / CIE_pointer = 0 / initial_location
///   占位 0 + ADDR64 reloc → 函数符号（与 CU low_pc 同款路径）/ address_range
///   = 函数代码字节 / 行流 = `FunctionCfi.rows`（forge-codegen emission 对
///   x86 prologue 扫描所得）：advance_loc 到行偏移 + def_cfa_offset /
///   def_cfa_register / offset（保存槽 = CFA 之下 cfa_bytes，操作数 =
///   cfa_bytes/8，data_align=-8 语义）
/// - CIE/FDE 指令流以 DW_CFA_nop 补齐到 **条目总长 % 8 == 0**（gcc 同款：
///   CIE length=20 → 条目 24；FDE length=36 → 条目 40）
///
/// 无 CFI 的函数**不产 FDE**（`fns` 只含 scan 命中的函数——非 x86 后端 /
/// 非常规 prologue 不进本表，注释文档化）。debuginfo 关闭时整段不生成
///（backend.rs 门控）。
///
/// 返回 (字节, relocs: (数据内偏移, 符号名))。
pub fn gen_debug_frame(fns: &[(String, u64, FunctionCfi)]) -> (Vec<u8>, Vec<(usize, String)>) {
    let mut buf: Vec<u8> = Vec::new();
    let mut relocs: Vec<(usize, String)> = Vec::new();

    // ── CIE（一个，全段共用）──
    let cie_pos = buf.len(); // length 字段起点
    buf.extend_from_slice(&0u32.to_le_bytes()); // length 占位（写完回填）
    buf.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // CIE id（32 位格式全 1）
    buf.push(1); // version = 1
    buf.push(0); // augmentation = ""（空字符串，含 NUL）
    buf.push(1); // code_alignment_factor (uleb) = 1
    buf.push(0x78); // data_alignment_factor (sleb) = -8
    buf.push(16); // return_address_column (uleb) = 16（DWARF x86-64；
                  // gcc mingw = 32 是 PE 异常列号，gdb 解栈不依赖）
    buf.push(DW_CFA_DEF_CFA); // def_cfa rsp(7), 8——入口：CFA = rsp + 8
    buf.push(7); // reg = rsp（DWARF 列 7）
    buf.push(8); // offset = 8
    buf.push(DW_CFA_OFFSET | 16); // offset RA 列（16 < 64 → 主操作码）
    buf.push(1); // 操作数 uleb = 8/8（data_align -8 → CFA-8）
    pad_frame_entry(&mut buf, cie_pos);
    backfill_frame_length(&mut buf, cie_pos);

    // ── FDE × 每函数（行按 code_offset 升序，advance_loc 到行点）──
    for (sym, size, cfi) in fns {
        let entry_pos = buf.len(); // length 字段起点
        buf.extend_from_slice(&0u32.to_le_bytes()); // length 占位
        buf.extend_from_slice(&0u32.to_le_bytes()); // CIE_pointer = 0
        // initial_location 占位 0 + reloc → 函数符号（ADDR64，链接器解析
        // 为函数真实地址；占位字节即 addend——此处 0 = 函数起点）
        let loc_pos = buf.len();
        buf.extend_from_slice(&0u64.to_le_bytes());
        relocs.push((loc_pos, sym.clone()));
        // address_range = 函数代码字节（fn_sizes，与 .debug_info high_pc 同源）
        buf.extend_from_slice(&size.to_le_bytes());
        let mut prev = 0u64;
        for (off, ops) in &cfi.rows {
            debug_assert!(
                u64::from(*off) >= prev,
                "CFI rows must be ascending (scan guarantees)"
            );
            push_advance_loc(&mut buf, u64::from(*off) - prev);
            prev = u64::from(*off);
            for op in ops {
                push_cfi_op(&mut buf, op);
            }
        }
        // 尾声不产行（v1）：规则持续到函数末——尾声 pop 只读保存槽内存
        // （槽内容未毁），尾声 PC 解栈仍正确。gcc 额外发 restore/def_cfa
        // 行覆盖 ret 前的最后 1-2 字节，本实现省略（见 cfi.rs 文档）。
        pad_frame_entry(&mut buf, entry_pos);
        backfill_frame_length(&mut buf, entry_pos);
    }
    (buf, relocs)
}

/// DW_CFA_advance_loc：推到 delta 之后（delta 以 code_align=1 计字节）。
fn push_advance_loc(buf: &mut Vec<u8>, delta: u64) {
    if delta == 0 {
        return;
    }
    if delta < 64 {
        buf.push(DW_CFA_ADVANCE_LOC | delta as u8);
    } else {
        buf.push(DW_CFA_ADVANCE_LOC1);
        encode_uleb128(buf, delta);
    }
}

/// 编码一条 CFI 规则（DWARF5 §6.4.2 操作码）。
fn push_cfi_op(buf: &mut Vec<u8>, op: &CfiOp) {
    match op {
        CfiOp::DefCfaOffset(v) => {
            buf.push(DW_CFA_DEF_CFA_OFFSET);
            encode_uleb128(buf, u64::from(*v));
        }
        CfiOp::DefCfaRegister(r) => {
            buf.push(DW_CFA_DEF_CFA_REGISTER);
            encode_uleb128(buf, u64::from(*r));
        }
        CfiOp::SaveReg { dw_reg, cfa_bytes } => {
            // 保存槽在 CFA 之下：DW_CFA_offset 位置 = CFA + 操作数×(-8) =
            // CFA - cfa_bytes → 操作数 = cfa_bytes / 8（uleb）。
            debug_assert!(cfa_bytes % 8 == 0, "save slots are 8-byte aligned");
            if *dw_reg < 64 {
                buf.push(DW_CFA_OFFSET | dw_reg);
            } else {
                // 寄存器列 ≥ 64 需长形式（0x05 offset_extended + uleb reg）——
                // x86 GPR 保存列均 < 64，此分支仅防御性。
                buf.push(0x05);
                encode_uleb128(buf, u64::from(*dw_reg));
            }
            encode_uleb128(buf, u64::from(cfa_bytes / 8));
        }
    }
}

/// 把 length 字段（entry_pos 起的 4 字节）补齐使**条目总长 % 8 == 0**
///（gcc .debug_frame 同款对齐：长度值 ≡ 4 mod 8）。pad 字节 = DW_CFA_nop。
fn pad_frame_entry(buf: &mut Vec<u8>, entry_pos: usize) {
    let total = buf.len() - entry_pos;
    let pad = (8 - (total % 8)) % 8;
    for _ in 0..pad {
        buf.push(DW_CFA_NOP);
    }
}

/// 回填 CIE/FDE 的 length 字段（= 从 entry_pos 起除 length 外全部字节）。
fn backfill_frame_length(buf: &mut Vec<u8>, entry_pos: usize) {
    let len = (buf.len() - entry_pos - 4) as u32;
    buf[entry_pos..entry_pos + 4].copy_from_slice(&len.to_le_bytes());
}

/// 生成全部 DWARF 段。
/// `src_file`：源文件路径（CU 名 + .debug_line file 条目——gdb `list`/
/// 源码断点需要真实文件名，crate 名匹配不到磁盘文件）。
/// `code_span`：CU 代码总字节（high_pc 偏移；+0x200 边距盖过函数间
/// 对齐填充与 main 别名副本——单 CU 超范围无害）。
/// `fn_sizes`：符号 → 函数代码字节（subprogram high_pc；行程序终端行
/// advance 目标）。
/// `fn_cfi`：符号 → (代码字节, CFI 行)——.debug_frame 的 FDE 清单（只含
/// forge-codegen scan 命中的函数；无 CFI 的符号不在此列，不产 FDE）。
/// 返回 (段名, 字节, 段内 reloc: (偏移, 符号名)) 列表。
#[allow(clippy::too_many_arguments)]
pub fn build_dwarf_sections(
    fns: &[(String, u32, Vec<(u32, u32)>)],
    vars: &[FnVarEntries],
    producer: &str,
    src_file: &str,
    code_span: u64,
    fn_sizes: &std::collections::HashMap<String, u64>,
    enums: &[EnumTypeEntry],
    full: bool,
    fn_cfi: &[(String, u64, FunctionCfi)],
) -> Vec<(String, Vec<u8>, Vec<(usize, String)>)> {
    let entries: Vec<(String, u32)> = fns.iter().map(|(s, l, _)| (s.clone(), *l)).collect();
    let span = code_span + 0x200;
    let first_sym = entries.first().map(|(s, _)| s.as_str());
    let (line_bytes, line_relocs) = gen_debug_line(fns, fn_sizes);
    let (frame_bytes, frame_relocs) = gen_debug_frame(fn_cfi);
    let (info_bytes, info_relocs) = gen_debug_info(
        &entries, vars, producer, src_file, span, fn_sizes, enums, full,
    );
    let (ar_bytes, ar_relocs) = gen_debug_aranges(first_sym, span);
    vec![
        (".debug_line".to_string(), line_bytes, line_relocs),
        (
            ".debug_line_str".to_string(),
            gen_debug_line_str(src_file),
            vec![],
        ),
        (".debug_info".to_string(), info_bytes, info_relocs),
        (".debug_aranges".to_string(), ar_bytes, ar_relocs),
        (".debug_abbrev".to_string(), gen_debug_abbrev(), vec![]),
        (".debug_frame".to_string(), frame_bytes, frame_relocs),
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

/// 编码 ULEB128（DW_CFA 操作数 / advance_pc delta 用）。
fn encode_uleb128(buf: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        buf.push(byte | if v == 0 { 0 } else { 0x80 });
        if v == 0 {
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
            (
                "helper".to_string(),
                20u32,
                vec![(4u32, 21u32), (12u32, 22u32)],
            ),
        ];
        // fn_sizes（终端行 advance 目标）：main 30B / helper 40B
        let mut sizes = std::collections::HashMap::new();
        sizes.insert("main".to_string(), 30u64);
        sizes.insert("helper".to_string(), 40u64);
        let (bytes, relocs) = gen_debug_line(&fns, &sizes);
        assert!(bytes.len() > 24, "line program too small");
        // 函数级 2 + helper 的 2 个 per-statement = 4 个 reloc
        assert_eq!(relocs.len(), 4, "reloc per line entry");
        assert_eq!(relocs[0].1, "main");
        // .debug_line_str 串池：gen_debug_line_str 携带源路径
        let strs = gen_debug_line_str("test_crate");
        assert!(
            strs.windows(10).any(|w| w == b"test_crate"),
            "line_str 池含路径"
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
        let mut sizes = std::collections::HashMap::new();
        sizes.insert("helper".to_string(), 40u64);
        let (bytes, relocs) = gen_debug_line(&fns, &sizes);
        assert_eq!(relocs.len(), 2);
        // 第 2 个 reloc 的占位（relocs[1].0 处 8 字节）= 12
        let off = relocs[1].0;
        let v = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        assert_eq!(
            v, 12,
            "per-statement set_address placeholder = instr offset"
        );
    }

    #[test]
    fn info_has_cu_and_subprograms() {
        let entries = vec![("f".to_string(), 5u32)];
        let (bytes, relocs) = gen_debug_info(
            &entries,
            &[],
            "forge",
            "test",
            0x30,
            &Default::default(),
            &[],
            false,
        );
        // relocs：CU low_pc(1) + subprogram low_pc(1)
        assert_eq!(relocs.len(), 2);
        assert_eq!(relocs[0].1, "f", "CU low_pc reloc → first fn");
        // header：unit_length(4) + version(2) + unit_type(1) + addr_size(1)
        // + abbr_off(4) = 12——CU DIE abbrev code = 1 在 index 12（v5）
        assert_eq!(bytes[12], 1);
        // CU 后子项 terminator(1) + subprogram abbrev code = 2
        assert!(
            bytes.windows(1).any(|w| w[0] == 2),
            "subprogram abbrev 2 present"
        );
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
                    members: vec![],
                    size: 4,
                },
                VarEntry {
                    name: "a".to_string(),
                    slot_offset: -32,
                    ty_desc: "i32".to_string(),
                    is_arg: true,
                    decl_line: 6,
                    members: vec![],
                    size: 4,
                },
                VarEntry {
                    name: "b".to_string(),
                    slot_offset: -16,
                    ty_desc: "&i32".to_string(),
                    is_arg: true,
                    decl_line: 6,
                    members: vec![],
                    size: 8,
                },
            ],
        }];
        let (bytes, relocs) = gen_debug_info(
            &entries,
            &vars,
            "forge",
            "test",
            0x30,
            &Default::default(),
            &[],
            true,
        );
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
        assert!(
            bytes.windows(1).any(|w| w[0] == 0x91),
            "DW_OP_fbreg present"
        );
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
                VarEntry {
                    name: "x".to_string(),
                    slot_offset: -24,
                    ty_desc: "i32".to_string(),
                    is_arg: false,
                    decl_line: 7,
                    members: vec![],
                    size: 4,
                },
                VarEntry {
                    name: "a".to_string(),
                    slot_offset: -32,
                    ty_desc: "i32".to_string(),
                    is_arg: true,
                    decl_line: 6,
                    members: vec![],
                    size: 4,
                },
                VarEntry {
                    name: "b".to_string(),
                    slot_offset: -16,
                    ty_desc: "&i32".to_string(),
                    is_arg: true,
                    decl_line: 6,
                    members: vec![],
                    size: 8,
                },
                VarEntry {
                    name: "p".to_string(),
                    slot_offset: -40,
                    ty_desc: "varprobe::Point".to_string(),
                    is_arg: false,
                    decl_line: 8,
                    members: vec![
                        VarMember {
                            name: "y".to_string(),
                            ty_desc: "i32".to_string(),
                            byte_off: 4,
                        },
                        VarMember {
                            name: "x".to_string(),
                            ty_desc: "i32".to_string(),
                            byte_off: 0,
                        },
                    ],
                    size: 8,
                },
            ],
        }];
        let (bytes, relocs) = gen_debug_info(
            &entries,
            &vars,
            "forge",
            "test",
            0x30,
            &Default::default(),
            &[],
            true,
        );
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
        p += 2; // version (5)
        p += 1; // unit_type
        p += 1; // address_size
        p += 4; // debug_abbrev_offset
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
        // CU 子项：base_type(6) / pointer_type(7) / structure_type(8) /
        // subprogram(2|3)，0 终止（类型区在 subprogram 后——前向引用）
        let mut found_base = false;
        let mut found_ptr = false;
        let mut found_sub = false;
        let mut found_struct = false;
        let mut member_offs: Vec<(String, u32)> = Vec::new();
        let mut struct_mem_types: Vec<u32> = Vec::new();
        let mut base_off: Option<u32> = None;
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
                    let die_start = (p - 1) as u32; // abbrev 码位置 = DIE 起点
                    let bname = rd_str(&mut p);
                    if bname == "i32" {
                        base_off = Some(die_start);
                    }
                    p += 2;
                    found_base = true;
                }
                7 => {
                    // pointer_type：byte_size d1 / type ref4
                    p += 1;
                    rd_u32(&mut p);
                    found_ptr = true;
                }
                8 => {
                    // structure_type：name str / byte_size data4 + 子项 code 9
                    let nm = rd_str(&mut p);
                    let sz = rd_u32(&mut p);
                    assert!(nm.ends_with("Point"), "structure name = Point，got {nm}");
                    assert_eq!(sz, 8, "byte_size");
                    let mut mem_types: Vec<u32> = Vec::new();
                    loop {
                        let c = bytes[p];
                        if c == 0 {
                            p += 1;
                            break;
                        }
                        p += 1;
                        assert_eq!(c, 9, "member abbrev 9, got {c}");
                        let mn = rd_str(&mut p);
                        mem_types.push(rd_u32(&mut p)); // member type ref4
                        let off = rd_u32(&mut p); // data_member_location
                        member_offs.push((mn, off));
                    }
                    found_struct = true;
                    struct_mem_types = mem_types;
                }
                2 => {
                    // subprogram：name str / decl_file d1 / low_pc addr8 /
                    // high_pc data8 / decl_line data4
                    rd_str(&mut p);
                    p += 1; // decl_file
                    p += 8;
                    p += 8; // high_pc data8（测试用空 fn_sizes = 0）
                    rd_u32(&mut p);
                    found_sub = true;
                }
                3 => {
                    // subprogram（children yes）：name / decl_file / low_pc /
                    // high_pc / decl_line / frame_base exprloc + children（4/5）至 0
                    rd_str(&mut p);
                    p += 1; // decl_file
                    p += 8;
                    p += 8; // high_pc data8
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
                        p += 1; // decl_file
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
                    bytes
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
            }
        }
        assert_eq!(p, bytes.len(), "DIE stream must parse to exact end");
        assert!(
            found_base && found_ptr && found_sub && found_struct,
            "all DIE kinds present"
        );
        // structure_type 成员（x@0 / y@4——layout 实测偏移，与槽内一致）
        member_offs.sort_by_key(|(_, o)| *o);
        assert_eq!(
            member_offs,
            vec![("x".to_string(), 0u32), ("y".to_string(), 4u32)],
            "struct members with byte offsets"
        );
        // 成员 type ref 指向 i32 base_type（非 0）
        let bo = base_off.expect("i32 base_type present");
        assert_eq!(
            struct_mem_types,
            vec![bo, bo],
            "member type refs → i32 base"
        );
        // fbreg 偏移（sleb128，x = slot_offset）：x=-32 / p=-40 / b=-16 / a=-24
        let mut offs = fbreg_offsets;
        offs.sort_unstable();
        assert_eq!(
            offs,
            vec![-40, -32, -24, -16],
            "fbreg offsets from slot offsets"
        );
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
            (
                "main".to_string(),
                10u32,
                vec![(4u32, 11u32), (12u32, 12u32)],
            ),
            ("helper".to_string(), 20u32, vec![]),
        ];
        // fn_sizes：终端行 advance 目标（main 末行 @12 → 推进 8 到 20；
        // helper 仅 decl 行 @0 → 推进 24 到 24）
        let mut sizes = std::collections::HashMap::new();
        sizes.insert("main".to_string(), 20u64);
        sizes.insert("helper".to_string(), 24u64);
        let (bytes, relocs) = gen_debug_line(&fns, &sizes);
        // 头部 reloc 数 = 4 条目（main + 2 stmt + helper）
        assert_eq!(relocs.len(), 4);

        let mut p = 0usize;
        let rd_u32 = |p: &mut usize| {
            let v = u32::from_le_bytes(bytes[*p..*p + 4].try_into().unwrap());
            *p += 4;
            v
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
        let unit_len = rd_u32(&mut p);
        assert_eq!(
            unit_len as usize,
            bytes.len() - 4,
            "unit_length covers section"
        );
        // v5：version u16 / address_size / segment_selector_size / header_length
        assert_eq!(bytes[p], 5, "line version lo");
        assert_eq!(bytes[p + 1], 0, "line version hi");
        p += 2;
        assert_eq!(bytes[p], 8, "address_size");
        p += 1;
        assert_eq!(bytes[p], 0, "segment_selector_size");
        p += 1;
        let hl = rd_u32(&mut p);
        let header_end = p + hl as usize;
        assert_eq!(bytes[p], 1, "min_inst_length");
        p += 1;
        assert_eq!(bytes[p], 1, "max_ops_per_inst");
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
        // v5：目录表 format_count = 0 + dir_count = 0（空表）
        assert_eq!(bytes[p], 0, "dir format count 0");
        p += 1;
        assert_eq!(bytes[p], 0, "dir count 0");
        p += 1;
        // 文件表：format_count = 1，[DW_LNCT_path → DW_FORM_line_strp]，
        // count = 1，entry = line_strp 偏移 0
        assert_eq!(bytes[p], 1, "file format count 1");
        p += 1;
        assert_eq!(bytes[p], 1, "DW_LNCT_path");
        p += 1;
        assert_eq!(bytes[p], 0x1f, "DW_FORM_line_strp");
        p += 1;
        assert_eq!(
            rd_uleb(&mut p),
            2,
            "file_names_count 2（gcc hack：0/1 基读者皆命中）"
        );
        // line_strp 偏移 4 字节
        assert_eq!(&bytes[p..p + 4], &[0, 0, 0, 0], "file[0] strp 偏移 0");
        p += 4;
        assert_eq!(
            &bytes[p..p + 4],
            &[0, 0, 0, 0],
            "file[1] strp 偏移 0（同串）"
        );
        p += 4;
        assert_eq!(p, header_end, "header ends exactly at header_length");

        // 行程序：每函数一个 sequence（decl 行 + stmt 行 + 终端 advance +
        // end_sequence）——行寄存器 sequence 起点 = 1，同 sequence 内累加
        //（delta 相对上一行）。终端行（M2）：末行 copy 后 advance_pc 到
        // fn_sizes（末行不再零跨度——否则 gdb 丢零长度行，行号丢失）。
        let mut ops: Vec<i64> = Vec::new(); // 每行 advance_line 操作数
        let mut addrs_found: Vec<u64> = Vec::new();
        let mut want_rows: Vec<(u32, i64)> = Vec::new(); // (addr, 目标行)
        for (_sym, line, stmts) in &fns {
            let mut fn_rows: Vec<(u32, i64)> = vec![(0, *line as i64)];
            for (off, ln) in stmts {
                fn_rows.push((*off, *ln as i64));
            }
            want_rows.extend(fn_rows.iter().copied());
            // 该函数序列的行记录
            for _ in 0..fn_rows.len() {
                // 行记录（set_address + set_file + advance_line + copy）
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
                ops.push(decode_sleb(&bytes[s0..p]));
                assert_eq!(bytes[p], 1, "DW_LNS_copy");
                p += 1;
            }
            // 终端行 advance_pc：末行 copy 后推进到 fn_sizes[sym]
            let want_advance = sizes.get(_sym.as_str()).copied().unwrap_or(0)
                - fn_rows.last().unwrap().0 as u64;
            if want_advance > 0 {
                assert_eq!(bytes[p], 2, "DW_LNS_advance_pc");
                p += 1;
                assert_eq!(
                    rd_uleb(&mut p),
                    want_advance,
                    "terminal advance to fn end ({_sym})"
                );
            }
            // end_sequence
            assert_eq!(bytes[p], 0, "extended");
            p += 1;
            assert_eq!(bytes[p], 1, "end_sequence len");
            p += 1;
            assert_eq!(bytes[p], 1, "DW_LNE_end_sequence");
            p += 1;
        }
        assert_eq!(p, bytes.len(), "program consumed to exact end");
        // 解码行：行寄存器在 end_sequence 后复位为 1（每函数序列起点 1）——
        // 按函数分组累加 delta 得目标行。
        let mut want_lines: Vec<i64> = Vec::new();
        let mut got_lines: Vec<i64> = Vec::new();
        let mut reg = 1i64;
        let mut op_i = 0usize;
        for (_sym, line, stmts) in &fns {
            want_lines.push(*line as i64);
            reg += ops[op_i];
            got_lines.push(reg);
            op_i += 1;
            for (_, ln) in stmts {
                want_lines.push(*ln as i64);
                reg += ops[op_i];
                got_lines.push(reg);
                op_i += 1;
            }
            reg = 1; // end_sequence 复位
        }
        assert_eq!(got_lines, want_lines, "每 sequence 行 = 目标行");
        let want_addrs: Vec<u64> = want_rows.iter().map(|(a, _)| *a as u64).collect();
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

    #[test]
    fn enum_info_structurally_parses() {
        // C-like 枚举：enumeration_type（code 10：name/byte_size）+
        // enumerator 子项（code 11：name/const_value data8）；枚举变量 DIE
        // 的 type ref 指向枚举 DIE（占位回填）。
        let entries = vec![("f".to_string(), 5u32)];
        let vars = vec![FnVarEntries {
            sym: "f".to_string(),
            vars: vec![VarEntry {
                name: "c".to_string(),
                slot_offset: -16,
                ty_desc: "varprobe::Color".to_string(),
                is_arg: false,
                decl_line: 9,
                members: vec![],
                size: 1,
            }],
        }];
        let enums = vec![EnumTypeEntry {
            desc: "varprobe::Color".to_string(),
            size: 1,
            variants: vec![
                ("Red".to_string(), 0u64),
                ("Green".to_string(), 1u64),
                ("Blue".to_string(), 2u64),
            ],
        }];
        let (bytes, relocs) = gen_debug_info(
            &entries,
            &vars,
            "forge",
            "test",
            0x30,
            &Default::default(),
            &enums,
            true,
        );
        assert_eq!(relocs.len(), 2, "CU + fn relocs");
        // 枚举 DIE：code 10 段含 name "Color" + byte_size 1
        let s = String::from_utf8_lossy(&bytes);
        assert!(
            s.contains("Color\u{0}\u{01}\u{00}\u{00}\u{00}"),
            "enum Color byte_size 1"
        );
        assert!(
            s.contains("Red\u{0}\u{00}\u{00}\u{00}\u{00}\u{00}\u{00}\u{00}\u{00}"),
            "enumerator Red const 0"
        );
        assert!(
            s.contains("Blue\u{0}\u{02}\u{00}\u{00}\u{00}\u{00}\u{00}\u{00}\u{00}\u{00}"),
            "enumerator Blue const 2"
        );
        // 结构走查：CU → subprogram code 3（child c）→ 枚举区 code 10
        let mut p = 12usize; // v5 头 12 字节
        assert_eq!(bytes[p], 1, "CU abbrev");
        p += 1;
        // 跳 producer/name 串 + language + low/high/stmt_list
        while bytes[p] != 0 {
            p += 1;
        }
        p += 1;
        while bytes[p] != 0 {
            p += 1;
        }
        p += 1;
        p += 2 + 8 + 8 + 4; // language + low_pc + high_pc + stmt_list
        assert_eq!(bytes[p], 3, "subprogram code 3");
        p += 1;
        while bytes[p] != 0 {
            p += 1;
        }
        p += 1;
        p += 1 + 8 + 8 + 4; // decl_file + low + high + decl_line
        // frame_base exprloc [1, 0x56]
        assert_eq!(bytes[p], 1);
        p += 1;
        assert_eq!(bytes[p], 0x56);
        p += 1;
        // child c：code 5（variable）
        assert_eq!(bytes[p], 5);
        p += 1;
        while bytes[p] != 0 {
            p += 1;
        }
        p += 1;
        p += 1; // decl_file
        let enum_ref = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        // location exprloc + decl_line data4
        let loc_len = bytes[p] as usize;
        p += 1 + loc_len;
        p += 4;
        assert_eq!(bytes[p], 0, "children terminator");
        p += 1;
        // 类型区：枚举 code 10
        assert_eq!(bytes[p], 10, "enumeration_type");
        let enum_start = p; // abbrev 码位置 = DIE 起点
        p += 1;
        while bytes[p] != 0 {
            p += 1;
        }
        p += 1;
        let e_size = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        assert_eq!(e_size, 1, "enum byte_size");
        assert_eq!(
            enum_start as usize, enum_ref as usize,
            "var ref → enum DIE 起点"
        );
        for (vn, vv) in [("Red", 0u64), ("Green", 1u64), ("Blue", 2u64)] {
            assert_eq!(bytes[p], 11, "enumerator");
            p += 1;
            let nm = {
                let s0 = p;
                while bytes[p] != 0 {
                    p += 1;
                }
                String::from_utf8_lossy(&bytes[s0..p]).into_owned()
            };
            p += 1;
            let val = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
            p += 8;
            assert_eq!(nm, vn);
            assert_eq!(val, vv);
        }
        assert_eq!(bytes[p], 0, "enum children terminator");
        p += 1;
        assert_eq!(bytes[p], 0, "CU children terminator");
        p += 1;
        assert_eq!(p, bytes.len(), "exact end");
    }

    #[test]
    fn frames_structurally_decodes() {
        // .debug_frame（M2）：用 forge-codegen scan 的 prologue 产物驱动
        // gen_debug_frame，结构解码整个段：CIE 头 + 每函数 FDE（reloc /
        // address_range / 行流状态机）——指令流必须能精确消费到条目边界且
        // 条目总长 8 对齐（gcc 同款）。解码器状态终点 = CFA=rbp+16 + 全部
        // callee-saved 保存槽（对照 scan 行集的语义结果）。
        let prefix: [u8; 15] = [
            0x55, 0x48, 0x89, 0xE5, 0x53, 0x57, 0x56, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41,
            0x57,
        ];
        let cfi_a = code_forge::backend::scan_x86_prologue(&prefix).expect("prologue rows");
        let mut code_b = prefix.to_vec();
        code_b.extend_from_slice(&[0x48, 0x81, 0xEC, 0x20, 0, 0, 0]); // sub rsp, 32
        code_b.extend_from_slice(&[0xC3]); // ret
        let cfi_b = code_forge::backend::scan_x86_prologue(&code_b).expect("rows w/ body");
        assert_eq!(cfi_a, cfi_b, "scan stops right after the 7 pushes");
        let fns: Vec<(String, u64, FunctionCfi)> = vec![
            ("f_alpha".to_string(), 40u64, cfi_a),
            ("f_beta".to_string(), 16u64, cfi_b),
        ];
        let (bytes, relocs) = gen_debug_frame(&fns);
        assert_eq!(relocs.len(), 2, "one FDE reloc per fn");
        assert_eq!(relocs[0].1, "f_alpha");
        assert_eq!(relocs[1].1, "f_beta");

        let mut p = 0usize;
        let rd_u32 = |p: &mut usize| {
            let v = u32::from_le_bytes(bytes[*p..*p + 4].try_into().unwrap());
            *p += 4;
            v
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

        // ── CIE ──
        let cie_len = rd_u32(&mut p);
        let cie_end = p + cie_len as usize;
        assert_eq!(&bytes[p..p + 4], &[0xFF, 0xFF, 0xFF, 0xFF], "CIE id");
        p += 4;
        assert_eq!(bytes[p], 1, "version 1（gcc .debug_frame 同款）");
        p += 1;
        assert_eq!(bytes[p], 0, "augmentation empty");
        p += 1;
        assert_eq!(bytes[p], 1, "code_align = 1");
        p += 1;
        assert_eq!(bytes[p], 0x78, "data_align = -8（sleb）");
        p += 1;
        assert_eq!(rd_uleb(&mut p), 16, "RA column 16（DWARF x86-64；gcc=32）");
        // 初始规则：def_cfa rsp 8 + offset RA 列（0x90）@ CFA-8（操作数 1）
        assert_eq!(bytes[p], 0x0c, "DW_CFA_def_cfa");
        p += 1;
        assert_eq!(rd_uleb(&mut p), 7, "cfa reg rsp");
        assert_eq!(rd_uleb(&mut p), 8, "cfa offset 8");
        assert_eq!(bytes[p], 0x90, "DW_CFA_offset r16（0x80|16）");
        p += 1;
        assert_eq!(rd_uleb(&mut p), 1, "RA @ CFA-8");
        // 对齐 pad（NOP）到条目总长 % 8 == 0
        while p < cie_end {
            assert_eq!(bytes[p], DW_CFA_NOP, "CIE pad nop");
            p += 1;
        }
        assert_eq!(cie_end % 8, 0, "CIE entry 8-aligned（gcc 同款）");
        assert_eq!(cie_len as usize, cie_end - 4, "length covers entry");

        // ── FDE（每函数：length / cie_ptr / loc占位+reloc / range / 行流）──
        // 解码器状态：继承 CIE 初始（cfa_reg=rsp, off=8），FDE 行流增量更新。
        let mut cfa_reg = 7u8;
        let mut cfa_off = 8u64;
        let mut saves: std::collections::HashMap<u8, u64> = Default::default(); // reg → factor
        for (i, (sym, want_size, _)) in fns.iter().enumerate() {
            let fde_start = p;
            let fde_len = rd_u32(&mut p);
            let fde_end = p + fde_len as usize;
            assert_eq!(rd_u32(&mut p), 0, "CIE_pointer = 0（首个 CIE）");
            assert_eq!(
                p,
                relocs[i].0,
                "initial_location placeholder 与 reloc 位置一致 ({sym})"
            );
            assert_eq!(&bytes[p..p + 8], &[0u8; 8], "initial_location 占位 0");
            p += 8;
            let range = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
            p += 8;
            assert_eq!(range, *want_size, "address_range = fn size ({sym})");
            // 行流解码：advance_loc（主/长形式）+ def_cfa_offset（0x0e）/
            // def_cfa_register（0x0d）/ offset（0x80|reg）——状态机直到条目尾
            let mut loc = 0u64;
            while p < fde_end {
                let op = bytes[p];
                p += 1;
                match op {
                    0x40..=0x7f => loc += u64::from(op & 0x3f),
                    0x02 => loc += rd_uleb(&mut p), // advance_loc1
                    0x0e => cfa_off = rd_uleb(&mut p),
                    0x0d => cfa_reg = rd_uleb(&mut p) as u8,
                    0x80..=0xbf => {
                        let reg = op & 0x3f;
                        saves.insert(reg, rd_uleb(&mut p));
                    }
                    0x05 => {
                        // offset_extended（防御分支，scan 不会产）
                        let reg = rd_uleb(&mut p) as u8;
                        saves.insert(reg, rd_uleb(&mut p));
                    }
                    0x00 => break, // pad nop（对齐补丁，行流结束）
                    other => panic!("unexpected CFA opcode {other:#x} at {p}"),
                }
            }
            assert_eq!(loc, 15, "行流推进到 prologue 末（15）({sym})");
            while p < fde_end {
                assert_eq!(bytes[p], DW_CFA_NOP, "FDE pad nop");
                p += 1;
            }
            assert_eq!(p, fde_end, "FDE 行流精确消费到条目边界");
            assert_eq!((fde_end - fde_start) % 8, 0, "FDE entry 8-aligned");
        }
        assert_eq!(p, bytes.len(), "整个 .debug_frame 精确消费");
        // 解码终态 = scan 行集的语义结果：CFA=rbp(6)+16；7 callee-saved +
        // rbp 全部有保存槽（factor × data_align -8 = 槽位）
        assert_eq!(cfa_reg, 6, "CFA = rbp");
        assert_eq!(cfa_off, 16, "CFA = rbp + 16（8 返回 + 8 保存 rbp）");
        let want_saves: std::collections::HashMap<u8, u64> = [
            (6u8, 2u64), // rbp @ -16
            (3, 3),      // rbx @ -24
            (5, 4),      // rdi @ -32
            (4, 5),      // rsi @ -40
            (12, 6),     // r12 @ -48
            (13, 7),     // r13 @ -56
            (14, 8),     // r14 @ -64
            (15, 9),     // r15 @ -72
        ]
        .into_iter()
        .collect();
        assert_eq!(saves, want_saves, "全部 callee-saved 保存槽到位");
    }

    #[test]
    fn line_end_advance_to_fn_end() {
        // 终端行（M2）：末行 copy 后、end_sequence 前必须 advance_pc 到
        // fn_sizes（末行不再零跨度——gdb 丢零长度行 → 最后一行行号丢失）。
        // 1) 单函数 decl@0 + stmt@4，size=20 → 末行(4)推进 16 → 20
        let fns = vec![("f".to_string(), 10u32, vec![(4u32, 11u32)])];
        let mut sizes = std::collections::HashMap::new();
        sizes.insert("f".to_string(), 20u64);
        let (bytes, _) = gen_debug_line(&fns, &sizes);
        // 程序尾：advance_pc(2) uleb16(0x10) + end_sequence(0 1 1)
        assert_eq!(
            &bytes[bytes.len() - 5..],
            &[DW_LNS_ADVANCE_PC, 0x10, 0, 1, DW_LNE_END_SEQUENCE],
            "decl+1 stmt 的序列以 advance_pc 16 收尾"
        );
        // 2) 无 per-statement（仅 decl@0），size=20 → 推进 20（decl 行覆盖全函数）
        let fns = vec![("f".to_string(), 10u32, vec![])];
        let (bytes, _) = gen_debug_line(&fns, &sizes);
        assert_eq!(
            &bytes[bytes.len() - 5..],
            &[DW_LNS_ADVANCE_PC, 0x14, 0, 1, DW_LNE_END_SEQUENCE],
            "decl-only 序列以 advance_pc 20 收尾（decl 行覆盖全函数）"
        );
        // 3) fn_sizes 缺失（未知大小）→ 无 advance（保持旧行为，安全）
        let fns = vec![("g".to_string(), 10u32, vec![(4u32, 11u32)])];
        let (bytes, _) = gen_debug_line(&fns, &Default::default());
        assert_eq!(
            &bytes[bytes.len() - 3..],
            &[0, 1, DW_LNE_END_SEQUENCE],
            "大小未知：直接 end_sequence（无 advance）"
        );
        // 4) advance_pc 不发 reloc（行表 reloc 数 = 行数，不含终端 advance）
        let (_, relocs) = gen_debug_line(&fns, &Default::default());
        let fns = vec![("f".to_string(), 10u32, vec![(4u32, 11u32), (12u32, 12u32)])];
        let (_, relocs2) = gen_debug_line(&fns, &sizes);
        assert_eq!(relocs.len(), 2, "decl + stmt（无 reloc 变化）");
        assert_eq!(relocs2.len(), 3, "advance_pc 不产 reloc");
    }
}
