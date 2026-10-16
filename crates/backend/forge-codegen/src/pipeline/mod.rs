/// 宿主 AbiTarget 适配器（真实后端 → forge-abi 引擎；v20 A3）。
pub mod abi_setup;
pub mod abi_target;
pub mod agg_const;
pub mod agg_expand;
pub mod alloc_config;
pub mod compiler;
/// 宿主调用约定注册表（IR 的 `CallConvId` → 注册名；未注册即 fail-closed）。
pub mod conv_registry;
pub mod emission;
pub mod frame_layout;
pub mod frame_slots;
/// **能力缺口台账的代码侧锚点**（v24 M3）：`capability(ID, msg)` ⇒ 消息带 `[cap:ID]`，
/// 让"这台机器/这份约定/这份谱做不了什么"成为**可机读的集合**（守卫与台账逐一相等）。
pub mod legalize;
pub mod liverange;
pub mod lowering;
pub mod regalloc_bt;
/// 模块级编译编排：`CompiledModule` + 整模块/单函数入口（M1：唯一编译入口）。
pub mod session;
/// IR 视图 → 引擎视图（签名 + 参数属性；v20 A2b）。
pub mod sig_view;
/// **观测开关台账**（v24 M4）：诊断开关不可能静默失效（见模块文档）。
pub mod trace;
pub mod va_expand;

/// **管线阶段清单**（v24 M5 的契约锚点）。
///
/// 契约文档 `docs/reference/backend-pipeline.md` 的"管线阶段"表由守卫
/// `pipeline_stages_match_doc` 与本表**集合相等**钉住：改了阶段就同时改表（多一个、
/// 少一个都红）。id 用短名，说明写给"读契约的人"。
pub const STAGES: &[(&str, &str)] = &[
    (
        "legalize",
        "合法化（**两相**：计划相只读算计划 → 物化相按指令序单遍改写）",
    ),
    (
        "isel",
        "指令选择（lowering：生成物的 `lower_inst` + 宿主的辅助发射）",
    ),
    ("regalloc", "寄存器分配（活跃区间 + 溢出与回填）"),
    ("frame", "帧布局（槽位/对齐/帧尺寸/ABI 槽）"),
    ("emit", "发射（序言 / 指令 / 尾声 / fixup 解析）"),
    ("jit", "装载（JIT：可执行内存 + 符号解析）"),
];

pub use forge_isa_runtime::{alloc_result, emit, vcode};

#[cfg(test)]
mod stage_contract_tests {
    use super::*;

    fn doc() -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("..");
        let p = root.join("docs/reference/backend-pipeline.md");
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("读不到 {}：{e}", p.display()))
    }

    /// 取某个小节里"第一列是反引号名字"的表格行的名字（按出现序）。
    fn table_ids(sec: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in sec.lines() {
            let l = line.trim();
            if !l.starts_with("| `") {
                continue;
            }
            let cell = l.trim_matches('|').split('|').next().unwrap_or("").trim();
            let name = cell.trim_matches('`');
            // 表头分隔行/其它表格过滤掉：名字必须是 ASCII 标识符
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                out.push(name.to_string());
            }
        }
        out
    }

    /// 文档里从 `## <标题关键词>` 到下一个 `## ` 的片段。
    fn section(text: &str, heading_contains: &str) -> String {
        let mut out = String::new();
        let mut inside = false;
        for line in text.lines() {
            if line.starts_with("## ") {
                inside = line.contains(heading_contains);
                continue;
            }
            if inside {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    /// **阶段表 ↔ 代码阶段清单**（`STAGES`）：双向相等（多一个少一个都红）。
    #[test]
    fn pipeline_stages_match_doc() {
        let text = doc();
        let ids = table_ids(&section(&text, "管线阶段"));
        let code: Vec<String> = STAGES.iter().map(|(id, _)| (*id).to_string()).collect();
        assert!(!code.is_empty(), "`STAGES` 为空");
        let doc_set: std::collections::BTreeSet<&String> = ids.iter().collect();
        let code_set: std::collections::BTreeSet<&String> = code.iter().collect();
        assert_eq!(
            doc_set, code_set,
            "契约文档的阶段表与 `pipeline::STAGES` 不一致：\n  文档 {ids:?}\n  代码 {code:?}"
        );
    }

    /// **合法化 pass 表 ↔ `LegalizeDriver::with_aggregates().pass_names()`**（含次序）。
    #[test]
    fn legalize_passes_match_doc() {
        let text = doc();
        let ids = table_ids(&section(&text, "合法化"));
        let d = legalize::LegalizeDriver::<crate::arch::x86::TargetMachine>::with_aggregates();
        let code = d.pass_names();
        assert_eq!(
            ids, code,
            "契约文档的合法化 pass 表与驱动的合并次序不一致（顺序也要一致）"
        );
    }
}
