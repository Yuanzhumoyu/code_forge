//! forge-ir 二进制序列化（IR bitcode v1）。
//!
//! # 目标
//!
//! `Module` ⇄ 字节流**无损**往返：字节流确定（同输入同输出）、解码 fail-closed
//! （截断/越界/未知 tag 一律 `Err`，绝不 panic、绝不静默丢数据）。
//!
//! # 容器布局（v1）
//!
//! ```text
//! magic "FORGEIR\0"(8) | varint 版本 | producer(varint 长度 + UTF-8)
//! | varint 段数 | 段表: [u8 id | varint offset | varint len] × 段数 | 段体…
//! ```
//!
//! 段 id 见 [`SectionId`]；`offset` 是**绝对偏移**，段体按 id 升序紧密排列。
//! 编码原语：LEB128 varint / zigzag、字符串表（索引 0 = 空串）、句柄一律 dense index、
//! 枚举写显式判别值并在读侧穷举 `match`。
//!
//! # 纪律
//!
//! - **版本不兼容即错**（仓库既有决策"无需兼容旧版本结构"）；演进只允许"加段 + 升版本"；
//! - **不做不可信分配**：长度字段先与剩余字节比对再分配；
//! - **确定性**：不遍历 `HashMap` 决定输出顺序（写侧字符串表的 `HashMap` 只作索引）；
//! - **字符串表逐条往返**：表不预留"空串槽"，索引即池句柄顺序 ⇒ 解码后的
//!   `StringPool` 与编码前**逐条相同**（含空串在内，不多不少）。
//! - 完整规范与切片计划见 `docs/plans/forge-ir-binary-serialization-plan.md`。

mod consts;
pub mod format;
mod funcs;
mod globals;
mod meta;
mod reader;
mod types;
mod writer;

pub use format::{IR_FORMAT_VERSION, MAGIC, PRODUCER, SectionId};

use crate::error::IrError;
use crate::ir::function::Module;
use crate::util::imm_str::ImmStr;
use crate::util::string_pool::InternedStr;

/// 只读头部即可回答"这份字节流我能不能读"（不建任何 IR）。
///
/// 用于缓存方在反序列化前做版本/段检查，避免"读到一半才发现版本不符"。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinaryCompat {
    /// 流里的格式版本（当前只可能是 [`IR_FORMAT_VERSION`]，否则 `Err`）。
    pub format_version: u16,
    /// 写侧 producer 串（诊断用；跨版本不保证字节流相同）。
    ///
    /// 开放集合字符串一律 `ImmStr`（v3 S5 边界规则；`open_set_boundary.rs` 钉住）。
    pub producer: ImmStr,
    /// 流里出现的段（按段表顺序 = id 升序）。
    pub sections: Vec<SectionId>,
}

/// 只解析头部 + 段表（含结构校验），返回兼容信息。
///
/// 错误：魔数不符、版本不符、头部/段表截断、未知段 id、段越界/重叠、重复段——
/// 全部 `IrError::BinaryDecode { offset, msg }`。
pub fn check_binary_compat(bytes: &[u8]) -> Result<BinaryCompat, IrError> {
    let header = reader::parse_header(bytes)?;
    Ok(BinaryCompat {
        format_version: header.version,
        producer: ImmStr::from(header.producer.as_str()),
        sections: header.sections.iter().map(|e| e.id).collect(),
    })
}

impl Module {
    /// 序列化为确定性字节流（同输入必同输出）。
    ///
    /// 不缓存：每次调用都重新编码（IR 可能在两次调用之间被改写）。
    pub fn to_binary(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.to_binary_into(&mut out);
        out
    }

    /// 序列化并**追加**到 `out`（供调用方复用缓冲；`out` 原内容保留）。
    pub fn to_binary_into(&self, out: &mut Vec<u8>) {
        encode_module(self, out);
    }

    /// 从字节流重建模块。
    ///
    /// 截断/版本不符/未知段/段越界/字符串表重复/非法 UTF-8 一律 `Err`（不 panic）。
    pub fn from_binary(bytes: &[u8]) -> Result<Module, IrError> {
        decode_module(bytes)
    }
}

// ============================================================
// 编码 / 解码
// ============================================================

