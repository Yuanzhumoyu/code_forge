//! **机器能力视图**：从一份 ISA 谱里读出"这台机器有什么"——寄存器表、角色声明、
//! 固定用途寄存器、链接寄存器。**不读调用约定**（那是使用者的数据，见 `forge-abi`）。
//!
//! 为什么单独开一层而不是把 `V12Model` 暴露出去：
//!
//! - CLI（`forge-isa abi …`）与将来的宿主适配器只需要**这一小块**（`AbiTarget` 的
//!   输入面），不需要整个模型；
//! - 寄存器**编号**在这里定死（见下），于是"绑定文件里的名字解析成几号"不依赖
//!   任何后端 crate——这正是"没有宿主也能校验约定"的前提。
//!
//! ## 编号规则（与生成物一致）
//!
//! `codegen/machine.rs::gen_reg_enum` 的规则是"每个组内 `i + base_index`"，
//! 不同宽度视图共享同一物理号（x86 的 `RAX`/`EAX`/`AX`/`AL` 都是 0）。
//! 本视图把它整理成**两个物理寄存器文件**（与 `TargetRegInfo::num_gp_regs` /
//! `num_fp_regs` 对齐）：
//!
//! ```text
//! GPR 区 = 地址类组的成员，号 0..n_gpr-1        （x86: RAX..R15 = 0..15）
//! FP  区 = 主 FPR 组的成员，号 n_gpr..n_gpr+n_fp-1（x86: XMM0..XMM15 = 16..31）
//! ```
//!
//! 别名（`EAX`/`W0`/`ZMM0`…）解析到**同一物理号**；落到两个区之外的别名
//! （x86 的 `ZMM16..ZMM31`：AVX-512 的 32 个向量寄存器，而 XMM 视图只有 16 个）
//! 不进名字表，只在 `notes` 里留一条记录——不假装它们存在。
//! KReg（掩码寄存器）不参与 ABI，不进本视图。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::loader::LoadedSpec;
use crate::v12::model::{RegClass, Role, V12Model};
use crate::v12::shared::group_names;

/// 一个物理寄存器（`index` = 本文档顶部的编号）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegEntry {
    pub index: u32,
    pub name: String,
    /// 类名（诊断/快照用，如 `"GPR(8)"`）。
    pub class: String,
    /// 字节宽度（主视图的宽度）。
    pub width: u8,
    /// 固定用途（sp/fp/reserved）：**不可分配**。
    pub pinned: bool,
}

/// 一个角色族的声明情况。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleInfo {
    /// 是否有"无宽度语义"的声明（`roles = ["gpr_mov"]`）。
    pub widthless: bool,
    /// 有宽度语义的声明集合（位；如 `fpr_mov` 的 32/64）。
    pub bits: BTreeSet<u16>,
    /// 声明该角色的指令名（排序，诊断用；最多留 [`MAX_ROLE_INSTS`] 条）。
    pub insts: Vec<String>,
}

/// 每个角色最多记多少条指令名（报告只要"有没有、谁给的"，不需要全量）。
pub const MAX_ROLE_INSTS: usize = 6;

/// 一台机器的能力视图。
#[derive(Debug, Clone)]
pub struct MachineView {
    /// ISA 名（`[meta].name`）。
    pub isa: String,
    /// 谱的来源文件（include 展开后的根 + 各来源；诊断用）。
    pub sources: Vec<PathBuf>,
    /// 物理寄存器（GPR 区 + FP 区）。
    pub regs: Vec<RegEntry>,
    /// 名字（含别名）→ 物理号。
    pub names: BTreeMap<String, u32>,
    /// 固定用途（`[abi].reserved` + sp/fp），已排序去重。
    pub pinned: Vec<u32>,
    /// 溢出 scratch（`[abi].scratch`）。
    pub scratch: Vec<u32>,
    /// 链接寄存器（`[abi].call_ret_reg`）。
    pub link: Option<u32>,
    /// GPR / FP 区大小。
    pub n_gpr: u32,
    pub n_fp: u32,
    /// 角色 → 声明情况。
    pub roles: BTreeMap<String, RoleInfo>,
    /// 视图自身的说明（例如被丢掉的越界别名）。
    pub notes: Vec<String>,
}

impl MachineView {
    pub fn reg_name(&self, index: u32) -> Option<&str> {
        self.regs
            .get(index as usize)
            .map(|r| r.name.as_str())
            .filter(|_| index < self.n_gpr + self.n_fp)
    }

    pub fn reg_index(&self, name: &str) -> Option<u32> {
        self.names.get(name).copied()
    }

    /// 是否声明了某个角色（不看宽度）。
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.contains_key(role)
    }

    /// 某角色声明过的最宽位宽（无宽度语义 → `None`）。
    pub fn role_bits(&self, role: &str) -> Option<u16> {
        self.roles
            .get(role)
            .and_then(|r| r.bits.iter().next_back().copied())
    }
}

