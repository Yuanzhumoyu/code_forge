//! 参考操作数解析（生成器镜像此语义）。
//!
//! 输入：token 流（[`crate::assembler::lex::Tok`]）+ 寄存器表。每个槽类型的
//! 解析函数返回**类型化值**，`None` = 不匹配（多形状回退语义）。生成器
//! (`v12/codegen`) 按此语义在生成模块内发出 `__reg_cls`/`__imm`/`__label`/
//! `__cond`/`__mem` 的 std-only 版本。

#![cfg_attr(not(test), allow(dead_code))]
//!
//! 关键语义（与生成代码一致）：
//! - 寄存器：名字 → `(index, class)`；槽约束集过滤（`classes` 接纳才匹配）。
//! - 立即数：`Minus + Num` 吸收为负；`0x`/`0b`/float；按 `[min, max]` 校验，
//!   越界 → 不匹配（不再静默截断）。
//! - 标签：数字 = 立即偏移；**非寄存器 ident = 符号引用**（回填期解析）；
//!   寄存器名在标签槽 → 拒绝（避免与 reg 槽歧义）。
//! - 内存：括号平衡 `[ base ( + | - disp )? ]`，token 化后空白免疫。
//! - 条件码：`[conventions.cond]` 表查找（大小写不敏感）。

use std::collections::BTreeMap;

use crate::v12::model::RegClass;
use lex::Tok;

use super::lex;

/// 寄存器表条目（参考层用扁平表；生成层用生成的 `Reg` 枚举 + `FromStr`）。
#[derive(Debug, Clone)]
pub struct RegEntry {
    pub name: String,
    pub index: u32,
    pub class: RegClass,
}

pub type RegTable = Vec<RegEntry>;

/// token 流解析器（`pos` 前进式；`eat_*` 失败不消费）。
#[derive(Debug, Clone)]
pub struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
}

impl<'a> Parser<'a> {
    pub fn new(toks: &'a [Tok]) -> Self {
        Self { toks, pos: 0 }
    }

    pub fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    pub fn eof(&self) -> bool {
        self.pos >= self.toks.len()
    }

    pub fn reset(&mut self) {
        self.pos = 0;
    }

    pub fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub fn eat_ident(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Ident(x)) if x == s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// 任意寄存器：名字 → (index, class)。失败不消费。
    pub fn reg_any(&mut self, regs: &RegTable) -> Option<(u32, RegClass)> {
        let Tok::Ident(name) = self.peek()?.clone() else {
            return None;
        };
        let up = name.to_ascii_uppercase();
        let entry = regs.iter().find(|e| e.name.to_ascii_uppercase() == up)?;
        self.pos += 1;
        Some((entry.index, entry.class))
    }

    /// 约束集过滤：实际 class ∈ classes 才匹配；**不匹配时回滚**（不消费）。
    pub fn reg_in(&mut self, regs: &RegTable, classes: &[RegClass]) -> Option<(u32, RegClass)> {
        let save = self.pos;
        let (idx, cls) = self.reg_any(regs)?;
        if !classes.contains(&cls) {
            self.pos = save;
            return None;
        }
        Some((idx, cls))
    }

    /// 立即数（吸收 `-`；float 位模式；范围校验）。失败回滚。
    pub fn imm(&mut self, min: i64, max: i64, float: bool) -> Option<i64> {
        let save = self.pos;
        let neg = self.eat(&Tok::Minus);
        let v = match self.peek()? {
            Tok::Hex(v) | Tok::Bin(v) | Tok::Dec(v) => {
                let v = *v;
                self.pos += 1;
                v
            }
            Tok::Float(f) if float => {
                let f = *f;
                self.pos += 1;
                return Some(if neg {
                    (-f).to_bits() as i64
                } else {
                    f.to_bits() as i64
                });
            }
            _ => {
                self.pos = save;
                return None;
            }
        };
        let v = if neg {
            match v.checked_neg() {
                Some(x) => x,
                None => {
                    self.pos = save;
                    return None;
                }
            }
        } else {
            v
        };
        if v < min || v > max {
            self.pos = save;
            return None;
        }
        Some(v)
    }

    /// 标签：数字 = 偏移；ident（非寄存器名）= 符号引用（`syms` 记录后返回 0）。
    /// 失败回滚。
    pub fn label(
        &mut self,
        regs: &RegTable,
        min: i64,
        max: i64,
        syms: &mut Vec<(usize, String)>,
        op: usize,
    ) -> Option<i64> {
        let save = self.pos;
        match self.peek()? {
            Tok::Hex(v) | Tok::Bin(v) | Tok::Dec(v) => {
                let v = *v;
                self.pos += 1;
                if v < min || v > max {
                    self.pos = save;
                    return None;
                }
                Some(v)
            }
            Tok::Ident(name) => {
                // 寄存器名在标签槽 → 拒绝（防与 reg 槽歧义），不消费
                let up = name.to_ascii_uppercase();
                if regs.iter().any(|e| e.name.to_ascii_uppercase() == up) {
                    return None;
                }
                syms.push((op, name.clone()));
                self.pos += 1;
                Some(0)
            }
            _ => None,
        }
    }

