//! v12 ISA 模型 — 唯一 DSL 模型（严格 TOML，无 v11 字符串语法层）。
//!
//! 设计原则：
//! - 一切用户语法都是 TOML 数据：命名位域（SLEIGH 风格）、编码形式（语义键组合）、
//!   结构化谓词（`when`，迭代 4 定型）。仅 `asm`/`lowering.insts`/`emit.insts`
//!   保留模板字符串（带 `{0}`/`{name}` 占位符，语法在文档中定义）。
//! - `deny_unknown_fields`：未知键/表一律报错（严格 TOML；v11 文件必然无法解析）。
//! - 位域位置从 `[conventions.bitfields]` 推导，指令不再写位偏移；
//!   ModRM/REX/VEX 等 x86 机制降级为 form 的**语义键**（`modrm = "rr"` 等），
//!   实现为 forge-dsl 内部函数，语义键集合在迭代 3/4 定型。

use super::shared::group_names;
use quote::quote;
use serde::{Deserialize, Serialize, de::Visitor};
use std::{
    collections::BTreeMap,
    fmt::{self, Display},
    str::FromStr,
};

/// v12 顶层模型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V12Model {
    pub meta: Meta,
    /// 多文件组合（`include = [...]`，v18 S7d，见 `forge_isa_dsl::loader`）。
    ///
    /// **由 loader 消费**：合并后的文本里不再出现该键（所以正常路径下这里恒为空）。
    /// 在模型里登记它，是为了让 JSON Schema / 编辑器补全 / 文档键表认识这个键；
    /// 若有人绕过 loader 直接把**裸文本**交给解析器，校验会明确报错（见
    /// `validate_all`），不会静默忽略。
    #[serde(default)]
    pub include: Vec<String>,
    /// 显式覆盖（`[[override]]`，v18 S7d；`override` 是 Rust 保留字 → 生字段名 `r#override`）。
    /// 同上：由 loader 消费，正常路径恒为空。
    #[serde(default)]
    pub r#override: Vec<OverrideDef>,
    /// 指令编码宽度三态（`[encoding]`，v18 S4）。**可省**：省略 = 缺省
    /// `kind = "fixed"` 且**不给 bits**（尚未声明字长的骨架文档），真正需要字长的
    /// 生成期会在 `inst_bytes()` 处明确报错——省略整段不会被静默当成定宽 32。
    #[serde(default)]
    pub encoding: Encoding,
    /// 寄存器组（`[reg.NAME]`）。
    pub reg: BTreeMap<RegClass, RegGroup>,
    /// ISA 约定（位域/ModRM/REX/操作数宽度前缀）。
    #[serde(default)]
    pub conventions: Conventions,
    /// 类型 → 寄存器类的**显式映射**（`[types]`，可选；B2 接口通用化）。
    ///
    /// 键 = 类型名（`bool`/`i8`/`i16`/`i32`/`i64`/`i128`/`f16`/`f32`/`f64`/
    /// `f128`/`ptr`/`v64`/`v128`/`v256`），值 = 已声明 `[reg.*]` 组名，或
    /// `"unsupported"`（显式拒绝该类型）。显式条目**优先于**通用值池规则，
    /// 使"非常规映射"成为 ISA 数据而不是宿主代码：
    /// 软浮点（`f64 = "gpr8"`）、1 字节地址（`ptr = "gpr1"`）等。
    /// 未列出的类型走通用规则（族 + 宽度 ≤ 值池/寄存器文件存在性）。
    #[serde(default)]
    pub types: Option<BTreeMap<String, String>>,
    /// 栈与对齐（`[stack]`，可选）：槽单位/栈对齐/帧指针保存槽。
    /// 取代散落在 `[meta]`/`[abi]` 的同类键（2026-09-13 归并）。
    #[serde(default)]
    pub stack: Option<StackSection>,
    /// 操作数槽（`[[operand_slots]]`）。
    pub operand_slots: Vec<OperandSlot>,
    /// 编码形式（`[[forms]]`）。
    #[serde(default)]
    pub forms: Vec<Form>,
    /// 指令（`[[instructions]]`）。
    #[serde(default)]
    pub instructions: Vec<Instruction>,

    /// 参数化指令模板（`[[templates]]`，v18 S2）：解析期展开为指令 + 别名。
    #[serde(default)]
    pub templates: Vec<Template>,

    /// 重定位表（`[[reloc]]`，v18 S3d）：指令用 `reloc = "<name>"` 引用。
    #[serde(default)]
    pub reloc: Vec<RelocDef>,

    /// 派生谓词属性（`[[derive]]`，v18 S3f）：给 `when` 用的命名布尔属性。
    #[serde(default)]
    pub derive: Vec<DeriveDef>,

    /// 展开后的派生谓词（解析期由 [`V12Model::expand_derives`] 填；不进 TOML）。
    #[serde(skip, default)]
    pub derived_preds: BTreeMap<String, super::pred::Pred>,

    /// 汇编器伪指令（`[[pseudo]]`，v18 S3e）：汇编期的文本级多指令展开。
    #[serde(default)]
    pub pseudo: Vec<PseudoDef>,

    /// 指令选择规则（`[[lowering]]`）。
    #[serde(default)]
    pub lowering: Vec<Lowering>,
    /// 树型多指令匹配（`[[pattern]]`，S6，opt-in）：一根 IR 值被匹配进整棵
    /// 子树后，整块子树合并成一段发射序列。空 = 无此能力（零开销）。
    #[serde(default)]
    pub pattern: Vec<Pattern>,
    /// 调用约定（`[abi]`，为 YMM by-ref 铺路）。
    #[serde(default)]
    pub abi: Option<Abi>,
    /// 序言/尾声（`[emit]`）。
    #[serde(default)]
    pub emit: Option<EmitSection>,
    /// 溢出模板（`[spill.GPR]`/`[spill.FPR]`：load/store 指令 + 基址寄存器）。
    #[serde(default)]
    pub spill: BTreeMap<String, SpillTemplate>,
}

// ─────────────────── 宽度/类派生（元数据单点，禁止代码内写死）───────────────────
//
// 设计契约（2026-09-12 去「宽度写死」）：寄存器类、宽度、栈槽单位一律从
// `[meta]`/`[reg.*]`/`[abi.*]` 派生；**缺组即 Err**（fail-closed），绝不静默
// 回退到 `GPR(8)`/`GPR(4)`/`"RAX"`/`"RBP"` 这类 x86 缺省——1 字节寄存器的
// ISA 曾因这些回退而"生成成功但语义错误"。

impl V12Model {
    /// 校验显式宽度键指向的组是否存在。
    fn require_group(&self, rc: RegClass, key: &str) -> Result<RegClass, String> {
        if self.reg.contains_key(&rc) {
            Ok(rc)
        } else {
            Err(format!(
                "{key} = {} 没有对应的 [reg.{rc}] 寄存器组（显式宽度键必须指向已声明组）",
                rc.width()
            ))
        }
    }

    /// 主 GPR 类：`[meta].default_gpr_width` > 已声明 GPR 组中最宽者 > Err。
    /// 主 GPR 类是 GPR 名字/索引解析的**唯一锚点**（取代历史 `GPR(8).or(GPR(4))`）。
    pub(crate) fn main_gpr_class(&self) -> Result<RegClass, String> {
        if let Some(w) = self.meta.default_gpr_width {
            return self.require_group(RegClass::GPR(w), "[meta].default_gpr_width");
        }
        self.reg
            .keys()
            .filter(|rc| matches!(rc, RegClass::GPR(_)))
            .max_by_key(|rc| rc.width())
            .copied()
            .ok_or_else(|| {
                "[reg.*]/[meta]: 未声明任何 GPR 组（如 [reg.gpr8]）——\
                 整数/地址寄存器组是必需的"
                    .to_string()
            })
    }

    /// 主 FPR 类：`[meta].default_fpr_width` > `fpr16`（XMM 基准，历史规则）>
    /// 最宽 FPR 组 > None（无浮点组）。
    pub(crate) fn main_fpr_class(&self) -> Result<Option<RegClass>, String> {
        if let Some(w) = self.meta.default_fpr_width {
            return Ok(Some(
                self.require_group(RegClass::FPR(w), "[meta].default_fpr_width")?,
            ));
        }
        // 16 字节组优先于"更宽"（ZMM 32 字节组不得改变 SSE/ABI 占位基准）。
        Ok(self
            .reg
            .keys()
            .filter(|rc| matches!(rc, RegClass::FPR(_)))
            .max_by_key(|rc| {
                if rc.width() == 16 {
                    u16::MAX
                } else {
                    rc.width()
                }
            })
            .copied())
    }

    /// 地址/指针类：`[meta].addr_width` > 主 GPR 类。
    /// 用于 MemRef base/index、`lea`、sp/fp、帧地址计算与地址类槽。
    pub(crate) fn addr_class(&self) -> Result<RegClass, String> {
        if let Some(w) = self.meta.addr_width {
            return self.require_group(RegClass::GPR(w), "[meta].addr_width");
        }
        self.main_gpr_class()
    }

    /// 宿主「整数值寄存器池」类：`[meta].value_gpr_width` > 主 GPR 类。
    /// x86 = GPR(8)（历史值池宽，行为不变）。
    pub(crate) fn value_gpr_class(&self) -> Result<RegClass, String> {
        if let Some(w) = self.meta.value_gpr_width {
            return self.require_group(RegClass::GPR(w), "[meta].value_gpr_width");
        }
        self.main_gpr_class()
    }

    /// 宿主「浮点值寄存器池」类：`[meta].value_fpr_width` > `FPR(8)`
    /// （历史 `FPR64` 语义：f64 值池宽）> None（未声明该宽度组）。
    /// 注意：**不得**按"最宽 FPR 组"推导（x86 最宽是 ZMM，且 `fpr8` 是 MMX）。
    pub(crate) fn value_fpr_class(&self) -> Result<Option<RegClass>, String> {
        if let Some(w) = self.meta.value_fpr_width {
            return Ok(Some(
                self.require_group(RegClass::FPR(w), "[meta].value_fpr_width")?,
            ));
        }
        Ok(Some(RegClass::FPR(8)).filter(|rc| self.reg.contains_key(rc)))
    }

    /// ABI 栈槽单位（字节）：`[stack].slot` > 地址类宽度。
    pub(crate) fn slot_bytes(&self) -> Result<u16, String> {
        match self.stack.as_ref().and_then(|s| s.slot) {
            Some(b) => Ok(b),
            None => Ok(self.addr_class()?.width()),
        }
    }

    /// 栈对齐（字节）：`[stack].align` > 槽单位。
    pub(crate) fn stack_align(&self) -> Result<u32, String> {
        match self.stack.as_ref().and_then(|s| s.align) {
            Some(a) => Ok(a),
            None => Ok(self.slot_bytes()? as u32),
        }
    }

    /// 帧指针保存槽字节数：`[stack].fp_save` > 地址类宽度
    /// （x86/riscv64/arm64/demo 均为 8，与历史常量 `frame_pointer_overhead() = 8` 一致）。
    pub(crate) fn fp_overhead_bytes(&self) -> Result<u16, String> {
        match self.stack.as_ref().and_then(|s| s.fp_save) {
            Some(b) => Ok(b),
            None => Ok(self.addr_class()?.width()),
        }
    }

    /// **定宽**指令字长（字节）：`ceil([encoding].bits / 8)`。
    ///
    /// **无宽度白名单、无上限**：任何 ≥ 1 位的字宽都成立（含非 8 倍数位宽，如
    /// 12 位 = 2 字节；尾部填充位必须为 0——生成代码的"补集零 guard"保证）。
    /// 字宽是 ISA 数据：encode/decode 的读写宽度与定宽 label/global fixup 的
    /// `RelocKind` 宽度都由它派生（历史实现把字面量 4 写死在
    /// `codegen/mod.rs`/`machine.rs`）。
    ///
    /// 生成代码把指令字表示为**字节数组**（`[u8; ceil(位/8)]`，LE 位序），所以
    /// 字长不受任何机器字限制。唯一的天然边界是**单个位域 ≤ 64 位**：位域值承载
    /// 在 `Inst` 的 `i64` 操作数与 u64 常量键上，> 64 位的单域需要更宽的值表示
    /// （与字长无关——一个字可以有很多 ≤64 位的域）。
    ///
    /// 仅 `kind = "fixed"` 可用（mixed/prefix_scan 没有单一字长）。
    pub(crate) fn inst_bytes(&self) -> Result<u32, String> {
        match self.encoding.kind {
            EncodingKind::Fixed => {}
            EncodingKind::Mixed => {
                return Err(
                    "[encoding]: kind = \"mixed\" 的 ISA 没有单一指令字长（按 `width` 逐指令取）"
                        .into(),
                );
            }
            EncodingKind::PrefixScan => {
                return Err(
                    "[encoding]: kind = \"prefix_scan\" 的 ISA 没有固定指令字长（inst_bytes 不可用）"
                        .into(),
                );
            }
        }
        let bits = self.encoding.bits.ok_or_else(|| {
            "[encoding].bits 缺失：kind = \"fixed\" 必须声明指令字宽（位；8/16/32/64/任意）"
                .to_string()
        })?;
        if bits == 0 {
            return Err("[encoding].bits must be > 0".into());
        }
        Ok(bits.div_ceil(8))
    }

    /// 变长（mixed / prefix_scan）？
    pub(crate) fn is_variable_length(&self) -> bool {
        !matches!(self.encoding.kind, EncodingKind::Fixed)
    }

