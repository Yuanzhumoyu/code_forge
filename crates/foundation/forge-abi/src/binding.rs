//! [`AbiBinding`] — **(ISA, 约定) 的寄存器绑定**：把规则里的抽象池名绑到具体寄存器。
//!
//! 规则（[`crate::rules::AbiRules`]）是平台无关的；同一份 `win64` 规则能被任何
//! "寄存器编号与 Windows x64 一致"的 ISA 复用，靠的就是这一层：
//!
//! ```toml
//! isa = "x86"; conv = "win64"
//! [pools]
//! int    = ["RCX", "RDX", "R8", "R9"]
//! float  = ["XMM0", "XMM1", "XMM2", "XMM3"]
//! vector = ["XMM0", "XMM1", "XMM2", "XMM3"]
//! ```
//!
//! 池里的选择子两种写法：`"RCX"`（名字，ISA 侧解析）或 `7`（该 ISA 的寄存器号）。
//! 解析不到 → [`crate::error::AbiError::UnresolvedReg`]（fail-closed，不猜）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::AbiTarget;
use crate::error::AbiError;
use crate::plan::RegRef;

/// 池里的一个寄存器选择子。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RegSelector {
    /// 该 ISA 的寄存器号（0 基）。
    Index(u32),
    /// 寄存器名（ISA 侧解析；大小写敏感由 ISA 决定）。
    Name(String),
}

impl std::fmt::Display for RegSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegSelector::Index(i) => write!(f, "{i}"),
            RegSelector::Name(n) => write!(f, "\"{n}\""),
        }
    }
}

/// `[link.<format>]` 的一条：ISA 语义名 → 链接器重定位类型（v27 M6-98）。
///
/// 这是**目标工具链事实**（LLVM 的 `*ELFObjectWriter::getRelocType`、rustc 的
/// `Target`/`TargetOptions` 都在目标层），放在约定数据里 ⇒ 新增指令集只需要加
/// TOML，`forge-object`/`forge-rustc` 不改代码。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkEntry {
    /// 链接器重定位类型号（ELF `r_type` / COFF `typ` / MachO `r_type`）。
    #[serde(rename = "type")]
    pub rtype: u32,
    /// 链接器要重写的位段（写对象前 `word &= !clear_mask`）；0 = 不清。
    #[serde(default)]
    pub clear_mask: u32,
}

/// (ISA, 约定) 的寄存器绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbiBinding {
    /// ISA 名（与 `TargetMachine::isa_info().name()` 一致）。
    pub isa: String,
    /// 约定名（与 `AbiRules::name` 一致）。
    pub conv: String,
    /// 池名 → 有序寄存器列表。
    pub pools: BTreeMap<String, Vec<RegSelector>>,
    /// 对象格式 → (ISA 语义名 → 链接器重定位类型)。省略 = 全部走对象库的
    /// Generic 映射（x86 现状）。
    #[serde(default)]
    pub link: BTreeMap<String, BTreeMap<String, LinkEntry>>,
}

impl AbiBinding {
    pub fn from_toml(text: &str) -> Result<Self, AbiError> {
        toml::from_str(text).map_err(|e| AbiError::Parse(e.to_string()))
    }

    /// 解析一个池；池不存在 → [`AbiError::MissingPool`]。
    pub fn resolve_pool(
        &self,
        pool: &str,
        target: &dyn AbiTarget,
    ) -> Result<Vec<RegRef>, AbiError> {
        let Some(list) = self.pools.get(pool) else {
            return Err(AbiError::MissingPool {
                conv: self.conv.clone(),
                binding: self.isa.clone(),
                pool: pool.to_string(),
            });
        };
        let mut out = Vec::with_capacity(list.len());
        for (i, sel) in list.iter().enumerate() {
            let index = match sel {
                RegSelector::Index(ix) => *ix,
                RegSelector::Name(n) => {
                    target.reg_index(n).ok_or_else(|| AbiError::UnresolvedReg {
                        binding: self.isa.clone(),
                        pool: pool.to_string(),
                        index: i,
                        sel: sel.to_string(),
                        why: format!("ISA `{}` 里没有名为 `{n}` 的寄存器", target.isa_name()),
                    })?
                }
            };
            if index >= target.reg_count() {
                return Err(AbiError::UnresolvedReg {
                    binding: self.isa.clone(),
                    pool: pool.to_string(),
                    index: i,
                    sel: sel.to_string(),
                    why: format!(
                        "寄存器号 {index} 超出该 ISA 的寄存器总数 {}",
                        target.reg_count()
                    ),
                });
            }
            out.push(RegRef::new(
                index,
                target.reg_slot(index),
                target.reg_class(index),
                target
                    .reg_name(index)
                    .unwrap_or_else(|| format!("p{index}")),
            ));
        }
        Ok(out)
    }

    /// 绑定自洽性：非空、无重复寄存器（同一寄存器出现在两个池里，多数约定会打架）。
    pub fn validate(&self) -> Result<(), AbiError> {
        let bad = |why: String| AbiError::BadRules {
            name: format!("binding {}", self.isa),
            why,
        };
        if self.isa.trim().is_empty() || self.conv.trim().is_empty() {
            return Err(bad("isa/conv 不能为空".into()));
        }
        for (pool, list) in &self.pools {
            if list.is_empty() {
                return Err(bad(format!("池 `{pool}` 为空——要么删掉它，要么给寄存器")));
            }
        }
        // `[link.<format>]`：格式名必须是 elf/coff/macho、语义名非空。
        for (format, entries) in &self.link {
            if forge_abi_model::LinkFormat::parse(format).is_none() {
                return Err(bad(format!(
                    "`[link.{format}]` 不是已知对象格式（elf / coff / macho）"
                )));
            }
            for name in entries.keys() {
                if name.trim().is_empty() {
                    return Err(bad(format!("`[link.{format}]` 里的重定位语义名不能为空")));
                }
            }
        }
        Ok(())
    }

    /// 本绑定声明的链接层重定位表（只取 `format` 那一份）。
    pub fn link_table(
        &self,
        format: forge_abi_model::LinkFormat,
    ) -> forge_abi_model::LinkRelocTable {
        let entries = self
            .link
            .get(format.as_str())
            .map(|m| {
                m.iter()
                    .map(|(name, e)| forge_abi_model::LinkReloc {
                        format,
                        name: name.clone(),
                        rtype: e.rtype,
                        clear_mask: e.clear_mask,
                    })
                    .collect()
            })
            .unwrap_or_default();
        forge_abi_model::LinkRelocTable::new(entries)
    }
}
