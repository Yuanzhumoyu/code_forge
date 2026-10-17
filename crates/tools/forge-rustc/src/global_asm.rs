//! `global_asm!` 支持（v27 M2，参考 cranelift 后端的 `global_asm.rs`）。
//!
//! ## 为什么是"委托汇编器"而不是我们自己汇编码
//!
//! `global_asm!` 的输入是**汇编文本**：指令、标签、局部标签，还有一串伪指令
//! （`.globl`/`.p2align`/`.byte`/`.quad`/`.section`…）。我们自己的 ISA 汇编器
//! （`TargetAssembler::parse_insts`）只认谱里声明的指令模板——它既不懂伪指令，
//! 也没有符号表，硬做等于把"LLVM 的 asm parser"重写一遍（**冗余**）。真实后端
//! （cranelift / gcc 后端）的做法一致：**把文本交给系统上的汇编器**。
//!
//! 本实现走 cranelift 那条**可移植**的路：另起一个 `rustc` 子进程（默认后端 = LLVM）
//! 把文本包成 `global_asm!` 汇编成对象。它不依赖 GNU `as`（MSVC 上没有），
//! 也不需要用户装额外工具链。
//!
//! ## 归属与链接
//!
//! 产物作为 `CompiledModule::global_asm_object` 报给 rustc（rustc 侧 `link.rs`
//! 会把每个模块的这个对象加进链接命令——见 `rustc_codegen_ssa/back/link.rs`），
//! 因此 global_asm 里定义的符号对 Rust 侧 `extern "C"` 声明可见。
//!
//! ## 支持面（其余 fail-closed，点名缺口 ID）
//!
//! | 形态 | 状态 |
//! | --- | --- |
//! | 字面文本（指令/标签/伪指令） | **支持**（交给 LLVM 的 asm parser ⇒ 伪指令与语法全覆盖） |
//! | `const` 占位符 | **支持**（按常量类型的符号性格式化成十进制） |
//! | `sym`（函数/静态符号）占位符 | **支持**（复用 crate 内同一份 `symbol_name_for_instance`） |
//! | 寄存器操作数（`in`/`out`/`inout`）、`label` | 不支持 → `[cap:GLOBAL_ASM_OPERAND]`（`global_asm!` 本不该有） |
//!
//! "不支持"的那条是**编译期报错**、带缺口 ID，不静默。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rustc_ast::{InlineAsmOptions, InlineAsmTemplatePiece};
use rustc_hir::{InlineAsmOperand, ItemId, ItemKind};
use rustc_middle::ty::TyCtxt;

/// 一个 crate 里所有 `global_asm!` 项拼成的汇编文本。
pub struct GlobalAsmText {
    text: String,
    count: usize,
    is_x86: bool,
}

