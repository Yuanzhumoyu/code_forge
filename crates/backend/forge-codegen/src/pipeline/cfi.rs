//! x86_64 prologue → DWARF `.debug_frame` CFI 行（M2）。
//!
//! gdb 16.2（amd64-windows）解栈先试 SEH（.pdata），无条目则落到
//! dwarf2-frame 解码 `.debug_frame`（无 SEH + 无 CFI = bt / info args /
//! info locals 全空的根因，实证）。本模块从**发射完成的机器码字节**扫描
//! forge x86_64 v12 的固定 prologue，产出函数级 CFI 行集，供 forge-rustc
//! 的 dwarf.rs 生成 `.debug_frame`（FDE 每函数一个）。
//!
//! # v1 识别范围（安全退化）
//!
//! 仅识别 forge x86_64 v12 的常量 prologue 前缀（15 字节）：
//! `55 48 89 e5`（push rbp; mov rbp, rsp）+ 7 次 callee-saved push：
//! RBX RDI RSI R12 R13 R14 R15（`53 57 56 41 54 41 55 41 56 41 57`，
//! TOML [abi.callee_saved]/[abi.frame] 固定推满）。**前缀/数量不符 → None**
//! （安全退化：不产 CFI，行为等同现状——非 x86 后端/非常规函数零影响）。
//!
//! # CFA 与行语义
//!
//! - CFA = rbp + 16 全函数定帧（8 返回地址 + 8 保存 rbp），与 frame_size/
//!   栈参数无关（caller 视角在 mov rbp,rsp 后不变）。
//! - 行键 = 指令**结束**偏移（DWARF 行"该地址起生效"：push rbp @ [0,1) 的
//!   规则从 1 起生效；与 gcc FDE 行键对照一致）。
//! - 保存槽（CFA 相对，8B/槽）：rbp@-16、RBX@-24、RDI@-32、RSI@-40、
//!   R12@-48、R13@-56、R14@-64、R15@-72。
//! - x86 寄存器编码 → DWARF 列：RBX=3/RSP=4/RBP=5/RSI=6/RDI=7（ModRM 编码）
//!   → DWARF rbx=3/rsp=7/rbp=6/rsi=4/rdi=5；R12-R15（编码 12-15）→ 列 12-15。
//! - 尾声不产行（v1）：尾声 pop 只读保存槽内存（不毁槽内容），末行规则在
//!   整个函数范围持续有效——gdb 在尾声 PC 解栈仍得正确保存值。

/// 一条 CFI 规则（对应一条 DW_CFA 指令，省略行号——由行表携带）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfiOp {
    /// DW_CFA_def_cfa_offset：CFA = 当前 cfa 寄存器 + offset（字节）。
    /// x86：push rbp 后 rsp 下的 CFA = rsp + 16。
    DefCfaOffset(u32),
    /// DW_CFA_def_cfa_register：CFA = 寄存器（DWARF 列号，x86 rbp = 6）。
    DefCfaRegister(u8),
    /// DW_CFA_offset：寄存器（DWARF 列号）保存在 CFA - cfa_bytes。
    /// data_align = -8 的 FDE 里编码操作数 = cfa_bytes / 8。
    SaveReg {
        /// DWARF 寄存器列号（x86-64：rbx=3 rbp=6 rsi=4 rdi=5 r12-15=12-15）。
        dw_reg: u8,
        /// 距 CFA 的字节数（保存槽在 CFA 之下，正值）。
        cfa_bytes: u32,
    },
}

/// 一个函数的 CFI 行集：`(code_offset, 该处新增规则)`，按 offset 升序。
/// 规则累计生效——某行的规则集从此地址起持续到下一行/函数结束。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionCfi {
    /// 行表：`(机器码偏移, 规则列表)`。每函数首行 = 1（push rbp 后）。
    pub rows: Vec<(u32, Vec<CfiOp>)>,
}