    /// 前缀扫描式变长（x86 风格：长度由前缀链决定）？
    ///
    /// 只有这种编码走 `codegen/vlen.rs` 的定长切片译码路径；`mixed`
    /// （按字长分组编码）与 `fixed` 共用定长位域编解码。
    pub(crate) fn is_prefix_scan(&self) -> bool {
        matches!(self.encoding.kind, EncodingKind::PrefixScan)
    }

    /// 该指令的字长（字节）：`width`（位，逐指令）> `[encoding].bits`（缺省字长）。
    ///
    /// `kind = "fixed"` 时 `width` 允许显式重复 `bits`（便于单指令自解释），
    /// 但不允许与 `bits` 不一致。
    pub(crate) fn inst_width_bytes(&self, inst: &Instruction) -> Result<u32, String> {
        let bits = match inst.width.or(self.encoding.bits) {
            Some(b) => b,
            None => {
                return Err(format!(
                    "[[instructions.{}]]: 缺少指令字长——`kind = \"mixed\"` 时必须给 `width`\
                     （或给 [encoding].bits 作缺省字长）",
                    inst.name
                ));
            }
        };
        if bits == 0 {
            return Err(format!("[[instructions.{}]]: width must be > 0", inst.name));
        }
        Ok(bits.div_ceil(8))
    }

    /// 全部允许的指令字长（字节，升序去重）——`fixed` 一个、`mixed` 若干、`prefix_scan` 空。
    pub(crate) fn encoding_width_bytes(&self) -> Vec<u32> {
        let mut out: Vec<u32> = match self.encoding.kind {
            EncodingKind::Fixed => self.encoding.bits.into_iter().collect(),
            EncodingKind::Mixed => self.encoding.widths.clone(),
            EncodingKind::PrefixScan => Vec::new(),
        }
        .into_iter()
        .map(|b| b.div_ceil(8))
        .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// 向量类字节档位（升序）：`[meta].vector_tiers` > `[16, 32, 64]`
    /// （x86 XMM/YMM/ZMM 语义；缺省值即历史 `reg_class_for` 的三档硬编码）。
    pub(crate) fn vector_tiers(&self) -> Vec<u16> {
        self.meta
            .vector_tiers
            .clone()
            .unwrap_or_else(|| vec![16, 32, 64])
    }

    /// `[types]` 的显式条目 → `(类型名, 目标)`；`None` = 该表未声明。
    /// 目标：`Ok(Some(rc))` = 映射到类、`Ok(None)` = 显式 `"unsupported"`。
    pub(crate) fn explicit_type_map(&self) -> Result<Vec<(String, Option<RegClass>)>, String> {
        let Some(map) = &self.types else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(map.len());
        for (ty, target) in map {
            if type_id_ident(ty).is_none() {
                return Err(format!(
                    "[types].{ty}: 未知类型名（可用：bool/i8/i16/i32/i64/i128/f16/f32/f64/f128/\
                     ptr/v64/v128/v256/void）"
                ));
            }
            let t = target.trim();
            if t.eq_ignore_ascii_case("unsupported") {
                out.push((ty.clone(), None));
                continue;
            }
            let rc: RegClass = t
                .parse()
                .map_err(|_| format!("[types].{ty}: 无法解析寄存器类 \"{target}\""))?;
            if !self.reg.contains_key(&rc) {
                return Err(format!(
                    "[types].{ty} = \"{target}\": 该寄存器组未声明（必须指向已声明 [reg.*]）"
                ));
            }
            out.push((ty.clone(), Some(rc)));
        }
        Ok(out)
    }

    /// 向量 by-value 阈值（**字节**）：`[abi.arg_class]` 中
    /// `strategy = "by-ref"` 的 `limit`（位）/ 8。`None` = 本 ISA 未声明
    /// by-ref 策略（调用方自定缺省，如按值上限 = 浮点值池宽）。
    pub(crate) fn vector_by_ref_limit_bytes(&self) -> Result<Option<u16>, String> {
        let Some(abi) = &self.abi else {
            return Ok(None);
        };
        let mut found: Option<(String, u16)> = None;
        for ac in &abi.arg_class {
            let name = ac.class.name();
            match (ac.strategy, ac.limit) {
                // by-ref 与 limit 必须**成对**：只写一个都是配置错误（历史实现
                // 静默忽略 limit ⇒ 宽向量按值传参会静默截断/错 ABI）。
                (Some(ArgStrategy::ByRef), None) => {
                    return Err(format!(
                        "[abi.arg_class.{name}]: strategy = \"by-ref\" 必须同时声明 `limit`（位）"
                    ));
                }
                (Some(ArgStrategy::ByRef), Some(bits)) => {
                    if bits == 0 || bits % 8 != 0 {
                        return Err(format!(
                            "[abi.arg_class.{name}]: by-ref limit {bits} 必须是 8 的倍数（单位：位）"
                        ));
                    }
                    let bytes = (bits / 8) as u16;
                    if let Some((prev, prev_bytes)) = &found
                        && *prev_bytes != bytes
                    {
                        return Err(format!(
                            "[abi.arg_class]: 多个 by-ref 类的阈值冲突（{prev} = {prev_bytes} 字节 vs \
                             {name} = {bytes} 字节）——阈值必须唯一，否则 ABI 依声明序而变"
                        ));
                    }
                    found.get_or_insert((name.to_string(), bytes));
                }
                // 无 by-ref 策略却写了 limit：按值类没有阈值语义。
                (None, Some(bits)) => {
                    return Err(format!(
                        "[abi.arg_class.{name}]: 声明了 `limit = {bits}` 但 strategy 不是 \
                         \"by-ref\"——按值传参的类没有阈值语义（写 strategy = \"by-ref\" 或删掉 limit）"
                    ));
                }
                (None, None) => {}
            }
        }
        Ok(found.map(|(_, b)| b))
    }

    /// 指定组的寄存器名列表（缺组 → Err）。
    pub(crate) fn names_of(&self, rc: RegClass) -> Result<Vec<String>, String> {
        match self.reg.get(&rc) {
            Some(g) => group_names(g),
            None => Err(format!("[reg.{rc}] 未声明（引用该寄存器类但没有对应组）")),
        }
    }

    /// 指定组的 `名字 → 物理索引` 表（base_index + 组内序号）。
    /// 取代历史在 `integration.rs` 内重复两遍、且锚定 `GPR(8)/GPR(4)` 的映射。
    pub(crate) fn name_to_idx(
        &self,
        rc: RegClass,
    ) -> Result<std::collections::HashMap<String, u32>, String> {
        let base = self.reg.get(&rc).and_then(|g| g.base_index).unwrap_or(0);
        Ok(self
            .names_of(rc)?
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, i as u32 + base))
            .collect())
    }

    /// 主 GPR 类的 `名字 → 索引` 表（sp/fp/scratch/callee_saved/物理 clobber 解析用）。
    pub(crate) fn main_gpr_name_to_idx(
        &self,
    ) -> Result<std::collections::HashMap<String, u32>, String> {
        self.name_to_idx(self.main_gpr_class()?)
    }
}

// ─────────────────────────── [meta] ───────────────────────────

/// ISA 元信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    /// ISA 名（派生生成模块名：小写 + `-`→`_`）。
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default = "default_endian")]
    pub endian: Endian,
    /// 默认模式（x86 64 位模式 = 64）。
    #[serde(default = "default_mode")]
    pub mode: u8,
    /// 控制寄存器解析是否大小写不敏感。
    #[serde(default)]
    pub case_insensitive_regs: Option<bool>,
    /// 行注释起始字符（缺省 "#"）。
    #[serde(default = "default_comment_char")]
    pub comment_char: String,
    /// 标签定义后缀（缺省 ":"；如 "foo:"）。
    #[serde(default = "default_label_suffix")]
    pub label_suffix: String,
    /// 助记符大小写策略（缺省 insensitive）。
    #[serde(default)]
    pub mnemonic_case: MnemonicCase,
    /// 立即数前缀（缺省无；x86 AT&T 可设 "$"，ARM 可设 "#"）。
    #[serde(default)]
    pub imm_prefix: Option<String>,
    /// 伪指令前缀（缺省 "."）。
    #[serde(default = "default_directive_prefix")]
    pub directive_prefix: String,
    /// 主 GPR 类宽度（**字节**）。缺省 = 已声明 GPR 组中最宽者（x86 = 8）。
    /// 主 GPR 类是：名字/索引解析锚点、`__DEFAULT_GPR_CLASS`、寄存器槽类。
    #[serde(default)]
    pub default_gpr_width: Option<u16>,
    /// 主 FPR 类宽度（**字节**）。缺省 = `fpr16`（XMM 基准）优先，其次最宽已
    /// 声明 FPR 组；无 FPR 组 = None。16 字节组刻意优先于"最宽"——否则 x86 的
    /// ZMM 组会把 SSE/ABI 占位基准带偏（见 `main_fpr_class`）。
    #[serde(default)]
    pub default_fpr_width: Option<u16>,
    /// 地址/指针类宽度（**字节**）。缺省 = `default_gpr_width`。
    /// 用于 MemRef base/index、`lea`、sp/fp、帧地址计算。
    #[serde(default)]
    pub addr_width: Option<u16>,
    /// 宿主「整数值寄存器池」宽度（**字节**）。缺省 = `default_gpr_width`
    /// （x86 = 8 → 与历史 `GPR64` 值池一致）。
    #[serde(default)]
    pub value_gpr_width: Option<u16>,
    /// 宿主「浮点值寄存器池」宽度（**字节**）。缺省 = 8（历史 `FPR64` 语义：
    /// f64 值池宽）。**不能**按"最宽 FPR 组"推导——x86 的 `[reg.fpr8]` 是
    /// MM0-7、最宽组是 `fpr32`(ZMM)，都不是浮点标量值池。
    #[serde(default)]
    pub value_fpr_width: Option<u16>,
    /// 向量类字节档位（升序；`TargetRegInfo::vector_tiers`）。
    /// 缺省 = `[16, 32, 64]`（x86 XMM/YMM/ZMM 语义）。向量类型按字节数夹到
    /// "最小的 ≥ 请求值的档位"，超过最大档 → 生成/编译期 `Unsupported`。
    #[serde(default)]
    pub vector_tiers: Option<Vec<u16>>,
}

/// `[stack]` — 栈与对齐（2026-09-13 从 `[meta]`/`[abi]` 归并而来）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct StackSection {
    /// ABI 栈槽单位（**字节**）。缺省 = 地址类宽度。
    /// alloca/聚合拆分/传参栈槽/spill 槽对齐与步长都用它。
    #[serde(default)]
    pub slot: Option<u16>,
    /// 栈对齐（**字节**）。缺省 = `slot`。
    #[serde(default)]
    pub align: Option<u32>,
    /// 帧指针保存槽字节数（`RegInfo::frame_pointer_overhead`）。
    /// 缺省 = 地址类宽度（x86/riscv64/arm64/demo = 8）。
    #[serde(default)]
    pub fp_save: Option<u16>,
}

fn default_comment_char() -> String {
    "#".into()
}

fn default_label_suffix() -> String {
    ":".into()
}

fn default_directive_prefix() -> String {
    ".".into()
}

/// 助记符大小写策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MnemonicCase {
    /// 大小写不敏感（缺省；assemble 时统一小写匹配）。
    #[default]
    Insensitive,
    /// 大小写敏感（精确匹配模板首词）。
    Sensitive,
}

fn default_endian() -> Endian {
    Endian::Little
}

fn default_mode() -> u8 {
    64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Endian {
    Little,
    Big,
}

// ─────────────────────────── [reg.*] ───────────────────────────
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RegClass {
    /// 通用整数寄存器，payload = 字节宽度（任意 ISA 自定义宽度）。
    GPR(u16),
    /// 浮点寄存器，payload = 字节宽度。
    FPR(u16),
    /// 向量寄存器，payload = 字节宽度。
    VEC(u16),
    /// 掩码寄存器（如 x86 AVX-512 k0-k7），payload = 字节宽度。
    KReg(u16),
}

/// `[types]` 允许的类型名（与 forge-ir `TypeId` 常量一一对应）。
pub(crate) fn type_id_ident(s: &str) -> Option<&'static str> {
    Some(match s {
        "void" => "VOID",
        "bool" => "BOOL",
        "i8" => "I8",
        "i16" => "I16",
        "i32" => "I32",
        "i64" => "I64",
        "i128" => "I128",
        "f16" => "F16",
        "f32" => "F32",
        "f64" => "F64",
        "f128" => "F128",
        "ptr" => "PTR",
        "v64" => "V64",
        "v128" => "V128",
        "v256" => "V256",
        _ => return None,
    })
}

/// 类型名的字节宽（`[types]` 映射的健全性校验用）。`ptr` 由调用方按 ISA
/// 地址宽判定（传 `None` 表示"按地址宽，跳过比较"）；`void`/`bool` 记 1。
pub(crate) fn type_name_bytes(s: &str) -> Option<u16> {
    Some(match s {
        "void" | "bool" | "i8" => 1,
        "f16" | "i16" => 2,
        "i32" | "f32" => 4,
        "i64" | "f64" => 8,
        "i128" | "f128" => 16,
        "v64" => 8,
        "v128" => 16,
        "v256" => 32,
        "ptr" => return None, // 由 ISA 地址宽决定
        _ => return None,
    })
}