impl GlobalAsmText {
    /// `isa_name` 决定要不要发语法指示（x86 的 `asm!` 家族默认 Intel 语法）。
    pub fn new(isa_name: &str) -> Self {
        let is_x86 = isa_name.starts_with("x86");
        let mut text = String::new();
        if is_x86 {
            // rustc 的 x86 `asm!`/`global_asm!` 默认 Intel 语法（`.att_syntax` 需显式）。
            text.push_str(".intel_syntax noprefix\n");
        }
        Self {
            text,
            count: 0,
            is_x86,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// 展开一个 `global_asm!` 项（`MonoItem::GlobalAsm(ItemId)` 携带的 id）。
    pub fn push_item<'tcx>(&mut self, tcx: TyCtxt<'tcx>, item_id: ItemId) -> Result<(), String> {
        let item = tcx.hir_item(item_id);
        let ItemKind::GlobalAsm { asm, .. } = item.kind else {
            return Err(format!(
                "[cap:GLOBAL_ASM] 期望 GlobalAsm 项，实际是 {:?}",
                item.kind
            ));
        };
        if self.is_x86 && asm.options.contains(InlineAsmOptions::ATT_SYNTAX) {
            self.text.push_str(".att_syntax\n");
        }
        for piece in asm.template {
            match piece {
                InlineAsmTemplatePiece::String(s) => self.text.push_str(s),
                InlineAsmTemplatePiece::Placeholder { operand_idx, .. } => {
                    let (op, _span) = &asm.operands[*operand_idx];
                    match op {
                        // `const N`：求值后按**常量类型的符号性**格式化成十进制。
                        // rustc 的 `asm_const_to_str` 需要 LayoutCx（本 crate 没实现那套
                        // helper trait）⇒ 按同一语义简化：有符号 ⇒ i128（负值可读）、
                        // 无符号 ⇒ u128。
                        InlineAsmOperand::Const { anon_const } => {
                            let did = anon_const.def_id.to_def_id();
                            let signed = tcx.type_of(did).skip_binder().is_signed();
                            let cv = tcx.const_eval_poly(did).map_err(|e| {
                                format!(
                                    "[cap:GLOBAL_ASM_CONST] global_asm! 的 `const` 求值失败：{e:?}"
                                )
                            })?;
                            let rustc_middle::mir::ConstValue::Scalar(scalar) = cv else {
                                return Err(
                                    "[cap:GLOBAL_ASM_CONST] global_asm! 的 `const` 不是标量\
                                     （仅支持整数常量）"
                                        .to_string(),
                                );
                            };
                            let rustc_middle::mir::interpret::Scalar::Int(i) = scalar else {
                                return Err(
                                    "[cap:GLOBAL_ASM_CONST] global_asm! 的 `const` 不是整数\
                                     （指针常量尚未支持）"
                                        .to_string(),
                                );
                            };
                            let bits = i.to_bits(i.size());
                            if signed {
                                let width = i.size().bits() as u32;
                                let v = if width < 128 {
                                    ((bits as i128) << (128 - width)) >> (128 - width)
                                } else {
                                    bits as i128
                                };
                                self.text.push_str(&format!("{v}"));
                            } else {
                                self.text.push_str(&format!("{bits}"));
                            }
                        }
                        // `sym f`：写 Rust 符号名——复用 crate 内**同一份**
                        // `symbol_name_for_instance`（与函数/调用点符号同源，名字不会两处漂移）。
                        InlineAsmOperand::SymFn { expr } => {
                            let name = self.sym_fn_name(tcx, **expr)?;
                            self.text.push_str(&name);
                        }
                        InlineAsmOperand::SymStatic { def_id, .. } => {
                            let inst = rustc_middle::ty::Instance::new_raw(
                                *def_id,
                                rustc_middle::ty::GenericArgs::empty(),
                            );
                            self.text
                                .push_str(&crate::lower::symbol_name_for_instance(tcx, &inst));
                        }
                        other => {
                            return Err(format!(
                                "[cap:GLOBAL_ASM_OPERAND] global_asm! 里出现寄存器操作数 {other:?}\
                                 ——global_asm! 只接受字面文本/常量/符号"
                            ));
                        }
                    }
                }
            }
        }
        self.text.push('\n');
        self.count += 1;
        Ok(())
    }

    /// `sym` 占位符里的**函数**表达式 → 符号名。
    ///
    /// 与 `lower/mod.rs` 的直接调用路径同源：`Instance::try_resolve` 取完整单态化，
    /// 失败再 `new_raw` 兜底（部分替换的 substs 会让符号名断言 panic）。
    fn sym_fn_name<'tcx>(
        &self,
        tcx: TyCtxt<'tcx>,
        expr: rustc_hir::Expr<'tcx>,
    ) -> Result<String, String> {
        use rustc_middle::ty::{self, TyKind};
        let ty = tcx.typeck(expr.hir_id.owner).node_type(expr.hir_id);
        // 注意：`ty.kind()` 返回 `&TyKind`，直接按变体匹配会把 `args` 绑成 `&&List`
        // （⇒ `try_resolve`/`new_raw` 报类型不符）。解引用后绑 Copy 字段才是 `GenericArgsRef`。
        let TyKind::FnDef(def_id, args) = *ty.kind() else {
            return Err(format!(
                "[cap:GLOBAL_ASM_SYM] global_asm! 的 `sym` 不是函数项（实际类型 {ty}）"
            ));
        };
        // 本 nightly 的 `TyKind::FnDef` 第二字段是 **`Binder<GenericArgsRef>`**
        // （与 `lower/mod.rs` 直接调用路径的 `substs.skip_binder()` 同源）⇒ 先去 binder。
        let args = args.skip_binder();
        let resolved =
            ty::Instance::try_resolve(tcx, ty::TypingEnv::fully_monomorphized(), def_id, args)
                .ok()
                .flatten();
        let instance = resolved.unwrap_or_else(|| ty::Instance::new_raw(def_id, args));
        Ok(crate::lower::symbol_name_for_instance(tcx, &instance))
    }

