#![cfg(test)]

//! v19 V3d：**谱内派生枚举器的全指令编解码往返**守卫。
//!
//! 生成物里 `__spec_tests::all_insts()` 由**谱派生**（每条指令 × 宽度视图 + 立即数边界 +
//! 内存风味，取值规则与生成期自测同源），因此本文件是"宿主侧全指令往返"的**唯一**实现——
//! 历史上 x86 手抄过一份 650 行的 `all_insts()`，谱加指令时它不会自动跟上（漏测）。
//!
//! 判据两条：
//!
//! 1. **覆盖清点**：`SPEC_INSTS` 里的每条指令都要在枚举器里出现（派生逻辑漏了哪条就报哪条）；
//! 2. **闭环**：`encode(x) → decode → encode` 字节稳定，且 `decode` 必须吃满整条字节。
//!
//! 不做的事：不求 `decode(encode(x)) == x`（别名撞车时按声明序首匹配，本就不成立——
//! 那条断言仍由各 ISA 测试里的"规范指令"小清单守着）。

/// 单个 ISA 的往返检查（宏：三个 ISA 的 `Inst`/`encode`/`decode` 是各自生成模块的类型）。
macro_rules! roundtrip_of {
    ($isa:literal, $m:path) => {{
        use $m as isa;
        use isa::{decode, encode};
        let all = isa::__spec_tests::all_insts();
        let names = isa::__spec_tests::SPEC_INSTS;
        assert!(!all.is_empty(), "{}: 派生枚举器为空", $isa);
        for name in names {
            let name: &str = name;
            let hit = all.iter().any(|(label, _)| {
                *label == name || label.strip_prefix(name).is_some_and(|r| r.starts_with('['))
            });
            assert!(hit, "{}: 指令 '{name}' 不在派生枚举器里", $isa);
        }
        let mut checked = 0usize;
        for (label, inst) in &all {
            let b = encode(inst)
                .unwrap_or_else(|e| panic!("{} 编码 {label}（{inst:?}）失败：{e}", $isa));
            let (dec, n) =
                decode(&b).unwrap_or_else(|| panic!("{} 解码 {label}（{b:02x?}）失败", $isa));
            assert_eq!(
                n,
                b.len(),
                "{} {label}: 解码消费 {n} != {} 字节",
                $isa,
                b.len()
            );
            let b2 = encode(&dec).unwrap_or_else(|e| panic!("{} 重编码 {label} 失败：{e}", $isa));
            assert_eq!(b2, b, "{} {label}: decode→encode 往返字节不一致", $isa);
            checked += 1;
        }
        checked
    }};
}