impl RegClass {
    pub fn width(&self) -> u16 {
        match self {
            RegClass::GPR(w) | RegClass::FPR(w) | RegClass::VEC(w) | RegClass::KReg(w) => *w,
        }
    }
}

impl FromStr for RegClass {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(width) = s.strip_prefix("gpr") {
            if width.is_empty() {
                return Ok(RegClass::GPR(4));
            }
            let width: u16 = width
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            Ok(RegClass::GPR(width))
        } else if let Some(width) = s.strip_prefix("fpr") {
            if width.is_empty() {
                return Ok(RegClass::FPR(4));
            }
            let width: u16 = width
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            Ok(RegClass::FPR(width))
        } else if let Some(width) = s.strip_prefix("vec") {
            let width: u16 = width
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            Ok(RegClass::VEC(width))
        } else if let Some(width) = s.strip_prefix("kreg") {
            if width.is_empty() {
                return Ok(RegClass::KReg(8));
            }
            let width: u16 = width
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            Ok(RegClass::KReg(width))
        } else {
            Err("invalid reg class".to_string())
        }
    }
}

impl fmt::Display for RegClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegClass::GPR(width) => write!(f, "gpr{}", width),
            RegClass::FPR(width) => write!(f, "fpr{}", width),
            RegClass::VEC(width) => write!(f, "vec{}", width),
            RegClass::KReg(width) => write!(f, "kreg{}", width),
        }
    }
}

impl Serialize for RegClass {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

struct RegClassVisitor;

impl<'de> Visitor<'de> for RegClassVisitor {
    type Value = RegClass;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string like 'gpr2', 'fpr4', 'vec8', or 'kreg8'")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        // We use our FromStr implementation to parse the string.
        RegClass::from_str(value)
            .map_err(|_| E::custom(format!("invalid format for RegClass: '{}'", value)))
    }
}

impl<'de> Deserialize<'de> for RegClass {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(RegClassVisitor)
    }
}

impl quote::ToTokens for RegClass {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        let class = match self {
            RegClass::GPR(w) => quote! { forge_ir::RegClass::GPR(#w)},
            RegClass::FPR(w) => quote! { forge_ir::RegClass::FPR(#w)},
            RegClass::VEC(w) => quote! { forge_ir::RegClass::VEC(#w)},
            RegClass::KReg(w) => quote! { forge_ir::RegClass::KReg(#w)},
        };
        tokens.extend(class);
    }
}

/// 寄存器组。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegGroup {
    /// 显式寄存器名；缺省时由 `prefix` + 序号生成。
    #[serde(default)]
    pub names: Option<Vec<String>>,
    /// 生成名称的前缀（"R" → R0, R1, ...）。
    #[serde(default)]
    pub prefix: Option<String>,
    /// 首成员物理寄存器号偏移（x86 gpr8h 用 4）。
    #[serde(default)]
    pub base_index: Option<u32>,
    /// 组内寄存器数（仅 `names` 缺省时需要）。
    #[serde(default)]
    pub count: Option<u16>,
}

// ─────────────────────── [conventions.*] ───────────────────────

/// ISA 约定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Conventions {
    /// 命名位域（SLEIGH 风格），form/指令引用。
    #[serde(default)]
    pub bitfields: BTreeMap<String, Bitfield>,
    /// ModRM 结构约定（x86 家族）。
    #[serde(default)]
    pub modrm: Option<ModrmConvention>,
    /// 条件码表：**键 = 本 ISA 汇编/反汇编可见的条件名**（`e`/`z`/`b`/…），
    /// 每条给出编码与它实现的 IR 条件（见 [`CondEntry`]）。
    ///
    /// 三种用途共用这一张表（不需要第二处声明）：
    ///
    /// 1. 汇编：`cond` 槽解析这些名字；
    /// 2. 反汇编：编码 → 该编码**字母序最小**的名字（渲染用）；
    /// 3. lowering：`{cc}` 占位符 = 当前 IR 条件 → 本 ISA 编码，按 `ir` 字段查表。
    ///
    /// 缺省（未声明）= 该 ISA 不能用 `cond` 槽、也不能在 lowering 里用 `{cc}`
    /// （**不再回退 x86 的 16 项表**；真的用到就报错）。
    #[serde(default, deserialize_with = "de_cond_map")]
    pub cond: Option<BTreeMap<String, CondEntry>>,
    /// 变长解码前缀扫描表：条目 = 单字节或范围 + 效果集
    /// （"opsize16"/"lock"/"repe"/"repne"/"addr16"/"rex"）。缺省 = x86 扫描集。
    #[serde(default)]
    pub prefix_scan: Option<Vec<PrefixScanEntry>>,
    /// 内存操作数文本模板（v16）：`{base}`/`{index}`/`{scale}`/`{disp}` 占位符 +
    /// 字面标点。缺省 = `"[{base}+{index}*{scale}+{disp}]"`（x86 现行为）。
    #[serde(default)]
    pub mem: Option<MemTemplate>,
}

/// 内存操作数文本模板（v16）：组件占位符序列，同时派生汇编解析器与反汇编渲染器。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemTemplate {
    pub template: String,
}

/// IR 整数条件的规范名（与 `forge_ir::INTCC_NAMES` / `IntCC::mnemonic()` 一一对应）。
///
/// 这是**宿主契约**、不是 ISA 数据：`[conventions.cond]` 的 `ir` 字段用它作键空间。
/// forge-dsl 是 proc-macro crate（不依赖 forge-ir），故在此复述一份；两边的
/// 一致性由 x86 编译期校验 + 三架构 JIT 矩阵的比较用例端到端守着（若名字漂移，
/// 该 ISA 的 `{cc}` 会退化成 0 = 溢出条件，矩阵立刻红）。
pub const IR_INT_COND_NAMES: [&str; 10] = [
    "eq", "ne", "slt", "sle", "sgt", "sge", "ult", "ule", "ugt", "uge",
];

/// 条件码表的一条：本 ISA 的条件名 → 编码（+ 它实现哪个 IR 条件）。
///
/// 允许两种写法（同一张表，值的长短写法）：
///
/// ```toml
/// [conventions.cond]
/// eq  = 4                              # 简写 = { code = 4 }；`ir` 取键名
/// e   = { code = 4, ir = "eq" }        # 全写：汇编名 `e` 实现 IR 条件 `eq`
/// z   = { code = 4 }                   # 纯汇编别名（不映射 IR 条件）
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CondEntry {
    /// 本 ISA 的编码（进 `cond` 槽 / opcode 低 4 位）——**0..=15**（4 位字段）。
    pub code: u64,
    /// 该名对应的 IR 整数条件（[`IR_INT_COND_NAMES`] 之一）。
    ///
    /// 缺省 = 键名本身（键名恰是 IR 条件名时，如 `eq = 4`）；`ir = ""` 显式声明
    /// "纯汇编别名，不映射 IR 条件"。
    #[serde(default)]
    pub ir: Option<String>,
}

impl CondEntry {
    /// 该条目映射的 IR 条件名：显式 `ir`，或键名恰为规范名时的键名；纯别名 → `None`。
    pub fn ir_condition<'a>(&'a self, key: &'a str) -> Option<&'a str> {
        match self.ir.as_deref() {
            Some("") => None,
            Some(x) => Some(x),
            None if IR_INT_COND_NAMES.contains(&key) => Some(key),
            None => None,
        }
    }
}

/// `[conventions.cond]` 的值：整数简写或 [`CondEntry`]（`untagged`）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum CondEntryOrCode {
    Code(u64),
    Entry(CondEntry),
}

impl From<CondEntryOrCode> for CondEntry {
    fn from(v: CondEntryOrCode) -> Self {
        match v {
            CondEntryOrCode::Code(code) => CondEntry { code, ir: None },
            CondEntryOrCode::Entry(e) => e,
        }
    }
}

/// `[conventions.cond]` 的反序列化：整数值 = `{ code = n }` 简写。
fn de_cond_map<'de, D>(d: D) -> Result<Option<BTreeMap<String, CondEntry>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<BTreeMap<String, CondEntryOrCode>>::deserialize(d)?;
    Ok(raw.map(|m| m.into_iter().map(|(k, v)| (k, v.into())).collect()))
}

impl V12Model {
    /// `[conventions.cond]` 的"IR 条件 → 本 ISA 编码"有序表（按 [`IR_INT_COND_NAMES`]
    /// 的规范顺序，只含已映射的条件）。
    ///
    /// lowering 的 `{cc}` 占位符按这个顺序发 match 臂；校验器用"是否覆盖全部 10 个"
    /// 决定一个用到 `{cc}` 的 ISA 是否可以编译。
    pub fn cond_ir_codes(&self) -> Vec<(&'static str, u64)> {
        let Some(table) = self.conventions.cond.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for name in IR_INT_COND_NAMES {
            if let Some((_, e)) = table
                .iter()
                .find(|(k, e)| e.ir_condition(k.as_str()) == Some(name))
            {
                out.push((name, e.code));
            }
        }
        out
    }
}

/// 前缀扫描条目：`byte`（单字节）与 `range`（如 "0x40..0x4F"）二选一。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixScanEntry {
    #[serde(default)]
    pub byte: Option<u64>,
    /// "0x40..0x4F" / "0x40..=0x4F"（闭区间）。
    #[serde(default)]
    pub range: Option<String>,
    /// 效果："opsize16"（66 → opsize=2）、"lock"、"repe"、"repne"、
    /// "addr16"、"rex"（40-4F：REX.R/B/W 位）。
    pub effects: Vec<String>,
}

/// 命名位域：定宽 ISA 的编码单元。
///
/// 二选一（校验强制）：
/// - 单一位段：`{ offset, width }`；
/// - 散布位段：`{ pieces = [ { offset, width, shift }, ... ] }` —— 立即数分段
///   放置（S/U/B/J 型）。编码：`word |= ((value >> shift) & mask) << offset`；
///   解码：`value |= ((word >> offset) & mask) << shift`（可逆）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bitfield {
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub pieces: Option<Vec<BitfieldPiece>>,
}

/// 散布位段的一块：`value >> shift` 取 `width` 位，放置于 `offset`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BitfieldPiece {
    pub offset: u32,
    pub width: u32,
    /// 放置前的右移（缺省 0）。
    #[serde(default)]
    pub shift: u32,
}

/// ModRM 约定（表存在即启用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModrmConvention {
    /// 持有 ModRM.reg（3 位）的位域名（定宽 ISA）。
    #[serde(default)]
    pub reg_field: Option<String>,
    /// 持有 ModRM.rm（3 位，+REX.X 扩展第 4 位）的位域名（定宽 ISA）。
    #[serde(default)]
    pub rm_field: Option<String>,
    /// 必须强制带位移的 base 寄存器号（mod=00+rm=101 是 RIP-relative，
    /// 无法表达 [base]；x86 = [5, 13]）。
    #[serde(default)]
    pub force_disp_base: Vec<u8>,
}

// ──────────────────── [[operand_slots]] ────────────────────

/// 操作数槽：指令操作数的抽象类别。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperandSlot {
    pub name: String,
    pub kind: OperandKind,
    /// reg：所属 [reg.*] 组名（单类型糖，等价于 `classes = ["gpr8"]`）。
    #[serde(default)]
    pub class: Option<RegClass>,
    /// reg：可接纳的寄存器组集合（多宽度/多类型）。`class` 为单元素糖；
    /// 两者皆无 = 任意寄存器类（不推荐，会吞掉更具体的重载形式）。
    #[serde(default)]
    pub classes: Option<Vec<RegClass>>,
    /// reg：8 位寄存器操作数（spl/bpl/sil/dil 无 REX 时编码为 ah/ch/dh/bh，
    /// 索引 4-7 必须强制 REX 前缀）。
    #[serde(default)]
    pub byte_reg: Option<bool>,
    /// imm：值宽度（位）。
    #[serde(default)]
    pub width: Option<u32>,
    /// imm：有符号（缺省 false）。
    #[serde(default)]
    pub signed: Option<bool>,
    /// imm：允许浮点立即数（IEEE-754 位模式存储）。
    #[serde(default)]
    pub float: Option<bool>,
    /// imm：最小值约束（缺省 = 按 width/signed 推导）。
    #[serde(default)]
    pub min: Option<i64>,
    /// imm：最大值约束（缺省 = 按 width/signed 推导）。
    #[serde(default)]
    pub max: Option<i64>,
    /// 该槽可承担的角色；缺省 ["in"]。"inout" = 读改写（in 且 out）。
    #[serde(default)]
    pub roles: Option<Vec<OperandRole>>,
}

impl OperandSlot {
    /// 规范化寄存器约束集：`classes` 优先；否则 `class` → 单元素；两者皆无 → None
    /// （任意寄存器类）。
    pub fn classes(&self) -> Option<Vec<RegClass>> {
        if let Some(cs) = &self.classes {
            return Some(cs.clone());
        }
        self.class.map(|c| vec![c])
    }