    /// 把文本汇编成对象文件（`<outdir>/forge_global_asm.o`）。
    ///
    /// 走"另起 rustc（LLVM 默认后端）"这条路：包装成 `global_asm!` 从 stdin 喂进去，
    /// `--emit obj` 出对象。**必须清掉 `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS`**：
    /// 否则子 rustc 会再次加载本后端（`-Zcodegen-backend=<本 dylib>`）⇒ 递归。
    /// **宿主平台**汇编（不给子 rustc 传 `--target`：默认即宿主；本后端目前也只在
    /// 宿主平台工作——跨目标要连同目标面一起做，见方案 M4）。
    pub fn assemble(&self, outdir: &Path) -> Result<PathBuf, String> {
        if self.is_empty() {
            return Err("global_asm 文本为空——不应调用 assemble".to_string());
        }
        let obj = outdir.join("forge_global_asm.o");
        // 去掉 LLVM 风格的 `//` 注释（cranelift 同款处理：汇编器不认 `//`）。
        let body: String = self
            .text
            .lines()
            .map(|l| match l.find("//") {
                Some(i) => &l[..i],
                None => l,
            })
            .collect::<Vec<_>>()
            .join("\n");
        // 包装源码：`#![no_core]` + 内建 `global_asm!` 宏声明（cranelift 同款最小骨架）。
        // `{{`/`}}` 转义：文本会进 `format!`。
        let escaped = body.replace('{', "{{").replace('}', "}}");
        let src = format!(
            "#![feature(decl_macro, no_core, rustc_attrs)]\n\
             #![allow(internal_features)]\n\
             #![no_core]\n\
             #[rustc_builtin_macro]\n\
             #[rustc_macro_transparency = \"semiopaque\"]\n\
             macro global_asm() {{ /* compiler built-in */ }}\n\
             global_asm!(r###\"\n{escaped}\n\"###);\n"
        );

        let mut child = Command::new("rustc")
            .args(["--crate-type", "staticlib", "--emit", "obj", "-o"])
            .arg(&obj)
            .args([
                "-",
                "-Abad_asm_style",
                "-Zunstable-options",
                "--edition",
                "2024",
            ])
            // 关键：清掉会再次加载本后端的变量（见文档注释）。
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("[cap:GLOBAL_ASM] 起 rustc 汇编子进程失败：{e}"))?;
        child
            .stdin
            .take()
            .ok_or_else(|| "[cap:GLOBAL_ASM] 子进程 stdin 不可用".to_string())?
            .write_all(src.as_bytes())
            .map_err(|e| format!("[cap:GLOBAL_ASM] 写汇编文本失败：{e}"))?;
        let out = child
            .wait_with_output()
            .map_err(|e| format!("[cap:GLOBAL_ASM] 等待汇编子进程失败：{e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "[cap:GLOBAL_ASM] global_asm! 汇编失败（{} 项）：\n{}",
                self.count,
                err.lines().take(12).collect::<Vec<_>>().join("\n")
            ));
        }
        Ok(obj)
    }
}