/// 编码一个模块（B1：COMPAT + STRINGS；后续切片在此追加各段 writer）。
fn encode_module(module: &Module, out: &mut Vec<u8>) {
    let mut w = writer::Writer::new();
    // COMPAT：flags = 0（v1 无任何标志位；未知位在读侧是硬错）。
    writer::put_varint(w.section(SectionId::Compat), 0);
    // STRINGS：按类型库字符串池顺序**整池**登记；池句柄 → 表索引的映射记在 writer
    // 里（表索引 0 恒为空串），各段引用池内字符串一律经 `pool_string_index` 换算。
    {
        let store = module.types.borrow();
        let pool: Vec<ImmStr> = (0..store.strings.len())
            .map(|i| ImmStr::from(store.strings.lookup(InternedStr(i as u32))))
            .collect();
        drop(store);
        w.intern_pool(&pool);
        debug_assert_eq!(
            w.string_count(),
            pool.len(),
            "表 = 池内容逐条（不预留空串槽）"
        );
    }
    // TYPES：条目 + 命名类型 + 签名表 + DataLayout。
    types::encode_types(&module.types.borrow(), &mut w);
    // CONSTS：五通道（int/float/big/vector/aggregate）。
    consts::encode_consts(&module.constants, &mut w);
    // METADATA：节点表 + 命名表（必须在 FUNCS/GLOBALS 之前：它们是附件 id 的来源）。
    meta::encode_metadata(&module.metadata_store, &mut w);
    // FUNCS：函数表（签名/属性/符号 + 每函数常量池 + dfg + layout）。
    funcs::encode_funcs(module, &mut w);
    // GLOBALS：全局变量/别名/comdat；MODULE：三元组/源文件/模块 asm。
    globals::encode_globals(module, &mut w);
    globals::encode_module_section(module, &mut w);
    w.finish(out);
}

/// 解码一个模块（B1–B3：COMPAT + STRINGS + TYPES + CONSTS；后续切片按依赖顺序追加）。
fn decode_module(bytes: &[u8]) -> Result<Module, IrError> {
    let reader = reader::Reader::parse(bytes)?;
    let mut module = Module::new();
    {
        let mut store = module.types.borrow_mut();
        for (i, s) in reader.strings().iter().enumerate() {
            // 字符串表已保证无重复 ⇒ 逐项 intern 得到的新句柄必然等于其索引
            // （池从空开始、插入序即索引序）。这里用 debug 断言钉住该推理。
            let handle = store.strings.intern(s.clone());
            debug_assert_eq!(handle.0 as usize, i, "字符串表已去重，句柄应等于索引");
        }
    }
    // TYPES（必须在 CONSTS/FUNCS 之前：类型是它们的输入）。
    if let Some(cursor) = reader.section(SectionId::Types) {
        let mut store = module.types.borrow_mut();
        types::decode_types(&mut store, cursor, reader.strings())?;
    }
    // CONSTS（聚合常量引用标量常量，段内自带先后序）。
    if let Some(cursor) = reader.section(SectionId::Consts) {
        consts::decode_consts(&mut module.constants, cursor)?;
    }
    // METADATA（必须先于 FUNCS/GLOBALS：附件 id 指向这里的节点）。
    if let Some(cursor) = reader.section(SectionId::Metadata) {
        meta::decode_metadata(&mut module.metadata_store, cursor, reader.strings())?;
    }
    // FUNCS（依赖 TYPES/CONSTS：句柄按模块类型库与常量池校验）。
    if let Some(cursor) = reader.section(SectionId::Funcs) {
        let type_count = module.types.borrow().type_count();
        funcs::decode_funcs(&mut module, cursor, reader.strings(), type_count)?;
    }
    // GLOBALS（全局/别名/comdat 经 add_* 回放：名字索引表随之重建）。
    if let Some(cursor) = reader.section(SectionId::Globals) {
        globals::decode_globals(&mut module, cursor, reader.strings())?;
    }
    // MODULE（三元组/源文件/模块 asm）。
    if let Some(cursor) = reader.section(SectionId::Module) {
        globals::decode_module_section(&mut module, cursor, reader.strings())?;
    }
    validate_metadata_refs(&module).map_err(|msg| IrError::BinaryDecode {
        offset: bytes.len(),
        msg,
    })?;
    Ok(module)
}

/// 附件 metadata 的**悬空引用**校验：函数头/指令/全局/别名的每个 `MetadataId`
/// 必须落在 `metadata_store` 的节点数之内。
///
/// 解码顺序保证 METADATA 段先于 FUNCS/GLOBALS，因此这里只需查界——越界即文件
/// 自称引用了不存在的节点（fail-closed，不让下游 display/verifier 去猜）。
fn validate_metadata_refs(module: &Module) -> Result<(), String> {
    let store_len = module.metadata_store.len() as u32;
    let check =
        |what: &str, list: &[crate::ir::metadata::AttachedMetadata]| -> Result<(), String> {
            for m in list {
                if m.node.0 >= store_len {
                    return Err(format!(
                        "{what} 的 metadata 附件指向越界节点 {}（节点数 {store_len}）",
                        m.node.0
                    ));
                }
            }
            Ok(())
        };
    for f in module.iter_functions() {
        check(&format!("函数 {}", f.name), f.metadata())?;
        for (inst, data) in f.dfg.all_insts() {
            check(&format!("函数 {} 的指令 {inst}", f.name), data.metadata())?;
        }
    }
    for (_, g) in module.iter_globals() {
        check(&format!("全局 {}", g.name), g.metadata())?;
    }
    for a in module.iter_global_aliases() {
        check(&format!("别名 {}", a.name), a.metadata())?;
    }
    Ok(())
}