    /// 立即数/标签槽的值域：(min, max)。缺省由 width/signed 推导。
    pub fn imm_range(&self) -> Option<(i64, i64)> {
        if self.kind != OperandKind::Imm && self.kind != OperandKind::Label {
            return None;
        }
        let w = self.width.unwrap_or(64);
        let (lo, hi) = if self.signed.unwrap_or(false) && w < 64 {
            let half = 1i64 << (w - 1);
            (-half, half - 1)
        } else if w >= 64 {
            (i64::MIN, i64::MAX)
        } else {
            (0, (1i64 << w) - 1)
        };
        Some((self.min.unwrap_or(lo), self.max.unwrap_or(hi)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperandKind {
    Reg,
    Imm,
    Mem,
    Label,
    Cond,
}

impl OperandKind {
    /// 人类可读名称（错误消息用）。
    pub fn kind_name(self) -> &'static str {
        match self {
            OperandKind::Reg => "reg",
            OperandKind::Imm => "imm",
            OperandKind::Mem => "mem",
            OperandKind::Label => "label",
            OperandKind::Cond => "cond",
        }
    }
}

/// 操作数角色。
///
/// - `in`：只读源
/// - `out`：只写目标
/// - `inout`：读改写（同时是源与目标，如 x86 `ADD RM, R` 的 RM、`XCHG`）——
///   减少模板里同一操作数重复声明 in/out 的冗余。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperandRole {
    In,
    Out,
    InOut,
}

// ──────────────────── [[forms]] / 编码语义键 ────────────────────

/// 编码语义键集合——`[[forms]]`（预设）与 `[[instructions]]`（逐键覆盖）
/// **共用同一组字段**。
///
/// v14 之前 form 是"必须预先命名的组合点"，指令只能覆盖 opsize/rex_w/vex 三个
/// 键，于是每出现一个新组合就得新起一个 form 名——x86 47 个 form 里 17 个只被
/// 一条指令用，命名已到 `MRR_0F_NOOS_MEM` 与 `MRR_MEM_0F_NOOS` 并存（只差
/// `rex_w`）的程度。v15 起 form 退化为**可选混入的预设**，任意键都能在指令上
/// 覆盖，组合不再需要命名。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncKeys {
    /// ModRM 结构映射（v15）：`{ reg = <操作数名 | 固定扩展码>, rm = <操作数名> }`；
    /// `rm` 加方括号（`"[base]"`）= 内存形式（mod≠11），与 asm 里 `[{base}]` 同形。
    ///
    /// 取代 v14 的六个魔法串（`rr`/`rr_rev`/`rr_src2`/`ext`/`rm_mem`/`rm_memref`）
    /// ——它们把"哪个操作数进 reg 字段、哪个进 rm 字段"编进了一个不透明的名字，
    /// 读者必须回查生成器才知道 `rr_src2` 是 reg=op0+rm=op2。现在直接写名字。
    /// 内存形式的两种风味由 `rm` 引用的槽 kind 区分：`mem` 槽 → 带
    /// base/disp/index/scale；`reg` 槽 → 仅 `[base]`（disp 恒 0）。
    #[serde(default)]
    pub modrm: Option<ModrmMap>,
    /// 固定 ModRM 字节（无操作数指令如 MFENCE 0F AE F0：mod=11/reg/rm 全固定）。
    #[serde(default)]
    pub modrm_fixed: Option<u64>,
    /// REX 发射策略（"auto" | "never" | ...）。
    #[serde(default)]
    pub rex: Option<String>,
    /// VEX 结构（map/pp/l 来源）。
    #[serde(default)]
    pub vex: Option<VexSpec>,
    /// EVEX 结构（AVX-512；复用 [`VexSpec`] 数据键 map/pp/w/l，l=0/1/2 →
    /// L'L=128/256/512 位）。
    #[serde(default)]
    pub evex: Option<VexSpec>,
    /// 变长：固定前缀来源。`"field"` → fields.prefix（SSE 的 66/F2/F3/0）；
    /// 数字字符串 → 固定字节。缺省无前缀。
    #[serde(default)]
    pub prefix: Option<String>,
    /// 变长：编码宽度语义（见 [`Opsize`]）。
    #[serde(default)]
    pub opsize: Option<Opsize>,
    /// 变长：REX.W 位来源（枚举——未知值由 serde 报错并列出候选）。
    #[serde(default)]
    pub rex_w: Option<RexW>,
    /// 变长：opcode 含寄存器低 3 位（`+r` 形式：50+r/push、58+r/pop、
    /// B8+r/mov_imm64、C8+r/bswap）。opcode 字节 = 基值 | (op0 & 7)；
    /// REX.B = op0>>3；无 ModRM。
    #[serde(default)]
    pub opcode_reg: Option<u64>,
    /// 变长：尾部立即数宽度（位；如 32）。指令最后操作数（imm 槽）编码于此。
    #[serde(default)]
    pub imm: Option<u32>,
    /// 强制 escape 字节（x86：[0x0F]）。
    #[serde(default)]
    pub escape: Option<Vec<u8>>,
    /// 定宽：主 opcode 所在位域名。
    #[serde(default)]
    pub opcode_field: Option<String>,
    /// 定宽：按操作数位置绑定位域名（第 i 个操作数 → bitfields[i]）。
    /// 操作数少于该列表时，多余位域取 `fields` 固定值或隐式 0。
    #[serde(default)]
    pub operand_fields: Option<Vec<String>>,
}

impl EncKeys {
    /// 逐键覆盖：`self`（指令级）优先，缺省取 `base`（form 预设）。
    pub fn over(&self, base: &EncKeys) -> EncKeys {
        macro_rules! pick {
            ($f:ident) => {
                self.$f.clone().or_else(|| base.$f.clone())
            };
        }
        EncKeys {
            modrm: pick!(modrm),
            modrm_fixed: pick!(modrm_fixed),
            rex: pick!(rex),
            vex: pick!(vex),
            evex: pick!(evex),
            prefix: pick!(prefix),
            opsize: pick!(opsize),
            rex_w: pick!(rex_w),
            opcode_reg: pick!(opcode_reg),
            imm: pick!(imm),
            escape: pick!(escape),
            opcode_field: pick!(opcode_field),
            operand_fields: pick!(operand_fields),
        }
    }
}

/// 编码形式预设（`[[forms]]`）：具名的 [`EncKeys`]，指令用 `form = "名字"` 混入。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Form {
    pub name: String,
    #[serde(flatten)]
    pub keys: EncKeys,
}

/// ModRM 映射：哪个操作数进 `reg` 字段、哪个进 `rm` 字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModrmMap {
    /// `reg` 字段来源：操作数名，或固定扩展码（`/digit` 形式的整数）。
    /// 缺省 = 首个操作数（`ops[0]`）——多数指令 reg 字段就是 op0（S3 约定
    /// "数组序 = 编码序"，reg 恒在 op0）。
    #[serde(default)]
    pub reg: Option<ModrmReg>,
    /// `rm` 字段来源：操作数名；`"[名字]"` = 内存形式（mod≠11）。
    /// 缺省 = 最后操作数（2 操作数→op1、3 操作数 VEX/EVEX→op2）。
    #[serde(default)]
    pub rm: Option<String>,
}

impl ModrmMap {
    /// `rm` 是否内存形式，以及去掉方括号后的操作数名；`None` = 缺省（最后操作数，
    /// 非内存形式）。
    pub fn rm_operand(&self) -> (bool, Option<&str>) {
        match &self.rm {
            None => (false, None),
            Some(t) => {
                let t = t.trim();
                match t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                    Some(inner) => (true, Some(inner.trim())),
                    None => (false, Some(t)),
                }
            }
        }
    }
}

/// `modrm.reg` 的两种来源。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModrmReg {
    /// 固定扩展码（x86 的 `/0`..`/7`）——无操作数占用 reg 字段。
    Ext(u64),
    /// 操作数名。
    Op(String),
}

/// REX.W 位来源（x86-64）。
///
/// v14 前是自由字符串 + 手写 validate 分支；改枚举后未知值由 serde 直接报
/// "unknown variant" 并列出候选，且带 TOML 行号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RexW {
    /// opsize == 64 位时置 1（多数算术/传送）。
    Auto,
    /// 取指令 `fields.w`（SSE/VEX 系：W 位是 opcode 的一部分）。
    Field,
    /// 恒置 1（`+r` 形式的 mov_imm64 / bswap）。
    Always,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Opsize {
    Slot(u16),
    Reg(u16),
    /// `opsize = "<操作数名>"`：取该命名操作数的寄存器宽度。
    ///
    /// 自解释形态，取代 `"s<N>"` 的位置引用——`opsize = "dst"` 一眼看出取的是
    /// 目的操作数。`collect_inst_infos` 里按 `ops` 声明序解析成 [`Opsize::Slot`]，
    /// 下游只见索引。
    Named(String),
    /// `opsize = "max"`：宽度 = 全部 Reg 操作数宽度的**最大值**。
    ///
    /// 用于两个源槽都不是"结果"的同宽指令（x86 CMP/TEST：`cmp r/m, r` 两操作数
    /// 必须同宽，没有目的槽可取）。IR 层允许混宽（`TypeId::upcast`：
    /// `icmp(PTR, I32)`），取任一单槽宽度都会按较窄者编码——32 位 CMP 只比低半，
    /// 高半非 0 的指针与 0 判等为真。取宽者 = upcast 结果宽度 = 正确比较宽度。
    ///
    /// 汇编文本路径不受影响：多类 GPR 槽的宽度一致性检查同样对 `max` 生效
    /// （`cmp RAX, EBX` 仍拒绝），只有 IR 降级产生的混宽组合走取宽语义。
    Max,
    /// `opsize = "out"`：取**唯一** `out`/`inout` 角色的 Reg 操作数宽度。
    ///
    /// 目的操作数是"这条指令作用于目的宽度"的自然缺省——取代 `"s0"` 的位置引用
    /// （`s0` 在 `*_RM_R` 家族恰好是**源**，得靠 5 条指令逐一覆盖 `"dst"`）。
    /// 0 个或多个 out 操作数 → 编译期报错（改 `"max"` 或显式操作数名）。
    /// `collect_inst_infos` 里按角色解析成 [`Opsize::Slot`]，下游只见索引。
    Out,
}

impl Default for Opsize {
    fn default() -> Self {
        Self::Slot(0)
    }
}

impl FromStr for Opsize {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "max" {
            return Ok(Self::Max);
        }
        if s == "out" {
            return Ok(Self::Out);
        }
        // "s<N>" / "r<N>" 是位置/字节形态；其余非空字符串 = 命名操作数引用
        let digits = s.get(1..).unwrap_or("");
        let n: u16 = match digits.parse() {
            Ok(n) if !digits.is_empty() => n,
            _ => {
                return if s.is_empty() {
                    Err("opsize must not be empty".to_string())
                } else {
                    Ok(Self::Named(s.to_string()))
                };
            }
        };
        match s.chars().next() {
            Some('s') => Ok(Self::Slot(n)),
            // "r<字节>" 是 v14 及以前的写法（r8 = 8 字节 = 64 位），与
            // `[conventions]` 的位单位互相矛盾。v15 起固定宽度写**裸整数位宽**
            // （`opsize = 64`），这里保留读旧值的能力只为给出明确的迁移提示。
            Some('r') => Err(format!(
                "opsize = \"r{n}\" 是字节单位的旧写法，改写成位宽整数 opsize = {}",
                n as u32 * 8
            )),
            _ => Ok(Self::Named(s.to_string())),
        }
    }
}

impl Opsize {
    /// 裸整数 → 固定宽度（**位**；内部按字节存，与 `PhysReg::width()` 同单位）。
    fn from_bits(bits: u64) -> Result<Self, String> {
        if bits == 0 || !bits.is_multiple_of(8) {
            return Err(format!("opsize = {bits} 必须是 8 的正整数倍（位宽）"));
        }
        Ok(Self::Reg((bits / 8) as u16))
    }
}

impl Display for Opsize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Opsize::Slot(idx) => write!(f, "s{}", idx),
            // 序列化回位宽整数形态（往返一致）
            Opsize::Reg(bytes) => write!(f, "{}", *bytes as u32 * 8),
            Opsize::Named(n) => write!(f, "{n}"),
            Opsize::Max => write!(f, "max"),
            Opsize::Out => write!(f, "out"),
        }
    }
}

impl Serialize for Opsize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // 固定宽度序列化为整数位宽（与 TOML 写法一致，往返无损）
        match self {
            Opsize::Reg(bytes) => serializer.serialize_u64(*bytes as u64 * 8),
            other => serializer.serialize_str(&other.to_string()),
        }
    }
}

struct OpsizeVisitor;

impl<'de> Visitor<'de> for OpsizeVisitor {
    type Value = Opsize;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            r#"an opsize: bit width integer (16/32/64), "s<N>" (operand slot), "out" or "max""#
        )
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Self::Value::from_str(v).map_err(|e| E::custom(format!("invalid opsize: {}", e)))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Opsize::from_bits(v).map_err(|e| E::custom(format!("invalid opsize: {e}")))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if v < 0 {
            return Err(E::custom("invalid opsize: 位宽不能为负"));
        }
        self.visit_u64(v as u64)
    }
}

impl<'de> Deserialize<'de> for Opsize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(OpsizeVisitor)
    }
}