/// 扫描函数机器码开头是否 forge x86_64 v12 的固定 prologue，产出 CFI 行。
///
/// 字节匹配（15 字节常量前缀，测试 `frame_prologue_bytes` 同源）：
/// `55`（push rbp）+ `48 89 e5`（mov rbp, rsp）+ `53 57 56`（push
/// rbx/rdi/rsi）+ `41 54 41 55 41 56 41 57`（push r12-r15）。此后函数体
/// 紧跟（frame_alloc/指令）——扫描在 7 次 push 后立即停止，不解析函数体。
///
/// 前缀不符 / 长度不足 / push 数 ≠ 7 → `None`（安全退化：无 CFI）。
pub fn scan_x86_prologue(code: &[u8]) -> Option<FunctionCfi> {
    if code.len() < 15 || code[0] != 0x55 || code[1..4] != [0x48, 0x89, 0xE5] {
        return None;
    }
    // push rbp @ [0,1) 结束 → CFA = rsp + 16（def_cfa_offset），rbp 保存于
    // CFA-16；mov rbp,rsp @ [1,4) 结束 → CFA = rbp + 16（def_cfa_register）。
    let mut rows: Vec<(u32, Vec<CfiOp>)> = vec![
        (
            1,
            vec![
                CfiOp::DefCfaOffset(16),
                CfiOp::SaveReg {
                    dw_reg: 6, // DWARF rbp
                    cfa_bytes: 16,
                },
            ],
        ),
        (4, vec![CfiOp::DefCfaRegister(6)]),
    ];
    // 逐 push 解析：单字节 0x50+n（n=ModRM 编码）或 41 0x50+n（R8-R15，
    // 编码 8+n）。行键 = push 结束偏移；保存槽距 CFA 逐次 8 字节下移
    //（rbx 首个 = CFA-24 = 8×3，与固定前缀字节布局一致）。
    let mut cursor = 4usize;
    let mut push_count = 0u32;
    while cursor < code.len() {
        let (enc, len) = match code[cursor] {
            0x50..=0x57 => (code[cursor] - 0x50, 1usize),
            0x41 => match code.get(cursor + 1) {
                Some(0x50..=0x57) => (code[cursor + 1] - 0x50 + 8, 2usize),
                _ => return None,
            },
            _ => break,
        };
        let Some(dw) = dw_col_for_x86_enc(enc) else {
            return None;
        };
        cursor += len;
        push_count += 1;
        if push_count > 7 {
            // 标准 prologue 恰好 7 次 push——多出的 push 破坏尾声对称/槽位
            // 假设，拒绝（安全退化）。
            return None;
        }
        rows.push((
            cursor as u32,
            vec![CfiOp::SaveReg {
                dw_reg: dw,
                cfa_bytes: 8 * (push_count + 2),
            }],
        ));
        if push_count == 7 {
            break; // 7 次 callee-saved push 完成——函数体从此开始，不再解析
        }
    }
    if push_count != 7 {
        return None;
    }
    Some(FunctionCfi { rows })
}