/// 三份发行谱：谱里每条指令（含宽度视图/立即数边界/内存风味）编解码闭环。
///
/// 下界钉住的是**枚举器不许退化**：指令总数见 `spec_coverage_guard`（x86 276 /
/// riscv64 139 / arm64 206），枚举器条目只会更多（视图与风味），少于总数就是派生漏了。
#[test]
fn derived_insts_roundtrip_byte_stable() {
    let x86 = roundtrip_of!("x86", crate::arch::x86::x86);
    let riscv = roundtrip_of!("riscv64", crate::arch::riscv64::riscv64);
    let arm64 = roundtrip_of!("arm64", crate::arch::arm64::arm64);
    assert!(x86 >= 276, "x86 枚举器条目 {x86} < 指令总数 276");
    assert!(riscv >= 139, "riscv64 枚举器条目 {riscv} < 指令总数 139");
    assert!(arm64 >= 206, "arm64 枚举器条目 {arm64} < 指令总数 206");
    // 实测条目数（2026-09-24）：x86 **602** / riscv64 **327** / arm64 **360**——
    // 远多于指令总数（当前 261/119/110），因为每条指令还带宽度视图、立即数边界与内存风味。
    // 数字变了 ⇒ 谱的指令/操作数风味变了（或派生逻辑改了），人工复核后同步本行。
    // 2026-10-01：riscv 327 → 329（补 FSGNJ_D，三个 fpr 槽各带宽度视图）。
    // 2026-10-03：riscv 329 → 341（补 LWU/ADDIW）；x86 602 → 819（补 ALU 立即数/移位/ret 族 26 条、删掉 2 条重复的 64 位角色指令）。
    // 2026-10-03（v20 V10）：x86 819 → 1075（内存形式 ALU 16 条，每条带 gprx 的
    //   2/4/8 字节三个宽度视图 + 高低寄存器视图 + 内存的 disp/index 风味）；
    //   1075 → **1251**（8 位 ALU 24 条，每条带高低寄存器视图 + 内存/立即数风味）；
    //   1251 → **1263**（一元 `inc`/`dec` 4 条：`INCDEC_RM` 带三个宽度视图 + 高视图，
    //   `INCDEC_RM_8` 带一个高视图）。
    // 2026-10-04（v20 V10 第七批）：1263 → **1327**（内存形式的 mov 族 6 条：
    //   `MOV_R_MEM_AUTO`/`STORE_MEM_R_AUTO` 带 `gpr24` 的 2/4 字节两个宽度视图 + 高低
    //   寄存器视图 + 内存的 disp/index 风味；`MOV_R_MEM_8_AUTO`/`STORE_MEM_R_8_AUTO`
    //   同理带一个高低视图；`MOVSXD_R_MEM`/`XCHG_MEM_R_AUTO` 各带自己的视图与风味）。
    // 2026-10-04（v20 V10，第十一批）：riscv64 383 → **435**（`BGEU` + `FENCE_I`/`FENCE_TSO` 与
    //   Zicsr 六条：各带宽度视图 + 立即数边界风味）。
    // 2026-10-04（v20 V10，第十批）：riscv64 341 → **383**（字节/半字访存 6 条 × 两个视图 +
    //   内存/立即数风味；W 立即数移位 3 条；`SLLW`/`SRLW`/`SRAW` 从立即数形态改成 R 型，
    //   视图数变化而已——指令名与总数不变）。
    // 2026-10-04（v20 V10 第八批）：1327 → **1351**（SSE 比较谓词 4 条
    //   `CMPPS`/`CMPPD`/`CMPSS`/`CMPSD_SCALAR`：各带 fpr 槽的宽度视图 + 立即数边界风味；
    //   16 条谓词别名是 `[[pseudo]]` 文本展开，不进枚举器）。
    // 2026-10-04（v20 V10 第十二批）：arm64 386 → **778**（逻辑族补齐 68 条 = 8 助记符 ×
    //   4 移位 × X/W（64 条带移位后缀）+ `ands`/`bics` 的无后缀形态 4 条：带移位的每条
    //   3 reg 槽 × 2 视图 × (1 基线 + 2 立即数边界) = 6 条，无后缀的每条 2 视图 × 1 = 2 条，
    //   64 × 6 + 4 × 2 = 392）。
    // 2026-10-04（v20 V10 第十四批）：arm64 778 → **802**（`TBZ`/`TBNZ` × X/W 4 条：每条
    //   2 个视图 × (1 基线 + 位序号槽的 2 个立即数边界) = 6；label 槽不进边界风味）。
    // 2026-10-04（v20 V10 第十五批）：arm64 802 → **818**（逻辑立即数 8 条：每条 2 个视图 ×
    //   1 基线——非线性方案的可编码集不是区间，不生成 lo/hi 边界风味）。
    // 2026-10-04（v20 V10 第十六批）：arm64 818 → **825**（`RETR` 2 + `B_AL`/`B_NV` 各 1 +
    //   裸 `DCPS1B`/`DCPS2B`/`DCPS3B` 各 1）。
    // 2026-10-04（v20 V10 第十七批）：riscv64 435 → **442**（`JALR3` 6 条 = 2 视图 ×
    //   (1 基线 + imm12 两个边界)；`FENCE_PS` 1 条——位集合槽不进边界风味）。
    // 2026-10-05（APX/REX2 批次）：x86 1351 → **1354**（新增 16/32 位 MR 形态 `MOV_RM_R_24`
    //   1 条：`gpr24` 两个宽度视图 × 1 基线 + 1 高编号寄存器视图 = 3 条）。
    // 2026-10-05（APX 语料批次）：x86 1354 → **1426**（14 条新指令；主体是 8 条一元内存形态
    //   ——每条带内存的 disp/index 三个风味 = 4 条，8 × 4 = 32；`MUL_RM`/`IMUL_RM`/
    //   `MOVSXD_R_RM32` 带 `gprx`/`gpr4` 的宽度与高视图；`IMUL_R_MEM`/`NOP_MEM32` 带风味）。
    // 2026-10-05（助记符条件后缀批次）：1426 → **1450**（`SETCC_RM8_B` / `SETCC_R_MEM` /
    //   `CMOVCC_R_MEM` 三条的视图与内存风味；`CMOVCC_R_RM` 放宽到 `gprx` 后多出 16/32 位视图）。
    // 2026-10-05（movzx/movsx 内存源批次）：1482 → **1542**（4 条完整内存形态的宽度视图 + 内存风味；
    //   简写形态的基址槽收窄成单一 64 位地址类 ⇒ 少了无意义的宽度视图）。
    // 2026-10-05（锁 + 内存序提示批次）：1450 → **1482**（两条 `ACQUIRE/RELEASE_LOCK_ADD_MR`：
    //   `gprx` 的宽度/高视图 + 内存的 disp/index 风味）。
    // 2026-10-07（riscv64 浮点 rm 批次）：riscv64 442 → **622**（浮点算术与 `fcvt` 族
    //   各多一条"显式 rm"形态：`R_RM`/`R_RM2` 的 `rm3` 槽按名字表采样 ⇒ 每族多 6 个视图；
    //   同时删掉 6 条固定 `rtz` 的旧 `FCVT_*_RTZ`——它们已被 rm 操作数覆盖）。
    // 2026-10-07（aarch64 访存单位批次）：arm64 825 → **841**（8 条无位移形态：X/W 各 4 条，
    //   每条 2 个视图——`imm12s`/`imm7` 是固定 0，不进值边界风味）。
    // 2026-10-07（aarch64 字节/半字访存批次）：arm64 1433 → **1505**（18 条：imm12 的边界风味 + 宽度视图）。
    // 2026-10-07（aarch64 前后索引批次）：arm64 841 → **937**（16 条前后索引形态：单寄存器
    //   8 条各带 `imm9s` 的两个边界风味 + 宽度视图，pair 8 条各 2 视图）。
    // 2026-10-07（aarch64 寄存器偏移批次）：arm64 937 → **1049**（32 条寄存器偏移：索引槽按
    //   宽度取视图 + `xzr`/`wzr` 别名；8 条 ADD/SUB(S) 带量：`imm6`/`imm5` 两个边界风味）。
    // 2026-10-07（aarch64 扩展寄存器批次）：arm64 1049 → **1433**（48 条：每条的 `ext` 槽按
    //   命名表逐个名字采样 + `amt3` 的边界风味，带量/省略量各一套）。
    // 2026-10-07（aarch64 重定位修饰批次）：arm64 1505 → **1577**（12 条：`imm12sym` 的边界风味 +
    //   宽度视图；`require_symbol` 形态跳过文本闭环，只测编解码闭环）。
    assert_eq!(
        (x86, riscv, arm64),
        (1542, 622, 2359),
        "枚举器条目数变了：确认是谱的预期变更还是派生逻辑退化"
    );
}