/// VEX 字段来源声明（迭代 4）。
///
/// map/pp/w/l 每个值为：数字（固定）或 `"field"`（取指令 `fields.vex_map`/
/// `vex_pp`/`vex_w`/`vex_l`，缺省 0）。vvvv 语义由操作数数量决定：
/// 3 操作数（VEX_RRV 类）→ vvvv = ~op2；2 操作数 → vvvv = 0x0F（无源）。
/// EVEX 复用本结构（`form.evex`），额外键 b（broadcast）与 disp_scale
/// （压缩位移缩放 N）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VexSpec {
    /// VEX.mmmmm 来源（数字或 "field"）。
    #[serde(default)]
    pub map: Option<String>,
    /// VEX.pp 来源。
    #[serde(default)]
    pub pp: Option<String>,
    /// VEX.W 来源。
    #[serde(default)]
    pub w: Option<String>,
    /// VEX.L 来源（EVEX：L'L 值 0/1/2 → 128/256/512 位）。
    #[serde(default)]
    pub l: Option<String>,
    /// EVEX 专用：broadcast 位（"0"/"1" 或 "field" → fields.evex_b）。
    #[serde(default)]
    pub b: Option<String>,
    /// EVEX 专用：零掩码位 z（"0"/"1" 或 "field" → fields.evex_z；P2 bit7）。
    #[serde(default)]
    pub z: Option<String>,
    /// EVEX 专用：压缩位移缩放 N（"1"/"2"/"4"/"8"/"16"/"32"/"64"；
    /// "field" → fields.evex_disp_scale；缺省 1 = 不缩放）。
    #[serde(default)]
    pub disp_scale: Option<String>,
}

// ──────────────────── [[instructions]] ────────────────────

/// 单条指令。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    /// 内部 Rust 标识符（Inst 变体名；与汇编助记符解耦，可含语义后缀）。
    pub name: String,
    /// 编码预设名（`[[forms]]`）。**可省略**——省略时全部编码键由本指令的
    /// [`EncKeys`] 直接给出（组合不需要预先命名一个 form）。
    #[serde(default)]
    pub form: Option<String>,
    /// 主 opcode（值或首个 opcode 字节）。
    #[serde(default)]
    pub opcode: Option<u64>,
    /// 固定字段值（funct3/funct7/前缀字节...），按位域名引用。
    #[serde(default)]
    pub fields: Option<BTreeMap<String, u64>>,
    /// **命名操作数声明**（v15）：每项 `"名字:槽[:角色]"`，**数组序 = 编码序**
    /// （modrm reg/rm、定宽位域绑定都按这个序）。角色缺省 `in`。
    ///
    /// 声明与打印从此分离：`asm` 只用 `{名字}` **引用**操作数，不再内联声明。
    /// v14 的写法把两件事塞在一起——`asm = "add {1:[gprx:inout]}, {0:[gprx:in]}"`
    /// 里索引与打印序解耦，读者无法从 `opsize = "s0"` 看出 s0 是源还是目的
    /// （ADD_RM_R 里恰好是**源**，这正是 v14 那个 32 位截断指针 bug 的根源）。
    /// 命名之后写 `opsize = "dst"`，自解释。
    ///
    /// 省略 `ops` 时回退 v14 的内联声明形态（迁移期并存）。
    #[serde(default)]
    pub ops: Option<Vec<String>>,
    /// 汇编模板（完整格式）。**必填**（v17：回退自动派生，asm 就地显性编写）。
    /// 有 `ops` 时用 `{名字}` 引用；无 `ops` 时用 `{i:[槽:角色]}` 内联声明。
    pub asm: String,
    /// 结构化谓词（迭代 4 定型：{ and = [...], eq = [...] }）。
    #[serde(default)]
    pub when: Option<toml::Value>,
    /// **指令级编码键覆盖**（逐键压过 `form` 预设，见 [`EncKeys`]）。
    /// 组合不再需要预先命名一个 form——直接在指令上写差异那一两个键。
    #[serde(flatten)]
    pub enc: EncKeys,
    /// 效果标签（缺省空 = 无声明）。驱动 `MachineInst::effects` /
    /// `is_branch` / `is_call` / `is_ret` / `is_move`（TargetMachine 集成）。
    #[serde(default)]
    pub effect: Vec<Effect>,
    /// 语义角色（见 [`Role`]）——生成器按角色查指令，取代 `[abi]` 的 13 个
    /// `*_inst` 名指针与 v14 的 `tags` 字符串标签。每个角色全 ISA 唯一。
    #[serde(default)]
    pub roles: Vec<Role>,
    /// 隐式破坏的物理寄存器名（如 cqo 的 RDX、idiv 的 RAX/RDX）——regalloc
    /// 在本指令点避开（MachineInst::clobbers）。与 lowering 模板的显式物理
    /// 寄存器（collect_phys_clobbers）互补：这是指令自身的隐式写。
    #[serde(default)]
    pub implicit_regs: Option<Vec<String>>,
    /// **重定位引用名**（`reloc`，v18 S3d）：GlobalAddr lowering 专用指令声明它用哪个
    /// `[[reloc]]` 表项——生成器据此发 encoder reloc arm，不按指令名特判。
    /// 重定位的"语义"（absolute/pc_relative）与绑定的槽都在表里，指令只写名字。
    #[serde(default, rename = "reloc")]
    pub reloc: Option<String>,
    /// **指令字长**（位，v18 S4）：`kind = "mixed"` 时逐指令给；`kind = "fixed"` 时
    /// 可省略（取 `[encoding].bits`），显式给出时必须与之一致。
    #[serde(default)]
    pub width: Option<u32>,
    /// 展开来源（v18 S2）：由 `[[templates.NAME]]` 展开而来时记下模板名。
    ///
    /// 不参与序列化（`serde(skip)`）；诊断据此把错误锚回**模板声明行**而不是
    /// 指向 `[[instructions]]` 节头。
    #[serde(skip, default)]
    pub from_template: Option<Box<str>>,
    /// **引用名**（`ref`，v18 S2）：lowering/pattern/emit 模板行首可用它指向本指令；
    /// **多条指令共用一个 `ref`** 即"多态引用"（按操作数签名分派）——原先的
    /// `[[aliases]]` 就是"几条指令共用同一个 ref"。
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
}

/// `[encoding]` — 指令编码宽度**三态**（v18 S4，取代 `[meta].default_inst_width` /
/// `variable_length` / `max_inst_len` / `default_opsize`）。
///
/// ```toml
/// [encoding]
/// kind = "fixed"          # fixed | mixed | prefix_scan
/// bits = 32               # fixed：字长（位，任意 ≥ 1）
/// # mixed：widths = [16, 32]（允许的字长集；指令逐条给 width，缺省取 bits）
/// # prefix_scan：max_len = 15（最长字节数；前缀扫描表在 [conventions.prefix_scan]）
/// # default_opsize = 32   # 可选：无显式 opsize 的 form 在 decode 时的 __opsize 初值
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Encoding {
    /// 三态之一。
    pub kind: EncodingKind,
    /// `fixed`：指令字长（位）；`mixed`：可选缺省字长（指令 `width` 缺省时用它）。
    #[serde(default)]
    pub bits: Option<u32>,
    /// `mixed`：允许的字长集（位；非空、> 0；解码按升序尝试）。
    #[serde(default)]
    pub widths: Vec<u32>,
    /// `prefix_scan`：最长指令字节数（缺省 15）。
    #[serde(default)]
    pub max_len: Option<u8>,
    /// 缺省 opsize（位；如 32/64）。无显式 opsize 语义的 form 在 decode 时的
    /// `__opsize` 初始值（影响 REX.W/宽度 guard 的缺省判定）。缺省 None
    /// = 现状（decode 初始 4 = 32 位）。**位**为单位；生成代码里的 `__opsize`
    /// 是**字节**（= 本值 / 8）。
    #[serde(default)]
    pub default_opsize: Option<u8>,
}

/// 指令编码的三种形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncodingKind {
    /// 单一字长（riscv64/arm64 与各 demo 夹具）。
    Fixed,
    /// 混合字长（如 RVC/Thumb：16 与 32 位共存）——解码按 `widths` 升序尝试。
    Mixed,
    /// 变长 + 前缀扫描（x86：1..=15 字节，扫描表在 `[conventions.prefix_scan]`）。
    PrefixScan,
}

/// `[encoding]` 段的默认值：`kind = "fixed"` 但**不给 bits** ⇒ 定宽路径会在
/// `inst_bytes()` 处明确报错（"必须声明字宽"），因此省略整段不会被静默当成定宽 32。
impl Default for Encoding {
    fn default() -> Self {
        Self {
            kind: EncodingKind::Fixed,
            bits: None,
            widths: Vec::new(),
            max_len: None,
            default_opsize: None,
        }
    }
}

/// 指令的**语义角色**（v15-S4）。
///
/// 生成器需要"某个语义位置上的指令"时按角色查表。v14 是反过来的：`[abi]` 里
/// 13 个 `*_inst` 键存指令**名字**，生成器 `unwrap_or_else(|| "MOV_RM8_R64")`
/// 兜底——共 16 处 x86 指令名硬编码，于是 x86 靠默认"恰好能跑"、非 x86 必须逐个
/// 覆盖。角色化之后：指令自己声明担任什么角色，缺角色 → 明确的 `Unsupported`，
/// 不会静默去查一个别的 ISA 的名字。`tags`（宽向量 by-ref 那几个）一并并入。
///
/// 每个角色全 ISA 唯一（validate 强制）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// 整数寄存器移动（收参 / Copy / 溢出前搬运）。
    GprMov,
    /// 返回值 → 返回寄存器的移动。
    RetMov,
    /// f64 标量寄存器移动。
    FprMovF64,
    /// f32 标量寄存器移动。
    FprMovF32,
    /// ≤16B 向量按值的**全宽**寄存器移动（x86 MOVAPS；缺则向量 by-value 不支持）。
    VecMov,
    /// 直接调用（函数符号 reloc）。
    Call,
    /// 间接调用（寄存器/内存目标）。
    CallIndirect,
    /// 返回。
    Ret,
    /// 无条件跳转。
    Jump,
    /// 条件分支。
    Branch,
    /// 条件测试（Branch 的 test-cond 序列）。
    Test,
    /// 硬件 push（callee-saved 保存；缺则回退 `[spill.*]` store 模板）。
    Push,
    /// 硬件 pop。
    Pop,
    /// 帧分配（`@frame_alloc`）。
    FrameAlloc,
    /// 帧释放（`@frame_free`）。
    FrameFree,
    /// 尾声跳转（缺省用 `jump`；需要不同指令时单独声明）。
    EpilogueJump,
    /// 宽向量 by-ref：调用方栈拷贝 store（32 字节）。
    #[serde(rename = "wide_vec_store_32")]
    WideVecStore32,
    /// 宽向量 by-ref：调用方栈拷贝 store（64 字节）。
    #[serde(rename = "wide_vec_store_64")]
    WideVecStore64,
    /// 宽向量 by-ref/sret：收参与回读 load（32 字节）。
    #[serde(rename = "wide_vec_load_32")]
    WideVecLoad32,
    /// 宽向量 by-ref/sret：收参与回读 load（64 字节）。
    #[serde(rename = "wide_vec_load_64")]
    WideVecLoad64,
    /// 帧内 `[FP+disp]` 地址计算（sret / by-ref 临时槽）。
    FrameAddr,
    /// 栈参数收参 load（第 5+ 个参数从 `[FP+shadow+…]` 取）。
    StackArgLoad,
    /// 栈参数传参 store（调用方把第 5+ 个参数写到 `[SP+shadow+…]`）。
    StackArgStore,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 与 serde 的 snake_case 命名一致（错误消息里直接给 TOML 写法）
        let s = serde_json_name(self);
        f.write_str(s)
    }
}

/// 角色的 TOML 写法（snake_case）。
fn serde_json_name(r: &Role) -> &'static str {
    match r {
        Role::GprMov => "gpr_mov",
        Role::RetMov => "ret_mov",
        Role::FprMovF64 => "fpr_mov_f64",
        Role::FprMovF32 => "fpr_mov_f32",
        Role::VecMov => "vec_mov",
        Role::Call => "call",
        Role::CallIndirect => "call_indirect",
        Role::Ret => "ret",
        Role::Jump => "jump",
        Role::Branch => "branch",
        Role::Test => "test",
        Role::Push => "push",
        Role::Pop => "pop",
        Role::FrameAlloc => "frame_alloc",
        Role::FrameFree => "frame_free",
        Role::EpilogueJump => "epilogue_jump",
        Role::WideVecStore32 => "wide_vec_store_32",
        Role::WideVecStore64 => "wide_vec_store_64",
        Role::WideVecLoad32 => "wide_vec_load_32",
        Role::WideVecLoad64 => "wide_vec_load_64",
        Role::FrameAddr => "frame_addr",
        Role::StackArgLoad => "stack_arg_load",
        Role::StackArgStore => "stack_arg_store",
    }
}

/// 指令效果标签。
///
/// v14 前是自由字符串，生成器 `match e.as_str()` 的兜底分支把打错的标签静默
/// 变成 `EffectKind::Custom(0)`（既不报错也不生效）。改枚举后未知标签由 serde
/// 直接拒绝并列出候选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// 无副作用纯运算。
    Pure,
    /// 读内存。
    Read,
    /// 写内存。
    Write,
    /// 条件分支（+ Label 槽 → branch_targets）。
    Branch,
    /// 无条件跳转（+ Label 槽 → branch_targets）。
    Jump,
    /// 调用。
    Call,
    /// 返回。
    Ret,
    /// 陷入（ud2/ebreak——`Trap` lowering 取无操作数的那条）。
    Trap,
    /// 纯寄存器移动（regalloc 的 coalesce 依据；效果语义等同 `Pure`）。
    Move,
}

