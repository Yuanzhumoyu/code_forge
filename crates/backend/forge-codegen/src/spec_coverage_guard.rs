//! v18 S6 覆盖率守卫：把三条发行 ISA **生成期自测**的覆盖率与"文本歧义"名单钉死。
//!
//! 生成的自测（`arch::<isa>::<isa>::__spec_tests`）随 TOML 自动更新，因此它本身
//! 不需要维护；但**它是否还在覆盖全部指令**需要外部核对——本文件是那个"外部"：
//!
//! 1. `SPEC_COVERED + SPEC_SKIPPED == SPEC_TOTAL`（生成端自检的镜像）；
//! 2. **零跳过**：`SPEC_SKIPPED` 必须为空（S6 判据 = 全指令覆盖）；
//! 3. **指令总数钉死**：数字变了就说明指令谱动了——要么同步更新这里，要么把
//!    误删/误加的指令查清楚（防止"指令悄悄消失，自测跟着少跑"）；
//! 4. **文本歧义名单钉死**：名单上的指令"同名同形、编码不同"，自测对它们放宽为
//!    "文本幂等 + 自洽"（不要求回到同一字节）。名单增长 = 有指令新变成文本分不清的
//!    状态（可能是真加了歧义，也可能是新指令与老指令撞了 asm 文本）——必须显式确认，
//!    否则"强断言"会静默消失。

#![cfg(test)]

/// 单个 ISA 的自测覆盖报告。
struct Report {
    name: &'static str,
    total: usize,
    covered: usize,
    skipped: Vec<&'static str>,
    ambiguous: Vec<&'static str>,
}

macro_rules! report_of {
    ($name:literal, $path:path) => {{
        use $path as spec;
        Report {
            name: $name,
            total: spec::SPEC_TOTAL,
            covered: spec::SPEC_COVERED,
            skipped: spec::SPEC_SKIPPED.iter().map(|(n, _)| *n).collect(),
            ambiguous: spec::SPEC_TEXT_AMBIGUOUS.to_vec(),
        }
    }};
}

fn reports() -> Vec<Report> {
    vec![
        report_of!("x86", crate::arch::x86::x86::__spec_tests),
        report_of!("riscv64", crate::arch::riscv64::riscv64::__spec_tests),
        report_of!("arm64", crate::arch::arm64::arm64::__spec_tests),
    ]
}

/// 生成端自检的镜像 + **零跳过**（S6 的核心判据）。
#[test]
fn generated_spec_tests_cover_every_instruction() {
    for r in reports() {
        assert!(r.total > 0, "{}: 指令总数为 0", r.name);
        assert_eq!(
            r.covered + r.skipped.len(),
            r.total,
            "{}: 覆盖计数与总数不符",
            r.name
        );
        assert!(
            r.skipped.is_empty(),
            "{}: {} 条指令没能自动构造操作数：{:?}",
            r.name,
            r.skipped.len(),
            r.skipped
        );
    }
}