/// x86 寄存器 ModRM 编码 → DWARF 列号。GPR：RAX=0 RCX=1 RDX=2 RBX=3
/// RSP=7 RBP=6 RSI=4 RDI=5（DWARF 列与 ModRM 编码不同序！）；R8-R15
/// 编码 8-15 → 列号同值。向量/非 GPR 编码 → None（本前缀只推 GPR）。
fn dw_col_for_x86_enc(enc: u8) -> Option<u8> {
    Some(match enc {
        0 => 0, // rax
        1 => 1, // rcx
        2 => 2, // rdx
        3 => 3, // rbx
        4 => 7, // rsp
        5 => 6, // rbp
        6 => 4, // rsi
        7 => 5, // rdi
        8..=15 => enc, // r8-r15
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// forge x86_64 v12 常量 prologue（与 tests/v12_integration_tests.rs
    /// frame_prologue_bytes 同源）：55 48 89 e5 + 53 57 56 41 54 41 55 41 56 41 57。
    const PROLOGUE: [u8; 15] = [
        0x55, 0x48, 0x89, 0xE5, 0x53, 0x57, 0x56, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
    ];

    #[test]
    fn scan_full_prefix_rows() {
        // 15 字节完整前缀 → 行集：push rbp 后(1) CFA=rsp+16 + rbp@CFA-16；
        // mov rbp,rsp 后(4) CFA=rbp；逐 push 结束偏移(5..15) 记录保存槽。
        let cfi = scan_x86_prologue(&PROLOGUE).expect("15-byte prefix recognized");
        assert_eq!(
            cfi.rows,
            vec![
                (
                    1,
                    vec![
                        CfiOp::DefCfaOffset(16),
                        CfiOp::SaveReg {
                            dw_reg: 6,
                            cfa_bytes: 16
                        }
                    ]
                ),
                (4, vec![CfiOp::DefCfaRegister(6)]),
                (
                    5,
                    vec![CfiOp::SaveReg {
                        dw_reg: 3, // rbx
                        cfa_bytes: 24
                    }]
                ),
                (
                    6,
                    vec![CfiOp::SaveReg {
                        dw_reg: 5, // rdi
                        cfa_bytes: 32
                    }]
                ),
                (
                    7,
                    vec![CfiOp::SaveReg {
                        dw_reg: 4, // rsi
                        cfa_bytes: 40
                    }]
                ),
                (
                    9,
                    vec![CfiOp::SaveReg {
                        dw_reg: 12, // r12（41 54 两字节，结束 9）
                        cfa_bytes: 48
                    }]
                ),
                (
                    11,
                    vec![CfiOp::SaveReg {
                        dw_reg: 13,
                        cfa_bytes: 56
                    }]
                ),
                (
                    13,
                    vec![CfiOp::SaveReg {
                        dw_reg: 14,
                        cfa_bytes: 64
                    }]
                ),
                (
                    15,
                    vec![CfiOp::SaveReg {
                        dw_reg: 15,
                        cfa_bytes: 72
                    }]
                ),
            ],
            "rows: CFA 定帧 + 7 callee-saved 保存槽"
        );
        // 行按 offset 严格升序（DW_CFA_advance_loc 前提）
        let offs: Vec<u32> = cfi.rows.iter().map(|(o, _)| *o).collect();
        let mut sorted = offs.clone();
        sorted.sort_unstable();
        assert_eq!(offs, sorted, "rows ascending");
    }

    #[test]
    fn scan_with_frame_alloc_tail() {
        // prologue 后有函数体（frame_size=32 的 sub rsp 尾巴 48 81 ec 20...）
        // 不影响识别——扫描在 7 次 push 后停止。
        let mut code = PROLOGUE.to_vec();
        code.extend_from_slice(&[0x48, 0x81, 0xEC, 0x20, 0x00, 0x00, 0x00]);
        code.extend_from_slice(&[0xC3]); // ret
        let cfi = scan_x86_prologue(&code).expect("prefix + body recognized");
        assert_eq!(cfi.rows.last().unwrap().0, 15, "last row at prefix end");
    }

    #[test]
    fn scan_rejects_foreign_prefixes() {
        // 空 / 过短 → None
        assert_eq!(scan_x86_prologue(&[]), None);
        assert_eq!(scan_x86_prologue(&PROLOGUE[..14]), None);
        // 首字节非 push rbp（函数体开头/非 forge 形态）→ None
        assert_eq!(scan_x86_prologue(&[0x48, 0x83, 0xEC, 0x08]), None);
        // mov rbp,rsp 被换成其它（48 89 fc = mov rsp,rdi 之类的编码差异）→ None
        let mut bad = PROLOGUE;
        bad[2] = 0xFC;
        assert_eq!(scan_x86_prologue(&bad), None);
        // 少一次 push（如只推 6 个 callee-saved）→ None（槽位/尾声假设不成立）
        let mut short = PROLOGUE.to_vec();
        short.truncate(13);
        assert_eq!(scan_x86_prologue(&short), None);
        // riscv64 形态 prologue（addi sp, sp, -16 = 13 01 01 11 开头）→ None
        assert_eq!(scan_x86_prologue(&[0x13, 0x01, 0x01, 0x11, 0x13, 0x81, 0x01, 0x02]), None);
        // push 编码中有非 GPR 扩展前缀（如 41 后非 push）→ None
        let mut junk = PROLOGUE.to_vec();
        junk[8] = 0x90; // 破坏 41 54 的 54
        assert_eq!(scan_x86_prologue(&junk), None);
    }
}