/// `[[pseudo]]` — 汇编器伪指令（v18 S3e）。
///
/// 汇编期的**文本级**多指令展开（`.equ`/`.macro` 的结构化兄弟）：不碰编码器、
/// 不碰 lowering——`parse_insts` 遇到以伪指令名开头的行，就按 `params` 位置切分实参、
/// 逐行代入 `emit` 模板，再让**同一套汇编器**装配展开出的每一行。
///
/// ```toml
/// [[pseudo]]
/// name = "li"                     # 汇编可见的助记符（不得与指令助记符重名）
/// params = ["rd", "imm"]          # 位置实参名（emit 里用 `{名字}` 引用）
/// emit = [
///   "lui {rd}, (({imm} + 0x800) >> 12)",
///   "addi {rd}, {rd}, ({imm} - ((({imm} + 0x800) >> 12) << 12))",
/// ]
/// ```
///
/// 规则：
///
/// - 实参按**顶层逗号**切分（`()`/`[]` 内的逗号不算，故 `li x1, (a + b)` 与
///   内存操作数都能写），个数必须与 `params` 一致；
/// - `emit` 行就是普通汇编文本，其中的**算术表达式由既有表达式求值器求值**
///   （`+ - * / % << >> & | ^ ~`、括号、`.equ` 符号）——本能力不引入新的表达式语言；
/// - 实参是**文本**：写 `{imm}` 做算术时请自己加括号（`({imm} + 0x800)`），否则会
///   撞上运算符优先级；
/// - `emit` 行可以是**别的伪指令**（递归展开，深度上限 16 防环）；
/// - 单指令 `assemble()` API 只接受**展开成 1 条指令**的伪指令；多条要用
///   `TargetAssembler::parse_insts`（整段汇编）。
///
/// 未实现（如实记录）：按谓词分派同名多条（汇编期没有 IR 属性可判）、
/// `pseudo_fold`（反汇编折叠回伪指令——那是指令级模式识别，属另一件事）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PseudoDef {
    /// 助记符（汇编可见；不得与任何指令助记符或别的伪指令重名）。
    pub name: String,
    /// 位置实参名（非空、唯一）。
    pub params: Vec<String>,
    /// 展开模板行（至少一行；`{参数}` 会被实参文本替换）。
    pub emit: Vec<String>,
}

/// `[[derive]]` — 派生谓词属性（v18 S3f）。
///
/// 给 `[[lowering]]`/`[[pattern]]` 的 `when` 用一个**有名字的布尔属性**，值 = 1/0：
///
/// ```toml
/// [[derive]]
/// name = "is_64"
/// expr = { eq = ["rs1_width", 64] }
///
/// [[lowering]]
/// op = "Iadd"
/// when = { eq = ["is_64", 1] }
/// ```
///
/// 规则：
///
/// - `expr` 就是普通的结构化谓词（与 `when` 同一套语法与校验）；
/// - 派生可以引用**别的派生**（解析期展开成基础属性上的表达式，故不是递归求值）；
/// - 名字不得与[核心属性](super::pred::PRED_ATTRS)重名（否则静默遮蔽）；
/// - 依赖属性缺失时该派生为**假**（与核心属性"未知即假"一致）。
///
/// 展开在解析期完成，下游（校验/生成）只看到基础属性上的谓词——生成器不改判定逻辑。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeriveDef {
    /// 属性名（`when` 里按此引用）。
    pub name: String,
    /// 谓词表达式（与 `[[lowering]].when` 同语法）。
    pub expr: toml::Value,
}

/// `[[reloc]]` — 重定位表项（v18 S3d，取代 `GlobalReloc` 枚举）。
///
/// 指令只写 `reloc = "<name>"` 引用本表；**语义是宿主契约**（有限、ISA 无关：
/// [`RelocSemantics`]），"这条指令把重定位值编进哪个槽"是本表的数据。
///
/// ```toml
/// [[reloc]]
/// name = "abs64"                 # 指令引用：reloc = "abs64"
/// semantics = "absolute"         # 宿主语义（absolute / pc_relative）
/// slot = "imm64"                 # 绑定到哪个操作数槽（fixup 落点由它派生）
/// addend = 0                     # 可选：重定位值的链接期加减
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelocDef {
    /// 引用名（指令的 `reloc` 字段用它；全 ISA 唯一）。
    pub name: String,
    /// 宿主语义——有限集合，新增语义才需要宿主代码。
    pub semantics: RelocSemantics,
    /// 绑定的操作数槽（须是 `kind = "imm"` 的槽）。
    pub slot: String,
    /// 重定位值的链接期加减（缺省 0）。
    #[serde(default)]
    pub addend: Option<i64>,
}

/// 重定位的**宿主语义**（与宿主 `RelocKind` 一一对应）。
///
/// - `absolute`：把符号的绝对地址写进本指令绑定的槽（fixup = 指令末尾该槽的
///   字节区间 ⇒ 槽宽即补丁宽度，如 x86 `MOVABS_GLOBAL` 的 imm64）；
/// - `pc_relative`：写 `目标 − 指令起始`（fixup = 指令起始；补丁宽度 = 指令字长）。
///   具体落到哪些位段由**该 ISA 的 reloc patcher** 决定（riscv 按 opcode 分写
///   hi20/lo12，arm64 写 imm26/imm19）——那是 ISA 数据之外唯一的宿主代码，
///   "新增语义才需要宿主代码"这条边界由本枚举守住。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelocSemantics {
    /// 绝对地址（宿主 `RelocKind::Absolute(槽字节数)`）。
    Absolute,
    /// PC 相对（宿主 `RelocKind::Relative(指令字长, 0)`）。
    PcRelative,
}

/// `[[override]]` 表项（v18 S7d）：`key` = 点分路径，`value` = 新值。
///
/// 与 `include` 一样属于**组合键**：loader 用它替换被包含文件里的旧行，
/// 合并文本里既没有 `[[override]]` 块、也没有旧行。模型登记只为 schema/文档一致。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverrideDef {
    /// 点分路径，如 `meta.version` / `conventions.cond.eq.code`。
    pub key: String,
    /// 新值（TOML 标量或数组；loader 渲染成 `key = <值>` 一行）。
    pub value: toml::Value,
}

/// 操作数使用：**DSL 声明的名字** + 槽 + 角色（`ops = ["dst:r:out", …]`）。
///
/// 名字是作者面唯一的事实源：它既是 `asm` 模板里的占位符名（`{dst}`），
/// 也是**生成的 `Inst` 变体字段名**（v18 S7d）。编码位置（位域名 / modrm 角色）
/// 另行派生，不再借用名字——历史实现把"位域名"直接当字段名，于是
/// `ops = ["dst:r:out", "src:r"]` 生成出 `Inst::Iadd { rd, rs1 }`：
/// 作者写的名字在用户面**完全没有意义**。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperandUse {
    /// `ops` 里声明的操作数名（= `asm` 占位符名 = 生成字段名）。
    pub name: String,
    /// [[operand_slots]] 槽名。
    pub slot: String,
    /// 角色覆盖（缺省 "in"）。
    #[serde(default)]
    pub role: Option<OperandRole>,
}

// ──────────────────────── [[templates]] ────────────────────────

/// 参数化指令模板（v18 S2）：**一处声明 → N 条具体指令**，本 DSL 唯一的"指令分组"机制。
///
/// 取代 v17 的三套并行机制（`[[families]]` / `[[aliases]]` / 早期 `[[templates]]` 的平行
/// 参数表）——用户明确要求"只能保留一个，不然容易导致使用负担"。
///
/// - **行（`[[templates.rows]]`）是唯一的事实载体**：每行一条指令，键值都是 TOML 值
///   （可嵌套数组/表），不需要人工对齐多列平行列表；
/// - **`body` 是共享默认值**：行的同名字段覆盖它，表（如 `fields`/`modrm`）**递归合并**；
///   不给 `body` 就是"一组各自独立的指令"（原先 `[[aliases]]` 的场景）；
/// - **插值**：字符串可写 `{键}`（该行的键）与 `{inst}`（实例名），`{键.lower}` 取小写
///   （原先 `[[families]]` 的"变体名小写即助记符"由此显式表达）；
///   **整串恰为一个占位符**时保留值的类型（`opcode = "{opcode}"` 仍是整数）；
/// - **`ref`**：行（或 `body`）上的**引用名**——多条指令共用一个 `ref` 即"多态引用"
///   （lowering/pattern/emit 行首可用，按操作数签名分派）；原先的 `[[aliases]]` 就是
///   "几条指令共用同一个 ref"。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    /// 模板名（诊断用；缺省取首行的 `inst`）。
    #[serde(default)]
    pub name: Option<String>,
    /// 共享指令体（指令字段的默认值；可省略）。
    #[serde(default)]
    pub body: Option<toml::Table>,
    /// 行：每行一条指令实例（`inst` 必填，其余键即指令字段）。
    pub rows: Vec<TemplateRow>,
}

/// 模板的一行：`inst` = 实例名，其余键是**任意指令字段**（可含 `ref`）。
///
/// 故意**不** `deny_unknown_fields`：行键空间就是指令字段空间，由 `Instruction` 的反序列化
/// 拒绝拼错的键（错误消息点名哪一行/哪个实例）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateRow {
    /// 实例名（进 `Inst` 枚举与指令表；全 ISA 唯一）。
    pub inst: String,
    /// 该行的指令字段。
    #[serde(flatten)]
    pub fields: toml::Table,
}