/// 指令总数钉死（2026-09-20 实测；arm64 自 S3c 加 `b.cond` 16 行模板后 = 104，v20 A5 补浮点搬运 = 110；
/// riscv64 v20 V6+ 补 `fpr_mov` 的双精度档 `fsgnj.d` 后 = 117。
/// x86 v20 V10 补内存形式的 ALU 族（`*_MR` / `*_R_MEM` 各 8 条 → 237）、8 位 ALU 族
/// （`*_MR_8` / `*_R_MEM_8` / `*_RM8_IMM8` 各 8 条 → 261）、一元 `inc`/`dec`
/// （`INCDEC_RM` / `INCDEC_RM_8` 各 2 行 → 265）与**内存形式的 mov 族**
/// （`MOV_R_MEM{,_8}_AUTO` / `STORE_MEM_R{,_8}_AUTO` / `MOVSXD_R_MEM` / `XCHG_MEM_R_AUTO`
/// 6 条 → **271**：8/16/32 位 mov 全部走完整内存模板 + movsxd/xchg 的内存源形式）
/// 与 **SSE 比较谓词族**（`CMPPS`/`CMPPD`/`CMPSS`/`CMPSD_SCALAR` 4 条 → **275**；经典八谓词
/// 的 packed 别名是 `[[pseudo]]` 文本展开，不占指令数）后 = 275。
/// riscv64 补**字节/半字访存**（`LB`/`LH`/`LBU`/`LHU`/`SB`/`SH` 6 条）与 **W 立即数移位**
/// （`SLLIW`/`SRLIW`/`SRAIW` 3 条；`SLLW`/`SRLW`/`SRAW` 改成 R 型并进 `WW` 模板行，总数不变）
/// 后 = 128；再补 `BGEU`（`BB` 模板一行）、`FENCE_I`/`FENCE_TSO` 与 **Zicsr 六条**
/// （`CSRRW`/`CSRRS`/`CSRRC`/`CSRRWI`/`CSRRSI`/`CSRRCI`）→ **137**。`unimp` 是 `[[pseudo]]`
/// （别名 = `csrrw x0, cycle, x0`），不占指令数）。arm64 补**系统/异常生成族**
/// （`SVC`/`HVC`/`SMC`/`BRK`/`HLT`/`DCPS1..3` 8 条 + `ERET`/`DRPS` 两个整字常量）→ **120**。
/// arm64 再补**逻辑（移位寄存器）族**：带移位后缀的 8 助记符（`and`/`ands`/`bic`/`bics`/
/// `orr`/`orn`/`eor`/`eon`）× 4 种移位（`lsl`/`lsr`/`asr`/`ror`）× X/W = 64 条（移位种类
/// 是 2 位常量、移位量是操作数槽），外加 `ands`/`bics` 的**无后缀**形态 4 条 → **188**；
/// 再补 `TBZ`/`TBNZ` × X/W 4 条（位序号一个操作数摊到 `b40`+`b5` 两个位域）→ **192**；
/// 再补**逻辑（立即数）族**（`and`/`orr`/`eor`/`ands` × X/W = 8 条，值 → N/immr/imms 三分量）→ **200**；
/// 再补 `ret xN`（`RETR`）、`b.al`/`b.nv`（A64 保留码，上游汇编器收）与裸 `dcps1/2/3`（= `dcpsN #0`）6 条 → **206**）。
/// `XZR`/`WZR`/`LR` 是 `[reg.*].aliases`（解析认、渲染出主名），不占指令数。
/// riscv64 再补**三操作数 `jalr rd, rs1, imm`**（`JALR3`，与两操作数写法同编码）与
/// **`fence pred, succ`**（`FENCE_PS`，pred/succ 是字母集合 → 新的 `kind = "bits"`）→ **139**。
/// x86 补 **APX（REX2）能力**：四个 GPR 宽度组扩到 32 项（r16..r31 四个视图，编码器见到
/// ≥16 就改发 REX2），并补上 **16/32 位的 `89`（MR）形态** `MOV_RM_R_24`（上游 LLVM 对
/// reg-reg 用 MR 形态；我们原先只给 64 位建了它 ⇒ 那类上游编码我们**解不回来**）→ **276**。
/// 再补 **APX 语料用到的真实形态**（14 条）：一元族的**内存形态** 8 条
/// （`{NOT,NEG,INC,DEC,MUL,IMUL,DIV,IDIV}_MEM32`——一元形态没有寄存器驱动宽度，模板里
/// 写死 `dword ptr`）、一元 `MUL_RM`/`IMUL_RM`（`F7 /4,/5`）、两操作数 `IMUL_R_MEM`
/// （`0F AF` + 内存源）、`NOP_RM`/`NOP_MEM32`（`0F 1F /0`）与 `MOVSXD_R_RM32`
/// （真实汇编的 32 位源写法，与 lowering 用的 64 位源那条同编码）→ **290**。
/// 最后补**助记符条件后缀**（`sete`/`cmovl`）需要的三条通用形态的兄弟指令：
/// `SETCC_RM8_B`（8 位名 `setcc al, e`）、`SETCC_R_MEM`（`setcc byte ptr [m], e`）与
/// `CMOVCC_R_MEM`（`cmovcc r, m, cc`；`CMOVCC_R_RM` 同时从固定 64 位放宽到 16/32/64）
/// → **293**。真实写法本身是 `[[pseudo]]` 文本展开（不占指令数）。
/// 再补 `movzx`/`movsx` 的**完整内存源形态** 4 条（`MOVZX/MOVSX_MEM{8,16}`：源宽度只能由
/// 尺寸关键字给 ⇒ 关键字写成模板字面量，歧义随之消失）→ **299**。
/// 最后补**锁 + 内存序提示**（`acquire/release lock add [mem], r` = `F2`/`F3` + `F0`：
/// `prefix` 收**列表**，编码按序发、解码逐个判前缀标志）→ **295**。
/// 2026-10-07（riscv64 浮点 rm 批次）：riscv64 139 → **173**——浮点算术 5 族各加"显式 rm"形态
/// （`FADD/FSUB/FMUL/FDIV/FSQRT` × S/D = 10 条），`fcvt` 族 18 条改成"rm 操作数 + 省略 rm(=dyn)"
/// 两条并列（36 条），删掉 6 条固定 `rtz` 的旧声明与 6 条把省略写法写死 `rne` 的声明。
/// 2026-10-07（aarch64 访存单位批次）：arm64 206 → **214**——LDR/STR/LDP/STP 各加一条
/// **无位移**形态（`ldr x4, [x3]`，编码上就是位移为 0 的那一格；`ldp`/`stp` 同理）。
/// 2026-10-07（aarch64 前后索引批次）：arm64 214 → **230**——单寄存器 4 宽 × 前/后索引
/// （`ldr x0, [x1, #16]!` / `ldr x0, [x1], #16`）+ pair 4 宽 × 前/后索引共 16 条。
/// 2026-10-07（aarch64 寄存器偏移批次）：arm64 230 → **270**——寄存器偏移 32 条
/// （X/W × ldr/str × 8 种 option/S 组合）+ ADD/SUB(S) 移位寄存器带量 8 条。
/// 2026-10-07（aarch64 扩展寄存器批次）：arm64 270 → **318**——扩展寄存器 48 条
/// （6 助记符 × 2 宽度 × {W,X} 扩展 × {带量, 省略量}）。
/// 2026-10-07（aarch64 字节/半字访存批次）：arm64 318 → **336**——字节/半字 GPR 访存 18 条
/// （9 组助记符/类别 × {带位移, 无位移}）；`op8` 是无符号偏移族的 `0x39`/`0x79`/`0xB9`，
/// 这处写错正是被全集语料的字节对拍档抓出来的。
/// 2026-10-07（aarch64 重定位修饰批次）：arm64 336 → **348**——ALU 立即数 8 条
/// （`add`/`sub`/`adds`/`subs` × X/W）+ LDR/STR 4 条，槽是 `imm12sym`（unit 1 + symbols +
/// **require_symbol**：只收符号/带修饰的写法，否则数值写法会被它抢走）。
#[test]
fn spec_coverage_totals_are_pinned() {
    let totals: Vec<(&str, usize)> = reports().iter().map(|r| (r.name, r.total)).collect();
    assert_eq!(
        totals,
        vec![("x86", 299), ("riscv64", 173), ("arm64", 508)],
        "指令总数变了：确认是谱的预期变更还是指令丢失"
    );
}