/// 读一份谱并建能力视图。失败 = 人可读诊断（含 `行:列` 的来源映射）。
pub fn inspect(path: &Path) -> Result<MachineView, String> {
    let spec = LoadedSpec::load(path)?;
    let (model, _projection) =
        crate::v12::parse_and_validate_projected(&spec.text, &Default::default())
            .map_err(|e| crate::render_error_for(&spec, &e))?;
    build(&model, &spec)
}

/// 从已解析的模型建视图（`inspect` 的可测内核）。
pub fn build(model: &V12Model, spec: &LoadedSpec) -> Result<MachineView, String> {
    let mut notes: Vec<String> = Vec::new();
    let addr = model.addr_class()?;
    let fpr_main = model.main_fpr_class()?;

    let mut regs: Vec<RegEntry> = Vec::new();
    let mut names: BTreeMap<String, u32> = BTreeMap::new();

    // ── GPR 区（地址类组的成员顺序）──
    let gpr_names = group_names_of(model, addr)?;
    for (i, n) in gpr_names.iter().enumerate() {
        regs.push(RegEntry {
            index: i as u32,
            name: n.clone(),
            class: format!("GPR({})", addr.width()),
            width: addr.width() as u8,
            pinned: false,
        });
        names.insert(n.clone(), i as u32);
    }
    let n_gpr = gpr_names.len() as u32;

    // ── FP 区（主 FPR 组）──
    let mut n_fp = 0u32;
    if let Some(fpr) = fpr_main {
        let fp_names = group_names_of(model, fpr)?;
        n_fp = fp_names.len() as u32;
        for (i, n) in fp_names.iter().enumerate() {
            regs.push(RegEntry {
                index: n_gpr + i as u32,
                name: n.clone(),
                class: format!("FPR({})", fpr.width()),
                width: fpr.width() as u8,
                pinned: false,
            });
            names.insert(n.clone(), n_gpr + i as u32);
        }
    } else {
        notes.push(
            "谱里没有 FPR 寄存器组：浮点/向量参数没有寄存器可落（约定侧会报 MissingPool）".into(),
        );
    }
    let total = n_gpr + n_fp;

    // ── 别名（其它宽度视图 / VEC 视图）──
    let mut dropped: Vec<String> = Vec::new();
    for (rc, group) in &model.reg {
        let list = group_names(group)?;
        let base = group.base_index.unwrap_or(0);
        for (i, n) in list.iter().enumerate() {
            let idx = i as u32 + base;
            let mapped = match rc {
                RegClass::GPR(_) => Some(idx),
                RegClass::FPR(_) | RegClass::VEC(_) => Some(n_gpr + idx),
                // 掩码寄存器不参与 ABI（不是参数/返回值寄存器）。
                RegClass::KReg(_) => None,
            };
            match mapped {
                Some(m) if m < total => {
                    names.entry(n.clone()).or_insert(m);
                }
                // 越界别名（x86 的 ZMM16-ZMM31：AVX-512 有 32 个向量寄存器，
                // 而本视图的主 FPR 视图只有 16 个）——不假装它们存在，
                // 但把**全部**越界项收成一条说明，免得报告刷屏。
                Some(_) => dropped.push(n.clone()),
                None => {}
            }
        }
    }
    if !dropped.is_empty() {
        notes.push(format!(
            "{} 个别名超出本视图的 {} 个物理寄存器，不入名字表：{}（如 x86 的 ZMM16-ZMM31——\
             主 FPR 视图只有 XMM0-15；用整数选择子或这些别名写绑定会解析失败，请用主视图名字）",
            dropped.len(),
            total,
            dropped.join(" ")
        ));
    }

    // ── 固定用途：reserved + sp/fp ──
    let mut pinned: Vec<u32> = Vec::new();
    let mut scratch: Vec<u32> = Vec::new();
    let mut link: Option<u32> = None;
    if let Some(abi) = &model.abi {
        if let Some(frame) = &abi.frame {
            for n in std::iter::once(&frame.sp).chain(frame.fp.iter()) {
                match names.get(n) {
                    Some(i) => pinned.push(*i),
                    None => notes.push(format!("[abi.frame] 的 `{n}` 不在寄存器表里")),
                }
            }
        }
        for n in &abi.reserved {
            match names.get(n) {
                Some(i) => pinned.push(*i),
                None => notes.push(format!("[abi].reserved 的 `{n}` 不在寄存器表里")),
            }
        }
        for n in &abi.scratch {
            match names.get(n) {
                Some(i) => scratch.push(*i),
                None => notes.push(format!("[abi].scratch 的 `{n}` 不在寄存器表里")),
            }
        }
        link = abi
            .call_ret_reg
            .as_ref()
            .and_then(|n| names.get(n))
            .copied();
        if let Some(n) = &abi.call_ret_reg
            && link.is_none()
        {
            notes.push(format!("[abi].call_ret_reg 的 `{n}` 不在寄存器表里"));
        }
    }
    pinned.sort_unstable();
    pinned.dedup();
    scratch.sort_unstable();
    scratch.dedup();
    for p in &pinned {
        if let Some(r) = regs.get_mut(*p as usize) {
            r.pinned = true;
        }
    }

    // ── 角色 ──
    let mut roles: BTreeMap<String, RoleInfo> = BTreeMap::new();
    for inst in &model.instructions {
        for decl in &inst.roles {
            let key = decl.role().to_string();
            let e = roles.entry(key).or_default();
            match decl.bits() {
                Some(b) => {
                    e.bits.insert(b);
                }
                None => e.widthless = true,
            }
            if e.insts.len() < MAX_ROLE_INSTS && !e.insts.contains(&inst.name) {
                e.insts.push(inst.name.clone());
            }
        }
    }

    Ok(MachineView {
        isa: model.meta.name.clone(),
        sources: spec.sources.clone(),
        regs,
        names,
        pinned,
        scratch,
        link,
        n_gpr,
        n_fp,
        roles,
        notes,
    })
}