/// 表递归合并：`src` 覆盖 `dst`；两边都是表时递归，其它情况整体替换。
pub(crate) fn deep_merge(dst: &mut toml::Table, src: &toml::Table) {
    for (k, v) in src {
        match (dst.get_mut(k), v) {
            (Some(toml::Value::Table(d)), toml::Value::Table(s)) => deep_merge(d, s),
            _ => {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
}

/// 值的文本形态（插值用）：字符串原样，其余用 TOML 文本。
fn value_text(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 把字符串里的 `{键}` / `{键.lower}` 换成该行的取值。
///
/// - **只替换"行里真有这个键"的占位符**：其余 `{…}` 原样保留（那是内层 DSL 的占位符，
///   如 `asm` 里的 `{rd}`/`{imm}`——由操作数校验负责它们的正确性）；
/// - 整串恰为一个占位符 ⇒ 直接返回该值（**保留类型**）；
/// - 否则做文本替换后继续递归（支持多个占位符与混合文本）。
fn subst(v: &toml::Value, vars: &toml::Table, path: &str) -> Result<toml::Value, String> {
    let _ = path;
    match v {
        toml::Value::String(s) => {
            let mut pos = 0usize;
            while let Some(rel_open) = s[pos..].find('{') {
                let open = pos + rel_open;
                let Some(rel_close) = s[open..].find('}') else {
                    break;
                };
                let close = open + rel_close;
                let raw = &s[open + 1..close];
                let (key, lower) = match raw.strip_suffix(".lower") {
                    Some(k) => (k, true),
                    None => (raw, false),
                };
                if let Some(val) = vars.get(key) {
                    let resolved = if lower {
                        toml::Value::String(value_text(val).to_lowercase())
                    } else {
                        val.clone()
                    };
                    if open == 0 && close == s.len() - 1 {
                        return Ok(resolved);
                    }
                    let mut next = String::with_capacity(s.len());
                    next.push_str(&s[..open]);
                    next.push_str(&value_text(&resolved));
                    next.push_str(&s[close + 1..]);
                    return subst(&toml::Value::String(next), vars, path);
                }
                // 不是本行的键 ⇒ 内层 DSL 的占位符，跳过它继续找下一个
                pos = close + 1;
            }
            Ok(v.clone())
        }
        toml::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(subst(it, vars, path)?);
            }
            Ok(toml::Value::Array(out))
        }
        toml::Value::Table(t) => {
            let mut out = toml::map::Map::new();
            for (k, val) in t {
                out.insert(k.clone(), subst(val, vars, path)?);
            }
            Ok(toml::Value::Table(out))
        }
        other => Ok(other.clone()),
    }
}

/// 收集值里引用的插值键（`{key}` / `{key.lower}`），递归进数组与表。
fn referenced_keys(v: &toml::Value, out: &mut std::collections::BTreeSet<String>) {
    match v {
        toml::Value::String(s) => {
            let mut pos = 0usize;
            while let Some(rel) = s[pos..].find('{') {
                let open = pos + rel;
                let Some(end) = s[open..].find('}') else {
                    break;
                };
                let close = open + end;
                let raw = &s[open + 1..close];
                let key = raw.strip_suffix(".lower").unwrap_or(raw);
                out.insert(key.to_string());
                pos = close + 1;
            }
        }
        toml::Value::Array(items) => {
            for it in items {
                referenced_keys(it, out);
            }
        }
        toml::Value::Table(t) => {
            for (_, val) in t {
                referenced_keys(val, out);
            }
        }
        _ => {}
    }
}

impl Template {
    /// 模板的展示名（诊断用）。
    pub fn label(&self, index: usize) -> String {
        self.name
            .clone()
            .or_else(|| self.rows.first().map(|r| r.inst.clone()))
            .unwrap_or_else(|| format!("#{index}"))
    }

    /// 展开为指令列表（每行一条）。
    ///
    /// 行键分三类（规则由 `body` 唯一决定）：
    ///
    /// - `body` 里**有**这个键 ⇒ 行值覆盖它（表递归合并）；
    /// - `body` 里**没有**、但 body 的字符串用 `{键}` 引用了它 ⇒ **纯参数**，
    ///   只参与插值、不进指令字段（如 `slot = "r64"`）；
    /// - 其余（body 没有、也没被引用）⇒ 当作**指令字段**（拼错会被 `Instruction`
    ///   的反序列化点名拒绝，例如把 `slot` 写成 `slott`）。
    pub fn expand(&self, index: usize) -> Result<Vec<Instruction>, String> {
        let label = self.label(index);
        let path = format!("[[templates.{label}]]");
        if self.rows.is_empty() {
            return Err(format!("{path}: rows 不能为空（模板至少要一行）"));
        }
        let body = self.body.clone().unwrap_or_default();
        let mut referenced = std::collections::BTreeSet::new();
        for (_, v) in &body {
            referenced_keys(v, &mut referenced);
        }
        let mut out = Vec::with_capacity(self.rows.len());
        for (r, row) in self.rows.iter().enumerate() {
            if row.inst.trim().is_empty() {
                return Err(format!("{path}: 第 {r} 行的 `inst` 不能为空"));
            }
            let mut table = body.clone();
            for (k, v) in &row.fields {
                if !body.contains_key(k) && referenced.contains(k) {
                    continue; // 纯参数：只参与插值
                }
                match (table.get_mut(k), v) {
                    (Some(toml::Value::Table(dst)), toml::Value::Table(src)) => {
                        deep_merge(dst, src)
                    }
                    _ => {
                        table.insert(k.clone(), v.clone());
                    }
                }
            }
            let mut vars = row.fields.clone();
            vars.insert("inst".to_string(), toml::Value::String(row.inst.clone()));
            for (_, v) in table.iter_mut() {
                *v = subst(v, &vars, &path)?;
            }
            table.insert("name".to_string(), toml::Value::String(row.inst.clone()));
            let mut inst: Instruction = toml::Value::Table(table)
                .try_into()
                .map_err(|e| format!("{path}: 行 '{name}' 的字段非法：{e}", name = row.inst))?;
            inst.from_template = Some(label.clone().into());
            out.push(inst);
        }
        Ok(out)
    }
}

impl V12Model {
    /// 展开全部 `[[templates]]`：实例拼进 `instructions`。
    ///
    /// 在**解析期**调用（`parse` 在展开 `lowering.vary` 之后），因此下游（校验/生成）
    /// 只看到普通指令与它们的 `ref`——生成器无需感知模板。
    pub fn expand_templates(&mut self) -> Result<(), String> {
        if self.templates.is_empty() {
            return Ok(());
        }
        let mut expanded: Vec<Instruction> = Vec::new();
        for (i, t) in self.templates.iter().enumerate() {
            expanded.extend(t.expand(i)?);
        }
        self.instructions.extend(expanded);
        Ok(())
    }
}

// ──────────────────────── [[lowering]] ────────────────────────

/// 指令选择规则。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lowering {
    /// IR 操作名（`"Iadd"`），或**一组同类 op**（`["Copy", "Uextend", "Freeze"]`）。
    ///
    /// 名单 = "这几条 op 的 lowering 完全一样，只维护一份序列"；解析期展开成逐 op 的
    /// 规则（顺序按名单序，因此每个 op 内部的裁决序与逐条写开时一致）。收益实测：
    /// x86 220 → 210 条声明、riscv 110 → 106（等价性由 `expand_ops` + 既有死规则检测守）。
    pub op: OpsField,
    /// 符号化指令模板：`"{out} = MOV_R_RM {0}, {1}"`。
    pub insts: Vec<String>,
    /// 结构化谓词（宽度条件化 lowering）。
    #[serde(default)]
    pub when: Option<toml::Value>,
    /// 参数化行表：各列表**等长**，按下标 zip 成行展开成多条具体规则。
    ///
    /// 键分两类：
    /// - 名字在 [`crate::v12::pred::PRED_ATTRS`] 里 → 该行自动追加
    ///   `eq = [键, 值]` 到 `when`，**且**可在模板里用 `{键}` 引用；
    /// - 其余 → 纯替换变量（只在模板里用 `{键}`）。
    ///
    /// 消除"一个 op 一堆只差助记符的规则"：x86 `Vadd` 8 条（4 elem × 2 宽度）
    /// → 2 条，`Fcmp` 32 条（16 cond × 2 elem）→ 4 条。
    #[serde(default)]
    pub vary: Option<BTreeMap<String, Vec<VaryValue>>>,
    /// 显式优先级（缺省 0，大者先试）。规则**排序不依赖声明序**：
    /// 按 (priority 降, 谓词叶子数降, 声明序升) 裁决。
    ///
    /// 只在"故意让更宽的规则赢过更具体的规则"时才需要——例如 x86 `Vextract`
    /// 的 lane 0 快路径（`imm0 == 0` 两个约束）必须压过 V256 通路
    /// （`rd/elem/imm0` 三个约束）。其余场合留空，让特异性自动裁决。
    #[serde(default)]
    pub priority: Option<i32>,
}

/// `[[lowering]].op` 的两种写法：单个名字，或一组名字（TOML 里同键同时接受
/// 字符串与字符串数组——`op = "Iadd"` / `op = ["Copy", "Uextend"]`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpsField {
    One(String),
    Many(Vec<String>),
}

impl OpsField {
    /// 单个 op 名（**展开后**每条规则恰好一个；未展开时取第一个，仅用于诊断）。
    pub fn name(&self) -> &str {
        match self {
            OpsField::One(s) => s,
            OpsField::Many(v) => v.first().map(String::as_str).unwrap_or(""),
        }
    }

    /// 名字列表（原名或名单原序）。
    pub fn names(&self) -> Vec<String> {
        match self {
            OpsField::One(s) => vec![s.clone()],
            OpsField::Many(v) => v.clone(),
        }
    }
}

/// `vary` 的取值：整数（可作谓词值）或字符串（只作模板替换）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VaryValue {
    Int(i64),
    Str(String),
}

impl VaryValue {
    /// 模板替换用的文本。
    pub fn as_text(&self) -> String {
        match self {
            VaryValue::Int(v) => v.to_string(),
            VaryValue::Str(s) => s.clone(),
        }
    }
    /// 谓词值（仅整数可用）。
    pub fn as_int(&self) -> Option<i64> {
        match self {
            VaryValue::Int(v) => Some(*v),
            VaryValue::Str(_) => None,
        }
    }
}

impl V12Model {
    /// 按 op 分组的 lowering 规则，**每组内按裁决序排好**：
    /// `(priority 降, 谓词叶子数降, 声明序升)`。
    ///
    /// 排序取代"声明序首匹配"：作者不再需要记住"兜底规则必须写在最后"，
    /// 也不会因为插入一条规则的位置不对而静默改变分派。校验器与生成器读同一
    /// 个顺序（唯一事实源在此），故编译期报出的死规则就是运行期真的死规则。
    /// op 之间保持首次出现的声明序（生成代码的 match arm 顺序稳定）。
    pub fn lowering_by_op(&self) -> Vec<(&str, Vec<&Lowering>)> {
        let mut by_op: Vec<(&str, Vec<(usize, &Lowering)>)> = Vec::new();
        for (i, rule) in self.lowering.iter().enumerate() {
            let op = rule.op.name();
            match by_op.iter_mut().find(|(o, _)| *o == op) {
                Some((_, rules)) => rules.push((i, rule)),
                None => by_op.push((op, vec![(i, rule)])),
            }
        }
        by_op
            .into_iter()
            .map(|(op, mut rules)| {
                rules.sort_by_key(|(i, r)| {
                    let leaves = r
                        .when
                        .as_ref()
                        .and_then(|v| crate::v12::pred::parse(v).ok())
                        .map(|p| crate::v12::pred::leaf_count(&p))
                        .unwrap_or(0);
                    (-r.priority.unwrap_or(0) as i64, -(leaves as i64), *i as i64)
                });
                (op, rules.into_iter().map(|(_, r)| r).collect())
            })
            .collect()
    }
}

impl V12Model {
    /// 谓词属性全集：核心属性（[`super::pred::PRED_ATTRS`]）+ `[[derive]]` 名。
    ///
    /// 校验器与 `vary` 的"键是谓词属性还是纯替换变量"判定同读这一份。
    /// `[[pattern]]` 的裁决序（返回**声明下标**的排序结果）：v18 S5c 起与
    /// `[[lowering]]` 用同一套键——(`priority` 降, 匹配树 Op 节点数降, `when` 谓词
    /// 叶子数降, 声明序升)；后两者合起来就是"特异性降序"。
    ///
    /// codegen（生成 `__PATTERNS` 表与分派臂）与校验器（死模式检测）**共读这一份**，
    /// 消除"两边各排一次、排法不一致"的风险。
    pub fn pattern_order(&self) -> Result<Vec<usize>, String> {
        let mut keys: Vec<(i32, i32, i32)> = Vec::with_capacity(self.pattern.len());
        for (i, p) in self.pattern.iter().enumerate() {
            let tree = crate::v12::match_tree::parse(&p.r#match)
                .map_err(|e| format!("[[pattern]] #{i}.match: {e}"))?;
            let mut nodes = 0usize;
            fn count(n: &crate::v12::match_tree::MatchNode, acc: &mut usize) {
                if let crate::v12::match_tree::MatchNode::Op { args, .. } = n {
                    *acc += 1;
                    for a in args {
                        count(a, acc);
                    }
                }
            }
            count(&tree, &mut nodes);
            let leaves = match &p.when {
                None => 0,
                Some(v) => {
                    let pred = crate::v12::pred::parse(v)
                        .map_err(|e| format!("[[pattern]] #{i}.when: {e}"))?;
                    crate::v12::pred::leaf_count(&pred)
                }
            };
            keys.push((p.priority.unwrap_or(0), nodes as i32, leaves as i32));
        }
        let mut order: Vec<usize> = (0..self.pattern.len()).collect();
        order.sort_by_key(|&i| (-keys[i].0, -keys[i].1, -keys[i].2, i));
        Ok(order)
    }

    pub fn pred_attr_names(&self) -> Vec<String> {
        let mut out: Vec<String> = super::pred::PRED_ATTRS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        out.extend(self.derive.iter().map(|d| d.name.clone()));
        out
    }

    /// 展开 `[[derive]]`（解析期）：把 `expr` 解析成谓词 AST，存进
    /// `derived_preds`（下游只认展开后的谓词）。
    ///
    /// 错误（Parse 阶段，消息可直接锚到声明）：名称为空/重复/与核心属性重名、
    /// `expr` 不是合法谓词。**派生只能引用核心属性**（不支持派生引用派生）——
    /// 那样做要么得给"属性名出现在值位置"设计代换语义（`eq = [is_64, 0]` 该展开成
    /// 什么？），要么得引入递归求值；两者都会把"谓词是纯数据"这条边界弄糊，
    /// 所以这里直接拒绝，由校验器给出明确错误。
    pub fn expand_derives(&mut self) -> Result<(), String> {
        if self.derive.is_empty() {
            return Ok(());
        }
        let mut out: BTreeMap<String, super::pred::Pred> = BTreeMap::new();
        for d in &self.derive {
            if d.name.trim().is_empty() {
                return Err("[[derive]]: name 不能为空".into());
            }
            if super::pred::PRED_ATTRS.contains(&d.name.as_str()) {
                return Err(format!(
                    "[[derive.{}]]: 名字与核心谓词属性重名（核心属性：{}）——换个名字，别遮蔽它",
                    d.name,
                    super::pred::PRED_ATTRS.join(" / ")
                ));
            }
            let pred = super::pred::parse(&d.expr)
                .map_err(|e| format!("[[derive.{}]] expr: {e}", d.name))?;
            if out.insert(d.name.clone(), pred).is_some() {
                return Err(format!("[[derive.{}]]: 名字重复", d.name));
            }
        }
        self.derived_preds = out;
        Ok(())
    }
}

impl Lowering {
    /// 展开 `op` 名单 → 每条 op 一条规则（单个 op 时返回自身单元素）。
    ///
    /// 名单是"这几条 op 的 lowering 完全一样"，因此展开只是复制规则、换掉 op 名；
    /// 名单序 = 展开序（每个 op 内部的规则相对顺序与逐条写开时**完全一致**，
    /// 于是既有的 (`priority`, 谓词叶子数, 声明序) 裁决结果不变）。
    pub fn expand_ops(&self) -> Result<Vec<Lowering>, String> {
        let names = self.op.names();
        let path = format!("[[lowering.{}]].op", self.op.name());
        if names.is_empty() {
            return Err(format!("{path}: op 名单不能为空"));
        }
        let mut seen: Vec<&str> = Vec::with_capacity(names.len());
        for n in &names {
            if n.trim().is_empty() {
                return Err(format!("{path}: op 名不能为空"));
            }
            if seen.contains(&n.as_str()) {
                return Err(format!("{path}: op '{n}' 在名单里重复"));
            }
            seen.push(n.as_str());
        }
        if names.len() == 1 {
            return Ok(vec![self.clone()]);
        }
        Ok(names
            .into_iter()
            .map(|n| Lowering {
                op: OpsField::One(n),
                ..self.clone()
            })
            .collect())
    }