/// 文本歧义名单钉死（更新前先跑 `print_spec_coverage_report` 看当前值）。
///
/// v20 V10 起 x86 少了 6 条：`MOV64_{MR,RM}` / `MOVSD_{MR,RM}` / `MOVUPS_{MR,RM}`——
/// 它们与「反方向」的同名指令（`mov {mem}, {src}` 对 `mov {dst}, {mem}`）**文本并不同**
/// （一个是 `mov [RAX], RBX`，一个是 `mov RAX, [RBX]`），此前被歧义键按"操作数声明序"
/// 建键误判成同形。键改成"按模板占位符序 + 不带操作数序号"后，这几条恢复**强断言**
/// （`disasm → asm → encode` 必须回到同一字节），实测全部通过。
///
/// APX 批次多的一条 `MOV_RM_R_24`：16/32 位 reg-reg `mov` 的 **MR 形态**（`89`）与
/// `MOV_R_RM`（`8B`）文本都是 `mov A, B`——**同一条指令的两种合法编码**，汇编器按
/// "更具体的槽签名优先"选 `89`（与上游 LLVM 一致），`8B` 那条留给解码。这正是已有的
/// `MOV_RM_R`/`MOV_R_RM`（64 位）与 `MOV64_RR` 那一类的格局。
#[test]
fn spec_text_ambiguity_lists_are_pinned() {
    let x86 = [
        "ADC_R_IMM32",
        "ADC_R_IMM8S",
        "ADD_R_IMM32",
        "ADD_R_IMM8S",
        "AND_R_IMM32",
        "AND_R_IMM8S",
        "CMP_R_IMM32",
        "CMP_R_IMM8S",
        "LEA_R64_SIB",
        "LEA_RBP_OFF",
        "MOV64_RR",
        "MOVABS_GLOBAL",
        "MOVQ_FREG_XMM",
        "MOVQ_XMM_FREG",
        "MOVSD",
        "MOVSD_XMM_FREG",
        "MOV_R8_RM64",
        "MOV_REG_IMM64",
        "MOV_RM8_R64",
        "MOV_RM_R",
        "MOV_RM_R_24",
        "MOV_R_RM",
        "OR_R_IMM32",
        "OR_R_IMM8S",
        "SBB_R_IMM32",
        "SBB_R_IMM8S",
        "SUB_R_IMM32",
        "SUB_R_IMM8S",
        "VADDPS_ZMM_MASK",
        "VADDPS_ZMM_MASKZ",
        "VMOVUPS_MR",
        "VMOVUPS_RM",
        "VMOVUPS_ZMM_MEM",
        "VMOVUPS_ZMM_MR",
        "XOR_R_IMM32",
        "XOR_R_IMM8S",
    ];
    let riscv = ["ADDI", "ADDI_GLOBAL", "AUIPC", "AUIPC_GLOBAL"];
    let pinned: &[(&str, &[&str])] = &[
        ("x86", &x86),
        ("riscv64", &riscv),
        // v20 A5：arm64 的 FP 寄存器组用统一的 `V0..V31` 命名（不像 x86 那样 S/D 名字本身带宽度），
        // 因此 `fmov v0, v1` / `ldur v0, [x29, #8]` 的 S/D 两种编码**汇编文本相同**——
        // 反汇编按声明序取第一条（S），文本往返对这两族只能取其一。这是**已知且刻意**的
        // 取舍（宽度在指令里、不在名字里）；要消掉就得给 S/D/Q 各开一组别名寄存器。
        (
            "arm64",
            &["FMOV_D", "FMOV_S", "LDURD", "LDURS", "MOVKW_G0_RELO", "MOVKW_G1_RELO", "MOVKW_G2_RELO", "MOVKW_G3_RELO", "MOVKX_G0_RELO", "MOVKX_G1_RELO", "MOVKX_G2_RELO", "MOVKX_G3_RELO", "MOVNW_G0_RELO", "MOVNW_G1_RELO", "MOVNW_G2_RELO", "MOVNW_G3_RELO", "MOVNX_G0_RELO", "MOVNX_G1_RELO", "MOVNX_G2_RELO", "MOVNX_G3_RELO", "MOVWIMM_SYM_G0", "MOVWIMM_SYM_G1", "MOVWIMM_SYM_G2", "MOVWIMM_SYM_G3", "MOVXIMM_SYM_G0", "MOVXIMM_SYM_G1", "MOVXIMM_SYM_G2", "MOVXIMM_SYM_G3", "MOVZW_G0_RELO", "MOVZW_G1_RELO", "MOVZW_G2_RELO", "MOVZW_G3_RELO", "MOVZX_G0_RELO", "MOVZX_G1_RELO", "MOVZX_G2_RELO", "MOVZX_G3_RELO", "STURD", "STURS"],
        ),
    ];
    for r in reports() {
        let want = pinned
            .iter()
            .find(|(n, _)| *n == r.name)
            .map(|(_, l)| l.to_vec())
            .unwrap_or_default();
        let mut got = r.ambiguous.clone();
        got.sort_unstable();
        let mut want_sorted = want;
        want_sorted.sort_unstable();
        assert_eq!(
            got, want_sorted,
            "{}: 文本歧义名单变了——确认是否真有新歧义（若只是谱更名，同步本名单）",
            r.name
        );
    }
}

/// 打印当前报告（`--nocapture` 可见）：更新上面的钉死值前先看这里。
#[test]
fn print_spec_coverage_report() {
    for r in reports() {
        eprintln!(
            "SPEC-COVERAGE {}: total={} covered={} skipped={:?} ambiguous={} {:?}",
            r.name,
            r.total,
            r.covered,
            r.skipped,
            r.ambiguous.len(),
            r.ambiguous
        );
    }
}