fn group_names_of(model: &V12Model, rc: RegClass) -> Result<Vec<String>, String> {
    let g = model.reg.get(&rc).ok_or_else(|| {
        format!(
            "谱里没有 [reg.gpr{}]/[reg.fpr{}] 组（{rc:?}）",
            rc.width(),
            rc.width()
        )
    })?;
    group_names(g)
}

/// 角色 → 引擎需要的能力名（`None` = 该角色不参与 ABI 的寄存器搬运）。
pub fn role_capability(role: Role) -> Option<&'static str> {
    Some(match role {
        Role::GprMov | Role::RetMov => "gpr_mov",
        Role::FprMov => "fpr_mov",
        Role::VecMov => "vec_mov",
        Role::FrameAlloc | Role::FrameFree => "sp_adjust",
        Role::StackArgLoad => "stack_arg_load",
        Role::StackArgStore => "stack_arg_store",
        Role::FrameAddr => "frame_addr",
        Role::WideVecLoad | Role::WideVecStore => "wide_vec_move",
        Role::Call => "call",
        Role::CallIndirect => "call_indirect",
        Role::Ret => "ret",
        // 控制流/保存指令不是"约定要求的能力"：push/pop 由机制推导，
        // jump/branch/test/epilogue_jump 与参数传递无关。
        Role::Push | Role::Pop | Role::Jump | Role::Branch | Role::Test | Role::EpilogueJump => {
            return None;
        }
    })
}

/// 这类视图里"声明了哪些能力"（能力名 → 位宽），给 CLI 与宿主适配器用。
///
/// **无宽度语义**的角色（`roles = ["gpr_mov"]`）在引擎侧是"声明了，宽度按目标"，
/// 这里按**地址宽**折算成位（x86 = 64）——引擎的能力查询需要的是"能不能做 + 做多宽"。
pub fn declared_capabilities(view: &MachineView) -> BTreeMap<&'static str, u16> {
    let default_bits = (view.regs.first().map(|r| r.width).unwrap_or(8) as u16) * 8;
    let mut out: BTreeMap<&'static str, u16> = BTreeMap::new();
    for (role_name, info) in &view.roles {
        let Some(role) = role_from_name(role_name) else {
            continue;
        };
        let Some(cap) = role_capability(role) else {
            continue;
        };
        let bits = info
            .bits
            .iter()
            .next_back()
            .copied()
            .unwrap_or(default_bits);
        let e = out.entry(cap).or_insert(bits);
        *e = (*e).max(bits);
    }
    out
}

/// 角色名（`Display` 的 snake_case）→ `Role`。
pub fn role_from_name(name: &str) -> Option<Role> {
    Some(match name {
        "gpr_mov" => Role::GprMov,
        "ret_mov" => Role::RetMov,
        "fpr_mov" => Role::FprMov,
        "vec_mov" => Role::VecMov,
        "call" => Role::Call,
        "call_indirect" => Role::CallIndirect,
        "ret" => Role::Ret,
        "jump" => Role::Jump,
        "branch" => Role::Branch,
        "test" => Role::Test,
        "push" => Role::Push,
        "pop" => Role::Pop,
        "frame_alloc" => Role::FrameAlloc,
        "frame_free" => Role::FrameFree,
        "epilogue_jump" => Role::EpilogueJump,
        "wide_vec_store" => Role::WideVecStore,
        "wide_vec_load" => Role::WideVecLoad,
        "frame_addr" => Role::FrameAddr,
        "stack_arg_load" => Role::StackArgLoad,
        "stack_arg_store" => Role::StackArgStore,
        _ => return None,
    })
}