    /// 展开 `vary` 行表 → 多条具体规则（无 `vary` 时返回自身单元素）。
    ///
    /// 每行：模板里 `{键}` 换成该行取值；键若是谓词属性（`PRED_ATTRS`）则
    /// 额外把 `eq = [键, 值]` 合入 `when`（与原 `when` 取 `and`）。
    pub fn expand_vary(&self, pred_attrs: &[String]) -> Result<Vec<Lowering>, String> {
        let Some(vary) = &self.vary else {
            return Ok(vec![self.clone()]);
        };
        let path = format!("[[lowering.{}]].vary", self.op.name());
        if vary.is_empty() {
            return Err(format!("{path}: 不能为空表"));
        }
        let rows = vary.values().next().map(Vec::len).unwrap_or(0);
        if rows == 0 {
            return Err(format!("{path}: 列表不能为空"));
        }
        for (k, v) in vary {
            if v.len() != rows {
                return Err(format!(
                    "{path}: 各列表必须等长（按下标 zip 成行）——'{k}' 长 {} ≠ {rows}",
                    v.len()
                ));
            }
        }
        let mut out = Vec::with_capacity(rows);
        for row in 0..rows {
            let mut insts = self.insts.clone();
            let mut extra: Vec<toml::Value> = Vec::new();
            for (k, vals) in vary {
                let val = &vals[row];
                let needle = format!("{{{k}}}");
                let text = val.as_text();
                for line in insts.iter_mut() {
                    if line.contains(&needle) {
                        *line = line.replace(&needle, &text);
                    }
                }
                if pred_attrs.iter().any(|a| a == k) {
                    let iv = val.as_int().ok_or_else(|| {
                        format!("{path}: '{k}' 是谓词属性，取值必须是整数，got {val:?}")
                    })?;
                    extra.push(toml::Value::Table(
                        [(
                            "eq".to_string(),
                            toml::Value::Array(vec![
                                toml::Value::String(k.clone()),
                                toml::Value::Integer(iv),
                            ]),
                        )]
                        .into_iter()
                        .collect(),
                    ));
                }
            }
            let when = match (&self.when, extra.len()) {
                (base, 0) => base.clone(),
                (None, 1) => Some(extra.remove(0)),
                (base, _) => {
                    let mut all = Vec::with_capacity(extra.len() + 1);
                    if let Some(b) = base {
                        all.push(b.clone());
                    }
                    all.append(&mut extra);
                    Some(toml::Value::Table(
                        [("and".to_string(), toml::Value::Array(all))]
                            .into_iter()
                            .collect(),
                    ))
                }
            };
            out.push(Lowering {
                op: self.op.clone(),
                insts,
                when,
                vary: None,
                priority: self.priority,
            });
        }
        Ok(out)
    }
}

// ───────────────────────── [[pattern]] ─────────────────────────

/// 树型多指令匹配（S6）：声明一棵 IR **匹配树**（`match = "Fadd(Fmul(a,b),c)"`）
/// 与一段发射序列（`insts`）。运行期 lowering 驱动在块内逆序预扫时，把命中
/// 整棵子树的 IR 值合并成一次 `lower_pattern` 发射（叶变量按 DFS 序绑定为
/// `{N}` 输入），跳过被 consumed 的内部节点。无 `[[pattern]]` 的 ISA 零开销。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    /// 匹配树字符串（根必须是 Op 调用，变量是裸标识符）。见
    /// [`crate::v12::match_tree::parse`]。
    #[serde(rename = "match")]
    pub r#match: String,
    /// 显式优先级（缺省 0，大者先试）——与 `[[lowering]].priority` **同语义**（v18 S5c）。
    ///
    /// 裁决序 = (`priority` 降, 匹配树 Op 节点数降, `when` 谓词叶子数降, 声明序升)；
    /// 只在"故意让更宽的模式赢过更具体的模式"时才需要。
    #[serde(default)]
    pub priority: Option<i32>,
    /// 根指令派生属性上的结构化谓词（与 `[[lowering]].when` 同语法）——
    /// 两个结构相同、只差守卫（如 f32 vs f64）的模式靠它区分。
    #[serde(default)]
    pub when: Option<toml::Value>,
    /// 发射序列模板：叶变量 `{名字}` 与 `{out}`（codegen 把 `{名字}` 改写成
    /// 树 DFS 序的 `{N}` 后交给 lowering 发射器）。
    pub insts: Vec<String>,
}

// ───────────────────────── [abi] ─────────────────────────

/// `[abi.stack_args]` — 寄存器耗尽后的参数内存布局（2026-09-13 去 x86 写死）。
/// x86（Windows x64）栈参数在**被调方**是 `[fp + first_offset_slots*slot + k*stride_slots*slot]`、
/// 在**调用方**是 `[sp + shadow_bytes + k*stride_slots*slot]`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AbiStackArgs {
    /// **被调方**收参的基址寄存器：`"fp"`（帧指针，x86）或 `"sp"`。
    /// 缺省 `"fp"`；取自 `[abi.frame]` 声明的寄存器名。
    #[serde(default)]
    pub callee_base: Option<String>,
    /// **调用方**写栈参数的基址：`"sp"`（x86：`[sp + shadow + k*stride]`）或 `"fp"`。
    /// 缺省 `"sp"`。
    #[serde(default)]
    pub caller_base: Option<String>,
    /// 被调方第一个栈参数相对 `callee_base` 的槽数（x86 = 2：返回地址 + 保存的 fp）。
    #[serde(default)]
    pub first_offset_slots: Option<u32>,
    /// 相邻栈参数的槽步长（x86 = 1）。
    #[serde(default)]
    pub stride_slots: Option<u32>,
    /// 调用方在 call 前预留的 shadow space 字节数（Windows x64 = 32）。
    #[serde(default)]
    pub shadow_bytes: Option<u32>,
}

/// 调用约定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Abi {
    /// 帧布局的额外栈填充（字节）：x86 = 8（align/2，SysV/Windows x64
    /// ABI：prologue push rbp + callee-saved 后 rsp%16==8，sub rsp 需使
    /// call 前 rsp%16==0）。缺省 0。
    #[serde(default)]
    pub frame_padding: Option<i32>,
    /// 栈参数布局（`[abi.stack_args]`）：寄存器耗尽后的第 N+ 个参数怎么放。
    /// `None` = 不支持栈参数（超寄存器参数 → Unsupported）。
    #[serde(default)]
    pub stack_args: Option<AbiStackArgs>,
    #[serde(default)]
    pub arg_class: Vec<ArgClass>,
    /// 帧布局（sp/fp 寄存器名、帧分配/释放指令名）。
    #[serde(default)]
    pub frame: Option<AbiFrame>,
    /// 被调用者保存寄存器（prologue push / epilogue pop 顺序）。
    #[serde(default)]
    pub callee_saved: Option<CalleeSaved>,
    /// 溢出 scratch 寄存器（spill load/store 用；x86 R10/R11）。
    #[serde(default)]
    pub scratch: Vec<String>,
    /// 返回寄存器（物理名；如 riscv "X10"=a0）。缺省空 = index 0（x86 RAX
    /// 语义）。Return/Call lowering 的返回值移动目标用此列表首项。
    #[serde(default)]
    pub ret_regs: Vec<String>,
    /// Call 的返回地址寄存器（缺省 "X1"=riscv ra）。
    #[serde(default)]
    pub call_ret_reg: Option<String>,
    /// Call 点被调用方可能破坏的寄存器（物理名）——regalloc 的 call clobber
    /// 集。缺省 = 整数参数寄存器 + 返回寄存器（x86 语义）。定宽 ISA 无
    /// callee-saved 保存序列（如 riscv 当前 callee_saved=[]）时，callee 会
    /// 破坏全部 caller-saved（临时）寄存器 → 必须把 t0-t6 等也列入，否则
    /// 跨调用存活值留在寄存器被覆盖（实测递归 fib 死循环）。
    #[serde(default)]
    pub call_clobbers: Option<Vec<String>>,
    /// regalloc 不可分配的寄存器（物理名；如 riscv 的 X0=zero 不可写、
    /// X1=ra 返回地址被 prologue/call 占用、X3/X4=gp/tp）。缺省空。
    #[serde(default)]
    pub reserved: Vec<String>,
    /// 参数槽位分配规则（语义显式声明，见 [`ArgSlot`]）。
    #[serde(default)]
    pub arg_slot: Option<ArgSlot>,
}

/// 参数槽位分配规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArgSlot {
    /// 缺省（riscv SysV）：int/float 各自独立推进——int 序列 RCX/RDX/… 与
    /// float 序列 XMM0/… 分开计数。
    #[default]
    ByClass,
    /// Windows x64：int/float 共享位置计数——参数 i 用 GPR{i}/XMM{i}
    /// （第 2 参数即使第 1 个是整数也用 XMM1）。
    ByPosition,
}

/// 帧布局模式（[abi.frame].layout）：决定 callee-saved 保存槽相对帧的位置。
/// 其余帧数值（min_frame_bytes / callee_saved_bytes / stack_slot_shift）全部
/// 由运行期从本模式 + fp_push_bytes + callee_saved 表**推导**（pipeline/
/// frame_layout.rs::frame_layout_info），不再在 TOML 里手工写魔法数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LayoutMode {
    /// fp-outside（缺省，x86/demo）：callee-saved 用硬件 push 在帧指针**上方**
    /// （帧外）。spill 槽 sp_base = -(frame) - callee_saved_bytes、栈槽基准
    /// fp - callee_saved_bytes。
    #[default]
    #[serde(rename = "fp-outside")]
    FpOutside,
    /// fp-inside（riscv）：ra/fp/callee-saved 保存槽在帧**内顶部**
    /// （@push_callee 的 SD 到 [sp+frame-fp_push-(k+1)*8]，帧分配覆盖到固定
    /// 最小帧）。spill 槽 sp_base = -(frame)（帧内底部）、栈槽平移 = fp_push。
    #[serde(rename = "fp-inside")]
    FpInside,
}

/// 帧布局配置（[abi.frame]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbiFrame {
    /// 栈指针寄存器名（"RSP"）。
    pub sp: String,
    /// 帧指针寄存器名（"RBP"；None = 无帧指针）。
    #[serde(default)]
    pub fp: Option<String>,
    /// 帧布局模式（见 [`LayoutMode`]；缺省 fp-outside）。
    #[serde(default)]
    pub layout: LayoutMode,
    /// prologue 在帧指针上方 push 的字节数（帧指针保存槽；x86 = 8）。
    #[serde(default)]
    pub fp_push_bytes: Option<u32>,
    /// @frame_alloc 的立即数取负（riscv `addi sp, sp, -N`：ADDI 是加法指令、
    /// 帧分配需负偏移；x86 用 SUB 语义不需要）。缺省 false。
    #[serde(default)]
    pub alloc_neg: bool,
}

/// 被调用者保存寄存器（[abi.callee_saved]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalleeSaved {
    #[serde(default)]
    pub gpr: Vec<String>,
    #[serde(default)]
    pub xmm: Vec<String>,
}

/// 传参类别：arg_class 的类型语义（决定传参寄存器族与策略）。
/// serde 用小写字符串（"int"/"float"/"vector"/...），未知类别 → 解析失败
///（deny_unknown 语义提前到反序列化层，validate 不再做字符串自由检查）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArgClassKind {
    /// 整数/指针参数（GPR 族）。
    Int,
    /// 浮点标量参数（FPR 族）。
    Float,
    /// 向量参数（VEC 族；可配 by-ref 策略）。
    Vector,
    /// 其他自定义类别（KReg/掩码等）——生成器按通用寄存器槽处理。
    #[serde(rename = "other")]
    Other,
}

impl ArgClassKind {
    /// 人类可读名（错误消息用）。
    pub fn name(self) -> &'static str {
        match self {
            ArgClassKind::Int => "int",
            ArgClassKind::Float => "float",
            ArgClassKind::Vector => "vector",
            ArgClassKind::Other => "other",
        }
    }
}

/// 类型类别 → 传参寄存器/策略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgClass {
    /// 传参类别（int/float/vector/other——枚举，语义显式）。
    pub class: ArgClassKind,
    #[serde(default)]
    pub regs: Vec<String>,
    /// 传参策略（见 [`ArgStrategy`]）。
    #[serde(default)]
    pub strategy: Option<ArgStrategy>,
    /// 策略适用的大小上限（位）。
    #[serde(default)]
    pub limit: Option<u32>,
}

/// 传参策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArgStrategy {
    /// 超过 `limit` 位的值按引用传（调用方栈拷贝 + 传指针；YMM/ZMM ABI）。
    ByRef,
}

// ───────────────────────── [emit] ─────────────────────────

/// 序言/尾声指令块。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitSection {
    #[serde(default)]
    pub prologue: Option<EmitBlock>,
    #[serde(default)]
    pub epilogue: Option<EmitBlock>,
    /// `.align N` 伪指令的填充字节（缺省 0x00）。
    #[serde(default)]
    pub align_pad: Option<u8>,
    /// 是否生成独立尾声标签 + return 块的 epilogue 跳转（缺省 true =
    /// x86 语义：return block 经 epilogue_jump 跳到统一尾声）。定宽 ISA
    /// 无 JMP 指令时可设 false：return block 直接 fall-through 到尾声
    /// （仅单 return block 函数安全）。
    #[serde(default)]
    pub epilogue_label: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitBlock {
    pub insts: Vec<String>,
}

// ───────────────────────── [spill.*] ─────────────────────────

/// 溢出槽模板：load/store 指令 + 基址寄存器。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpillTemplate {
    /// 加载指令模板（`{0}` = 目标寄存器，`{1}` = 帧偏移——spill 生成时替换）。
    pub load: String,
    /// 存储指令模板（`{0}` = 源寄存器，`{1}` = 帧偏移）。
    pub store: String,
    /// 基址寄存器名（"RBP"）。
    #[serde(default)]
    pub base: Option<String>,
}