    /// 条件码（表查找，大小写不敏感）。失败不消费。
    pub fn cond(&mut self, table: &BTreeMap<String, u64>) -> Option<u8> {
        let Tok::Ident(name) = self.peek()?.clone() else {
            return None;
        };
        let code = table
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&name))
            .map(|(_, v)| *v as u8)?;
        self.pos += 1;
        Some(code)
    }

    /// 内存 `[ base ( + | - disp )? ]` → (base 索引, disp)。失败回滚。
    pub fn mem(&mut self, regs: &RegTable) -> Option<(u32, i64)> {
        let save = self.pos;
        if !self.eat(&Tok::LBracket) {
            return None;
        }
        let (base, _) = match self.reg_any(regs) {
            Some(b) => b,
            None => {
                self.pos = save;
                return None;
            }
        };
        let disp = if self.eat(&Tok::Plus) {
            let neg = self.eat(&Tok::Minus);
            let v = match self.raw_int() {
                Some(v) => v,
                None => {
                    self.pos = save;
                    return None;
                }
            };
            if neg { -v } else { v }
        } else if self.eat(&Tok::Minus) {
            match self.raw_int() {
                Some(v) => -v,
                None => {
                    self.pos = save;
                    return None;
                }
            }
        } else {
            0
        };
        if !self.eat(&Tok::RBracket) {
            self.pos = save;
            return None;
        }
        Some((base, disp))
    }

    fn raw_int(&mut self) -> Option<i64> {
        let v = match self.peek()? {
            Tok::Hex(v) | Tok::Bin(v) | Tok::Dec(v) => *v,
            _ => return None,
        };
        self.pos += 1;
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v12::model::RegClass;

    fn regs() -> RegTable {
        vec![
            RegEntry {
                name: "RAX".into(),
                index: 0,
                class: RegClass::GPR(8),
            },
            RegEntry {
                name: "EAX".into(),
                index: 0,
                class: RegClass::GPR(4),
            },
            RegEntry {
                name: "AX".into(),
                index: 0,
                class: RegClass::GPR(2),
            },
            RegEntry {
                name: "RBX".into(),
                index: 3,
                class: RegClass::GPR(8),
            },
        ]
    }

    #[test]
    fn reg_constraint_filtering() {
        let toks = lex::tokenize("rax, eax").unwrap();
        let mut p = Parser::new(&toks);
        let (idx, cls) = p.reg_in(&regs(), &[RegClass::GPR(8)]).unwrap();
        assert_eq!((idx, cls), (0, RegClass::GPR(8)));
        assert!(p.eat(&Tok::Comma));
        // eax 不在 GPR(8) 约束集内
        assert!(p.reg_in(&regs(), &[RegClass::GPR(8)]).is_none());
        // 但多类型约束集接纳
        let (idx, cls) = p
            .reg_in(
                &regs(),
                &[RegClass::GPR(2), RegClass::GPR(4), RegClass::GPR(8)],
            )
            .unwrap();
        assert_eq!((idx, cls), (0, RegClass::GPR(4)));
    }

    #[test]
    fn imm_sign_radix_range() {
        let toks = lex::tokenize("-0x10, 0b101, 1.5, 200").unwrap();
        let mut p = Parser::new(&toks);
        // -0x10 = -16
        assert_eq!(p.imm(i64::MIN, i64::MAX, false), Some(-16));
        assert!(p.eat(&Tok::Comma));
        assert_eq!(p.imm(0, 7, false), Some(5)); // 0b101
        assert!(p.eat(&Tok::Comma));
        assert_eq!(
            p.imm(i64::MIN, i64::MAX, true),
            Some(1.5f64.to_bits() as i64)
        ); // float 位模式
        assert!(p.eat(&Tok::Comma));
        assert!(p.imm(0, 100, false).is_none()); // 200 越界（回滚不消费）
        assert_eq!(p.imm(0, 300, false), Some(200));
    }

    #[test]
    fn label_symbol_vs_reg_name() {
        let toks = lex::tokenize("loop, RAX, 4").unwrap();
        let mut p = Parser::new(&toks);
        let mut syms = Vec::new();
        assert_eq!(p.label(&regs(), i64::MIN, i64::MAX, &mut syms, 2), Some(0));
        assert_eq!(syms, vec![(2, "loop".to_string())]);
        assert!(p.eat(&Tok::Comma));
        // 寄存器名在标签槽 → 拒绝且不消费
        assert!(p.label(&regs(), i64::MIN, i64::MAX, &mut syms, 2).is_none());
        assert!(matches!(p.peek(), Some(Tok::Ident(n)) if n == "RAX"));
        p.pos += 1; // 模拟该 form 失败回退后，下一 form 重新消费
        assert!(p.eat(&Tok::Comma));
        assert_eq!(p.label(&regs(), i64::MIN, i64::MAX, &mut syms, 2), Some(4));
    }

    #[test]
    fn mem_brackets_whitespace_immune() {
        let toks = lex::tokenize("[ rax + 8 ], [rbx-4], [rax]").unwrap();
        let mut p = Parser::new(&toks);
        assert_eq!(p.mem(&regs()), Some((0, 8)));
        assert!(p.eat(&Tok::Comma));
        assert_eq!(p.mem(&regs()), Some((3, -4)));
        assert!(p.eat(&Tok::Comma));
        assert_eq!(p.mem(&regs()), Some((0, 0)));
    }

    #[test]
    fn cond_table_lookup_case_insensitive() {
        let table: BTreeMap<String, u64> = [("e", 4), ("ne", 5)]
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect();
        let toks = lex::tokenize("NE, xx").unwrap();
        let mut p = Parser::new(&toks);
        assert_eq!(p.cond(&table), Some(5));
        assert!(p.cond(&table).is_none());
    }
}
