//! CodegenBackend 外壳：rustc 入口 → CGU 分区 → 逐 CGU 降级编译 →
//! 每 CGU 独立对象文件写出 → 增量元数据（多 WorkProduct）。
//!
//! # M3/M4/M5 演进（并行 CGU Stage A）→ M6（并行 CGU Stage B）
//!
//! Stage A（M3-M5）：单对象单模块。`collect_instances` 把 **全部 CGU 摊平**
//! 按符号名排序，函数降级任务（M4 起经 `rustc_data_structures::sync::par_map`
//! 提交 rustc 查询池）私有 FuncRefTable 归并后主线程**单 ObjectWriter** 按
//! 全局符号序 emission——产物与 `-C codegen-units` 无关（单 .o，链接器一次
//! 拿一个对象）。内部数据符号 M5 起为内容稳定键
//! （`__slice_{hash}`/`__vtable_{hash}`，跨串/并行逐字稳定）。
//!
//! Stage B（M6，本版）：**每 CGU 独立对象文件 + 多 CompiledModule/WorkProduct**
//! （rustc 原生多对象形态，CGU 级增量由此可得）。rustc 对 -Zcodegen-backend
//! 只调 codegen_crate 一次、无按 CGU 回调（LLVM 在自己 codegen_crate 内并行，
//! forge 不可复用）→ forge 侧消费 `collect_and_partition_mono_items` 的**真
//! CGU 分区**（rustc 已按 CGU 名排序、跨会话稳定），每 CGU：
//! - 组内实例按符号名排序（确定性，沿用 A1 排序键）；
//! - 组内函数任务沿用 M4 原语（par_map 到 rustc 池，否则串行 map——两条路径
//!   共用同一任务体，产物一致）；任务私有表归并到组表；
//! - 组内独立 ObjectWriter，主线程按 CGU 序依次写 `forge_codegen_output.{cgu}.o`
//!   （多对象模式下无跨 CGU 的 add_function/add_data 交错——每组自含）；
//! - **数据 owner 归并**：vtable/promoted 数据记录跨 CGU 按**符号名**（=内容
//!   稳定键）去重，owner = 全局首见 CGU；仅 owner 对象 add_data/add_rodata，
//!   其余 CGU 的函数对同符号走 UNDEF 外部引用 → 链接器解析（COFF 标准，
//!   无 LNK2005）。函数/Static 由 rustc 分区天然单归属；防御性按符号名
//!   跨 CGU 去重（LocalCopy/内联副本可能同实例出现在多 CGU——COFF 全局
//!   强符号只能定义一次，首 CGU 定义、其余引用）。
//! - 跨 CGU 函数引用 = 任务内就地 resolve 成真实符号名（M4 机制）→ 对象间
//!   UNDEF + 链接器解析。
//! - alloc runtime（__rust_alloc 等手写 shim）随首 CGU 对象（确定性位置）。
//!
//! 门控：`-C debuginfo>=1`（B-v1：单 CU DWARF 不拆，dwarf 段 reloc 要求目标
//! 符号同文件定义——见 add_dwarf）或 `-C codegen-units=1`（单 CGU 回归锚）或
//! env `FORGE_SINGLE_OBJECT=1` 时回退 **Stage A 单对象路径**（跨 CGU 摊平 +
//! 全局符号序 + 单 writer 尾注数据/dwarf——debuginfo 用例与 cgu=1 行为等价
//! 保持）。默认 rustc codegen-units=16 → 多对象为常态路径。

use crate::alloc_runtime::build_alloc_runtime;
use crate::compile::{auto_register_isa_for_target, isa_name_for_target};
use crate::func_ref::FuncRefTable;
use crate::prelude::*;
use rustc_middle::mono::{CodegenUnit, MonoItemPartitions};

/// rustc 的 `-Zcodegen-backend` 入口实现。
pub struct CodegenLibBackend;

impl CodegenBackend for CodegenLibBackend {
    fn name(&self) -> &'static str {
        "code-forge"
    }

    fn target_cpu(&self, _sess: &Session) -> String {
        "generic".to_string()
    }

    fn codegen_crate<'tcx>(&self, tcx: TyCtxt<'tcx>) -> Box<dyn Any> {
        crate::trace::install_panic_hook();
        // 1. 从环境变量加载外部 ISA 插件
        let _plugin_loader = code_forge::forge_plugin::PluginLoader::load_from_env();

        // 2. 根据目标三元组自动注册内置 ISA 后端
        let target_triple = format!("{}", tcx.sess.opts.target_triple);
        auto_register_isa_for_target(&target_triple);

        let mut compiled_modules = Vec::new();

        // G5：panic=unwind 明确拒绝（当前后端仅支持 panic=abort——无
        // unwind/异常表，unwind 会产生未定义行为/链接错误）。用
        // dcx().err 使编译失败而非静默产出错误程序。
        let panic_abort = tcx.sess.panic_strategy() == rustc_target::spec::PanicStrategy::Abort;
        if !panic_abort {
            tcx.dcx()
                .err("code-forge: panic=unwind 暂不支持——仅 panic=abort（请加 -C panic=abort）");
            compiled_modules.push(CompiledModule {
                name: "empty".to_string(),
                kind: ModuleKind::Regular,
                object: None,
                global_asm_object: None,
                dwarf_object: None,
                bytecode: None,
                assembly: None,
                llvm_ir: None,
                links_from_incr_cache: Vec::new(),
            });
            return Box::new(CompiledModules {
                modules: compiled_modules,
                allocator_module: None,
            });
        }

        let outdir = tcx.output_filenames(()).with_extension("").to_path_buf();
        let outdir = outdir
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();

        // ── M6 Stage B：消费真 CGU 分区（不摊平）──
        // rustc 分区（rustc_monomorphize::partitioning::partition）返回按 CGU
        // **名字符串升序**的列表（确定性、跨会话稳定），每个 CGU 的
        // items 为 FxIndexMap（MonoItem → MonoItemData）；首 CGU 已由 rustc
        // 标为 primary。CGU 数量 ≤ `-C codegen-units`（merge 后）。
        let MonoItemPartitions { codegen_units, .. } = tcx.collect_and_partition_mono_items(());
        if crate::trace::trace_enabled("CGU") {
            eprintln!(
                "[forge] rustc partition: codegen_units={} (codegen-units opt={:?})",
                codegen_units.len(),
                tcx.sess.opts.cg.codegen_units
            );
            for cu in codegen_units {
                eprintln!(
                    "[forge]   cgu items={} name={}",
                    cu.items().len(),
                    cu.name()
                );
            }
        }

        // B-v1 门控：debuginfo>=1（单 CU DWARF 不拆——add_dwarf 的 reloc 目标
        // 符号必须同文件定义）或 FORGE_SINGLE_OBJECT=1 逃生口 → 单对象；
        // 单 CGU 天然单对象（`-C codegen-units=1` 回归锚 = Stage A 语义）。
        let debuginfo_on = tcx.sess.opts.debuginfo != rustc_session::config::DebugInfo::None;
        let debuginfo_full = tcx.sess.opts.debuginfo == rustc_session::config::DebugInfo::Full;
        let force_single = std::env::var("FORGE_SINGLE_OBJECT")
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
        let multi_object = !debuginfo_on && !force_single && codegen_units.len() > 1;

        // 实例分组计划：单对象 = 1 个（跨 CGU 摊平）计划；多对象 = 每 CGU 一个
        //（组内过滤 allocator shim 等 + 按符号名排序 + 跨 CGU 同名去重）。
        let plans = build_plans(tcx, codegen_units, multi_object);

        if plans.is_empty() {
            compiled_modules.push(CompiledModule {
                name: "empty".to_string(),
                kind: ModuleKind::Regular,
                object: None,
                global_asm_object: None,
                dwarf_object: None,
                bytecode: None,
                assembly: None,
                llvm_ir: None,
                links_from_incr_cache: Vec::new(),
            });
            return Box::new(CompiledModules {
                modules: compiled_modules,
                allocator_module: None,
            });
        }

        // 创建对象文件写入器（每 CGU/每对象独立实例——自包含可多实例）
        let config = TargetConfig::host().unwrap_or_default();

        // ── 函数降级任务（M4 机制沿用，M6 按 CGU 分组）──
        // rustc 对 -Zcodegen-backend 只调 codegen_crate 一次、无按 CGU 回调
        // → forge 无法复用 rustc_codegen_ssa 的 per-CGU 调度；函数粒度任务化：
        // 全部 CGU 的**每个函数**是一个独立任务（任务序 = CGU 名升序 × 组内
        // 符号序——确定性），任务私有 FuncRefTable（@N/G{N} 编号只以重定位
        // 符号留机器码、每函数编译完就地 resolve——不跨函数逃逸，任务私有
        // 零加锁）+ 任务内登记的 vtable/promoted/line/var/enum 条目归并取回
        //（按任务序 = 全局函数首见序，与旧串行路径一致）。
        //
        // ⚠️ 宿主线程上下文（WA-38，M4 根治）：rustc 1.99/1.100 查询引擎
        // 统一为 WorkerLocal——tcx 查询只能在 rustc 自建池线程上执行
        //（WorkerLocal registry / 作业 TLV ImplicitCtxt / per-thread
        // SessionGlobals 三者只由 rustc 池线程持有且**无注册 API**）→ M3
        // 的 `std::thread` 自建池方案是死路。根治：直接复用 rustc 自家并行
        // 原语 `rustc_data_structures::sync::par_map`——rustc_codegen_ssa 在
        // -Z threads>=2（`sess.opts.jobs.frontend.is_some()`）时用同一原语
        // 把 per-CGU 编译作为**嵌套池作业**并行（该原语与 LLVM 无关）。
        // forge 的 codegen_crate 本身运行在 rustc 池作业线程上（可安全跑
        // tcx 查询），par_map 的嵌套作业继承全套线程上下文 → 单函数任务
        // 即为 par_map 的一个元素。par_* 在非并行模式自动退化为**当前线程
        // 按输入序串行**——行为与串行 map 恒等（产物一致）。
        let mut work: Vec<(usize, usize)> = Vec::new(); // (plan 下标, items 内下标)
        for (pi, plan) in plans.iter().enumerate() {
            for (fi, item) in plan.items.iter().enumerate() {
                if matches!(item, MonoItem::Fn(_)) {
                    work.push((pi, fi));
                }
            }
        }
        // M4 并行门控：env 显式要求并行（FORGE_CODEGEN_THREADS>1）&& rustc
        // 前端并行池存在（-Z threads>=2 / --jobs-frontend）&& 函数数 >1。
        let forge_threads = match std::env::var("FORGE_CODEGEN_THREADS") {
            Ok(v) => v.trim().parse::<usize>().unwrap_or(0),
            Err(_) => 0,
        };
        let rustc_pool = tcx.sess.opts.jobs.frontend.is_some();
        let parallel = forge_threads > 1 && rustc_pool && work.len() > 1;
        if parallel {
            eprintln!(
                "[forge] FORGE_CODEGEN_THREADS>1 + rustc 并行前端（-Z threads）：\
                 {} 个函数降级任务（{} CGU）提交到 rustc 查询池（par_map，确定序保序）",
                work.len(),
                plans.len()
            );
        } else if forge_threads > 1 && !rustc_pool && work.len() > 1 {
            eprintln!(
                "[forge] FORGE_CODEGEN_THREADS={forge_threads} 但 rustc 无并行前端\
                 （-Z threads>=2，经 RUSTFLAGS 传入）——forge 函数并行需 rustc 查询池\
                 （WA-38/M4），当前串行编译（产物不变）"
            );
        }
        let tasks: Vec<TaskOutcome> = if parallel {
            rustc_data_structures::sync::par_map(
                work.iter().copied(),
                |(pi, fi)| compile_fn_task_guarded(tcx, &plans, pi, fi),
            )
        } else {
            work.iter()
                .copied()
                .map(|(pi, fi)| compile_fn_task_guarded(tcx, &plans, pi, fi))
                .collect()
        };

        // ── 归并：每 CGU 组表 + 数据 owner 判定 ──
        // line/var/enum 并入模块级 func_ref_table（dwarf 用，任务序 = 全局
        // 函数序）；vtable/promoted 记录并入**规范表**（按符号名 = 内容稳定键
        // 去重；owner = 全局首见 CGU 下标）——每 CGU 对象只含被该 CGU 引用
        //（owner）的数据定义，其余 CGU 对同符号走 UNDEF（链接器解析），
        // 杜绝跨对象重复强符号 LNK2005。
        let mut func_ref_table = FuncRefTable::default();
        let mut vtables: Vec<crate::func_ref::VtableRecord> = Vec::new();
        let mut vtable_owner: Vec<usize> = Vec::new();
        let mut vtable_sym_idx: HashMap<String, usize> = HashMap::new();
        let mut promoted: Vec<crate::func_ref::PromotedRecord> = Vec::new();
        let mut promoted_owner: Vec<usize> = Vec::new();
        let mut promoted_sym_idx: HashMap<String, usize> = HashMap::new();
        let mut outcomes_by_plan: Vec<Vec<FnOutcome>> = Vec::new();
        outcomes_by_plan.resize_with(plans.len(), Vec::new);
        {
            let mut task_iter = tasks.into_iter().peekable();
            let mut total_fns = 0usize;
            for pi in 0..plans.len() {
                // 任务按 (plan, fn 序) 保序（par_map 保输入序、串行 map 天然保序）
                while let Some(t) = task_iter.peek() {
                    if t.plan != pi {
                        break;
                    }
                    let mut t = task_iter.next().expect("M6: task 队列错位");
                    // 数据记录先按符号名入规范表（owner = pi）
                    for (alloc_id, sym, bytes, relocs, align) in t.table.drain_vtables() {
                        if let Some(&idx) = vtable_sym_idx.get(&sym) {
                            let (_, old_sym, old_bytes, old_relocs, old_align) = &vtables[idx];
                            debug_assert_eq!(old_sym, &sym, "same vtable sym must have same name");
                            debug_assert_eq!(old_bytes, &bytes, "same vtable sym must have same bytes");
                            debug_assert_eq!(
                                old_relocs, &relocs,
                                "same vtable sym must have same relocs"
                            );
                            debug_assert_eq!(old_align, &align, "same vtable sym must have same align");
                            let _ = alloc_id;
                        } else {
                            vtable_sym_idx.insert(sym.clone(), vtables.len());
                            vtables.push((alloc_id, sym, bytes, relocs, align));
                            vtable_owner.push(pi);
                        }
                    }
                    for rec in t.table.drain_promoted() {
                        let (alloc_id, sym, bytes, align) = &rec;
                        if let Some(&idx) = promoted_sym_idx.get(sym) {
                            let (_, old_sym, old_bytes, old_align) = &promoted[idx];
                            debug_assert_eq!(old_sym, sym, "same promoted sym must have same name");
                            debug_assert_eq!(old_bytes, bytes, "same promoted sym must have same bytes");
                            debug_assert_eq!(old_align, align, "same promoted sym must have same align");
                            let _ = alloc_id;
                        } else {
                            promoted_sym_idx.insert(sym.clone(), promoted.len());
                            promoted.push(rec);
                            promoted_owner.push(pi);
                        }
                    }
                    // line/var/enum 并入模块表（dwarf；任务序 = 全局函数序）
                    func_ref_table.merge_task_table(&mut t.table);
                    total_fns += t.fns.len();
                    outcomes_by_plan[pi].append(&mut t.fns);
                }
            }
            debug_assert!(
                task_iter.next().is_none(),
                "M6: 残留未归并的任务（plans/任务序错位）"
            );
            debug_assert_eq!(
                total_fns, work.len(),
                "M6: worker 归并结果数必须等于函数实例数"
            );
        }
        let _ = work; // work 已消费（仅计数用）

        // owner 记录下标分组（发射每 CGU 对象时按全局序取 owner 记录）
        let vtable_idx_by_plan: Vec<Vec<usize>> = {
            let mut v: Vec<Vec<usize>> = (0..plans.len()).map(|_| Vec::new()).collect();
            for (i, &o) in vtable_owner.iter().enumerate() {
                v[o].push(i);
            }
            v
        };
        let promoted_idx_by_plan: Vec<Vec<usize>> = {
            let mut v: Vec<Vec<usize>> = (0..plans.len()).map(|_| Vec::new()).collect();
            for (i, &o) in promoted_owner.iter().enumerate() {
                v[o].push(i);
            }
            v
        };

        // ── 每 CGU 一个 ObjectWriter，主线程按 CGU 序依次写 .o ──
        // 模块级 FuncRef 表已归并完成；组内 Fn 结果按 (CGU, 符号序) 消费。
        // 跨 CGU 引用经符号名 UNDEF 由链接器解析（COFF 标准）；数据记录仅
        // owner CGU 定义。
        // B1：per-function per-statement 行号表（(符号, [(机器码偏移, 行)])）
        let mut fn_line_tables: Vec<(String, Vec<(u32, u32)>)> = Vec::new();
        // CU high_pc：所有发射函数的代码总字节（含 main 别名副本——见下方
        // add_function("main")），dwarf CU DIE 范围用（gdb pc→CU 映射）。
        let mut code_span: u64 = 0;
        // 每函数代码字节（subprogram DW_AT_high_pc——gdb 需函数结束地址才能
        // 建 function block，缺则变量 DIE 被丢（对照 gcc 实证））。
        let mut fn_sizes: std::collections::HashMap<String, u64> = Default::default();
        // M2：每函数 .debug_frame CFI（符号, 代码字节, 行集——emission 对
        // x86 prologue 扫描产物；与 fn_sizes 同键控、同迭代序收集；无 CFI
        //（非 x86/形态不符）不进表 → dwarf.rs 只对这些符号产 FDE）。
        let mut fn_cfi: Vec<(String, u64, FunctionCfi)> = Vec::new();

        let isa_name = isa_name_for_target(&target_triple);
        for (pi, plan) in plans.iter().enumerate() {
            if crate::trace::trace_enabled("CGU") {
                eprintln!(
                    "[forge] plan#{pi} cgu={} items={}",
                    plan.cgu_name.as_deref().unwrap_or("<single>"),
                    plan.items.len()
                );
            }
            let mut writer = ObjectWriter::new(&config).expect("create object writer");
            let mut plan_fns = std::mem::take(&mut outcomes_by_plan[pi]).into_iter();

            // 组内实例按符号序 emission：Fn 消费预编译结果（任务归并序 =
            // 组内符号序）、Static 现场求值——与旧串行路径逐条同序。
            for item in plan.items.iter() {
                match item {
                    MonoItem::Fn(instance) => {
                        let outcome = plan_fns.next().expect("M6: Fn emission 顺序错位");
                        match outcome.result {
                            Ok(compiled_func) => {
                                if crate::trace::trace_enabled("GLOBAL") {
                                    eprintln!(
                                        "[forge] plan#{pi} add_function sym={}",
                                        outcome.sym_name
                                    );
                                }
                                let _ = writer.add_function(&outcome.sym_name, &compiled_func);
                                code_span += compiled_func.code.len() as u64;
                                fn_sizes
                                    .insert(outcome.sym_name.clone(), compiled_func.code.len() as u64);
                                // M2：CFI（x86 prologue scan 产物）——与 fn_sizes
                                // 同键控收集，dwarf 生成 .debug_frame FDE
                                if let Some(cfi) = &compiled_func.cfi {
                                    fn_cfi.push((
                                        outcome.sym_name.clone(),
                                        compiled_func.code.len() as u64,
                                        cfi.clone(),
                                    ));
                                }
                                // B1：收集该函数的 per-statement 行号表（主库 emission
                                // 输出 (机器码偏移, 行)）——debuginfo 开启时 dwarf.rs
                                // 生成 .debug_line 的每语句条目。
                                if !compiled_func.line_entries.is_empty() {
                                    fn_line_tables.push((
                                        outcome.sym_name.clone(),
                                        compiled_func.line_entries.clone(),
                                    ));
                                }
                                // main 函数需要 C 名称别名（链接器入口点）。
                                // 用 def_path_str 而非 item_name——闭包/内部 shim 的
                                // DefId 无 item_name（对 closure DefId 调用会 ICE）
                                if tcx.def_path_str(instance.def_id()).ends_with("::main") {
                                    let _ = writer.add_function("main", &compiled_func);
                                    code_span += compiled_func.code.len() as u64;
                                }
                            }
                            Err(e) => {
                                tcx.dcx().err(format!(
                                    "code-forge: failed to compile '{}': {e}",
                                    outcome.sym_name
                                ));
                            }
                        }
                    }
                    MonoItem::Static(def_id) => {
                        // 静态数据：求值初始值 → .data/.rodata 段符号（global_data.rs）。
                        // `const {allocN}` 引用通过符号名解析，缺失会导致 static 读取错误。
                        match crate::global_data::build_static_data(tcx, *def_id) {
                            Ok(sd) => {
                                if sd.mutable {
                                    let _ = writer.add_data(&sd.sym, &sd.data, sd.align);
                                } else {
                                    let _ = writer.add_rodata(&sd.sym, &sd.data, sd.align);
                                }
                            }
                            Err(msg) => {
                                tcx.dcx().err(format!("code-forge: {msg}"));
                            }
                        }
                    }
                    MonoItem::GlobalAsm(_) => {
                        // A4：报错附 WA 编号（WORKAROUNDS.md 机读清单）。
                        tcx.dcx()
                            .err("code-forge: global_asm not supported [WA-03]");
                    }
                }
            }
            debug_assert!(plan_fns.next().is_none(), "M6: 残留未消费的 fn 结果");

            // 注入 alloc 运行时（__rust_alloc/dealloc/realloc/alloc_zeroed，基于
            // VirtualAlloc/VirtualFree），使 no_std + extern crate alloc 可用。
            // Stage A 语义 = 单对象尾注；Stage B 多对象 = 首 CGU 对象（确定性
            // 归属——无跨对象重复定义）。
            if pi == 0 {
                match build_alloc_runtime(tcx, &mut func_ref_table, isa_name) {
                    Ok(runtime_fns) => {
                        for (sym, mut compiled) in runtime_fns {
                            func_ref_table.resolve_relocs(&mut compiled);
                            let _ = writer.add_function(&sym, &compiled);
                        }
                    }
                    Err(e) => {
                        tcx.dcx().err(format!("code-forge: alloc runtime: {e}"));
                    }
                }
            }

            // 本 CGU owner 的数据段：
            // vtable 指针表落 .data（[WA-01] MSVC 链接器对 .rodata 段的 ADDR64
            // 重定位不应用（实测链接后全 0），vtable 指针表必须放 .data 段
            //（add_data_with_relocs）才能正确解析方法地址）。
            for &vi in &vtable_idx_by_plan[pi] {
                let (_, sym, bytes, relocs, align) = &vtables[vi];
                let reloc_refs: Vec<(usize, RelocKind, &str, i64)> = relocs
                    .iter()
                    .map(|(o, k, s, a)| (*o, *k, s.as_str(), *a))
                    .collect();
                let _ = writer.add_data_with_relocs(sym, bytes, &reloc_refs, *align);
            }
            // promoted/slice 常量（`&"str"`、`&[1,2,3]` 字面量）：纯数据无重定位，
            // 落 .rodata（无 [WA-01] ADDR64 限制）。
            for &ri in &promoted_idx_by_plan[pi] {
                let (_, sym, bytes, align) = &promoted[ri];
                let _ = writer.add_rodata(sym, bytes, *align);
            }

            // C1/C2 DebugInfo（仅单对象模式——B-v1：单 CU DWARF 不拆；多对象
            // per-CGU CU 为 B-v2）：`-C debuginfo` 非 None 时生成 DWARF 段
            //（.debug_line/.debug_info/.debug_abbrev/.debug_frame）并入主对象。
            let single_object = plans.len() == 1;
            if single_object && debuginfo_on && !func_ref_table.line_entries().is_empty() {
                let entries = func_ref_table.line_entries().to_vec();
                let vars = func_ref_table.var_entries().to_vec();
                // B1：entries（每函数声明行）+ fn_line_tables（per-statement）→
                // fns: (符号, 声明行, [(指令偏移, 行)])——dwarf gen_debug_line
                // 生成函数级 + 每语句行号条目（COFF addend 隐式：占位写偏移）。
                let fns: Vec<(String, u32, Vec<(u32, u32)>)> = entries
                    .iter()
                    .map(|(s, l)| {
                        let stmts = fn_line_tables
                            .iter()
                            .find(|(fs, _)| fs == s)
                            .map(|(_, v)| v.clone())
                            .unwrap_or_default();
                        (s.clone(), *l, stmts)
                    })
                    .collect();
                let producer = format!(
                    "code-forge {} (rustc {})",
                    env!("CARGO_PKG_VERSION"),
                    option_env!("CFG_VERSION").unwrap_or("")
                );
                // 源文件名：CU DW_AT_name + .debug_line file 条目必须指向磁盘上的
                // 真实源文件（gdb `list`/源码断点按此打开文件；crate 名匹配不到
                // .rs 文件）。取本地 crate 根模块所在文件。
                let root_name = tcx
                    .sess
                    .source_map()
                    .lookup_source_file(
                        tcx.def_span(rustc_hir::def_id::LOCAL_CRATE.as_def_id())
                            .lo(),
                    )
                    .name
                    .clone();
                let src_file = match &root_name {
                    rustc_span::FileName::Real(rf) => rf
                        .local_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<unknown>".to_string()),
                    _ => "<unknown>".to_string(),
                };
                let sections = crate::dwarf::build_dwarf_sections(
                    &fns,
                    &vars,
                    &producer,
                    &src_file,
                    code_span,
                    &fn_sizes,
                    func_ref_table.enum_types(),
                    debuginfo_full,
                    &fn_cfi,
                );
                // 段内 reloc（地址占位 → 函数符号）随段数据传给 add_dwarf——
                // object crate 对 COFF 调试段发射 ADDR64 reloc，链接器解析
                // 为函数真实地址（low_pc/行号 set_address 可用）。
                let dwarf_sections: Vec<(&str, Vec<u8>, Vec<(usize, String)>)> = sections
                    .iter()
                    .map(|(n, b, r)| (n.as_str(), b.clone(), r.clone()))
                    .collect();
                if let Err(e) = writer.add_dwarf(&dwarf_sections) {
                    tcx.dcx()
                        .warn(format!("code-forge: dwarf emission failed: {e}"));
                }
            }

            // 写入对象文件到磁盘（单对象 = forge_codegen_output.o——Stage A
            // 同名锚；多对象 = forge_codegen_output.{cgu 名}.o）
            let module_name = match &plan.cgu_name {
                Some(cgu) => {
                    format!("forge_codegen_output.{cgu}")
                }
                None => "forge_codegen_output".to_string(),
            };
            let obj_path = outdir.join(format!("{module_name}.o"));
            match writer.write_to_file(&obj_path) {
                Ok(()) => {
                    compiled_modules.push(CompiledModule {
                        name: plan.cgu_name.clone().unwrap_or_else(|| module_name.clone()),
                        kind: ModuleKind::Regular,
                        object: Some(obj_path),
                        global_asm_object: None,
                        dwarf_object: None,
                        bytecode: None,
                        assembly: None,
                        llvm_ir: None,
                        links_from_incr_cache: Vec::new(),
                    });
                }
                Err(e) => {
                    tcx.dcx()
                        .err(format!("code-forge: failed to write object file: {e}"));
                }
            }
        }

        Box::new(CompiledModules {
            modules: compiled_modules,
            allocator_module: None,
        })
    }

    fn join_codegen(
        &self,
        ongoing_codegen: Box<dyn Any>,
        _sess: &Session,
        _incr_comp_session: Option<&IncrCompSession>,
        _outputs: &OutputFilenames,
        _crate_info: &CrateInfo,
    ) -> (CompiledModules, WorkProductMap) {
        let compiled_modules = *ongoing_codegen
            .downcast::<CompiledModules>()
            .expect("codegen results");

        let mut work_products = rustc_data_structures::unord::UnordMap::default();

        // M6 Stage B：每 CGU 一个 WorkProduct（module.name = CGU 名 →
        // WorkProductId::from_cgu_name，rustc 侧按 CGU 粒度记录——增量到
        // CGU 粒度由此获得；单对象模式 = 单个 WorkProduct，与 Stage A 一致）。
        for module in &compiled_modules.modules {
            if let Some(ref obj_path) = module.object {
                let wp_id = WorkProductId::from_cgu_name(&module.name);
                let mut saved = rustc_data_structures::unord::UnordMap::default();
                saved.insert("o".to_string(), obj_path.to_string_lossy().to_string());
                work_products.insert(
                    wp_id,
                    WorkProduct {
                        cgu_name: module.name.clone(),
                        saved_files: saved,
                    },
                );
            }
        }

        (compiled_modules, work_products)
    }
    // link() 使用 trait 的默认实现 (link_binary)
}

// ============================================================
// M6 Stage B：CGU 分区 → 实例分组计划
// ============================================================

/// 一个待发射对象的实例分组计划。
struct CguPlan<'tcx> {
    /// 多对象模式：rustc CGU 名（= module.name / WorkProductId / 对象文件名
    /// 后缀，跨会话稳定）；单对象模式：None（"forge_codegen_output"）。
    cgu_name: Option<String>,
    /// 保留实例（过滤 allocator shim 等）且按符号名排序（A1 确定性）——
    /// Fn/Static/GlobalAsm（GlobalAsm 恒排最后，发射时 dcx 报错终止）。
    items: Vec<MonoItem<'tcx>>,
}

/// 是否保留某 MonoItem：Fn 需跳过 alloc crate 的 allocator shim 实例
///（__rust_alloc/dealloc/realloc/alloc_zeroed——extern "C" 声明，C 符号名，
/// 若编译会与手写 shim（build_alloc_runtime 注入的 forge-ir 版）重复定义冲突，
/// 且其 rustc 版内部依赖 Layout/Alignment 复杂 lowering）。Static/GlobalAsm 全保留。
fn keep_item<'tcx>(tcx: TyCtxt<'tcx>, item: MonoItem<'tcx>) -> bool {
    match item {
        MonoItem::Fn(instance) => {
            let def_id = instance.def_id();
            let in_alloc = tcx.crate_name(def_id.krate).as_str() == "alloc";
            if !in_alloc {
                return true;
            }
            let alloc_shims = [
                "__rust_alloc",
                "__rust_dealloc",
                "__rust_realloc",
                "__rust_alloc_zeroed",
            ];
            // 注意：闭包 DefId 无 item_name（ICE），用 def_path_str 兜底——
            // allocator shim 是普通 fn，def_path_str 以 "__rust_alloc" 结尾。
            let name = if let Some(n) = tcx.opt_item_name(def_id) {
                n.as_str().to_string()
            } else {
                tcx.def_path_str(def_id)
            };
            !alloc_shims.contains(&name.as_str())
        }
        _ => true,
    }
}

/// 消费 rustc 真 CGU 分区，构造实例分组计划。
///
/// - `multi_object = false`（单对象：debuginfo 开 / FORGE_SINGLE_OBJECT=1 /
///   单 CGU）：跨 CGU 摊平 + 全局符号序排序（= Stage A `collect_instances`
///   语义——cgu=1 与 debuginfo 用例产物/行为与旧路径等价），单计划（None 名）。
/// - `multi_object = true`：每 CGU 一个计划（组内过滤 + 符号序排序）；同名
///   实例（LocalCopy/内联副本会按引用 CGU 重复附着）跨 CGU **首见去重**——
///   COFF 全局强符号只能定义一次：首 CGU 定义、其余 CGU 对该符号走 UNDEF
///   由链接器解析（与 rustc LLVM 的 per-CGU internal 副本语义不同但等价——
///   forge 全对象为全局符号，见模块注释）。
/// - 计划序 = CGU 名字符串升序（rustc 已排序；跨会话稳定）。
///
/// E1 说明（WA-04 LNK2019）：裸 rustc 场景下，forge 编译的用户实例（如
/// slice::iter::Iter::next）引用 core 泛型辅助函数（如 unchecked_sub::
/// precondition_check），但 rustc collect 认为 core 由预编译 rlib 提供（rlib
/// 无该符号）→ LNK2019。尝试沿调用图补生成失败：裸 rustc 下 core rlib 只有
/// 优化码，`instance_mir` 查询 core 实例触发 ICE。根治路径：README 已记录的
/// `-Zshare-generics=yes`（cargo build-std 场景 LLVM 生成 core 实例）。slice_iter
/// 类用例标记 known_failure（WA-19）。
fn build_plans<'tcx>(
    tcx: TyCtxt<'tcx>,
    codegen_units: &'tcx [CodegenUnit<'tcx>],
    multi_object: bool,
) -> Vec<CguPlan<'tcx>> {
    if !multi_object {
        // 单对象（Stage A 语义）：跨 CGU 摊平 + 全局符号序 + 同名去重
        let mut items: Vec<MonoItem<'tcx>> = codegen_units
            .iter()
            .flat_map(|cgu| cgu.items().keys().copied())
            .filter(|item| keep_item(tcx, *item))
            .collect();
        items.sort_by_key(|a| mono_item_sort_key(tcx, *a));
        let items = dedup_by_symbol(tcx, items);
        if items.is_empty() {
            return Vec::new();
        }
        return vec![CguPlan {
            cgu_name: None,
            items,
        }];
    }

    let mut plans: Vec<CguPlan<'tcx>> = Vec::new();
    let mut seen: HashMap<String, ()> = HashMap::new(); // 跨 CGU 已定义符号名
    for cgu in codegen_units {
        let mut items: Vec<MonoItem<'tcx>> = cgu
            .items()
            .keys()
            .copied()
            .filter(|item| keep_item(tcx, *item))
            .collect();
        items.sort_by_key(|a| mono_item_sort_key(tcx, *a));
        // 跨 CGU 同名去重（首见保留；GlobalAsm 不去重——sort 键为常量非符号）
        let mut kept: Vec<MonoItem<'tcx>> = Vec::with_capacity(items.len());
        for item in items {
            match item {
                MonoItem::Fn(_) | MonoItem::Static(_) => {
                    let key = mono_item_sort_key(tcx, item);
                    if seen.insert(key, ()).is_some() {
                        continue;
                    }
                    kept.push(item);
                }
                MonoItem::GlobalAsm(_) => kept.push(item),
            }
        }
        if !kept.is_empty() {
            plans.push(CguPlan {
                cgu_name: Some(cgu.name().to_string()),
                items: kept,
            });
        }
    }
    plans
}

/// 单对象模式的同名实例去重（首见保留）：排序后相邻同名（=同实例，
/// LocalCopy 在多个 CGU 重复附着）只留一个——与 Stage A「第二次
/// add_function 静默跳过」产物一致，但省一次重复降级。
fn dedup_by_symbol<'tcx>(
    tcx: TyCtxt<'tcx>,
    items: Vec<MonoItem<'tcx>>,
) -> Vec<MonoItem<'tcx>> {
    let mut out: Vec<MonoItem<'tcx>> = Vec::with_capacity(items.len());
    let mut seen: HashMap<String, ()> = HashMap::new();
    for item in items {
        match item {
            MonoItem::Fn(_) | MonoItem::Static(_) => {
                let key = mono_item_sort_key(tcx, item);
                if seen.insert(key, ()).is_some() {
                    continue;
                }
                out.push(item);
            }
            MonoItem::GlobalAsm(_) => out.push(item),
        }
    }
    out
}

/// MonoItem 的确定性排序键：mangled 符号名（GlobalAsm 恒排最后）。
fn mono_item_sort_key<'tcx>(tcx: TyCtxt<'tcx>, item: MonoItem<'tcx>) -> String {
    match item {
        MonoItem::Fn(instance) => {
            let def_id = instance.def_id();
            let instantiating_crate = if def_id.is_local() {
                rustc_hir::def_id::LOCAL_CRATE
            } else {
                def_id.krate
            };
            rustc_symbol_mangling::symbol_name_for_instance_in_crate(
                tcx,
                instance,
                instantiating_crate,
            )
            .to_string()
        }
        MonoItem::Static(def_id) => {
            let instantiating_crate = if def_id.is_local() {
                rustc_hir::def_id::LOCAL_CRATE
            } else {
                def_id.krate
            };
            rustc_symbol_mangling::symbol_name_for_instance_in_crate(
                tcx,
                ty::Instance::mono(tcx, def_id),
                instantiating_crate,
            )
            .to_string()
        }
        MonoItem::GlobalAsm(_) => "\u{10ffff}".to_string(),
    }
}

// ============================================================
// M3/M4/M6：函数降级任务（提交 rustc 查询池；M6 起按 CGU 分组）
// ============================================================

/// 单个函数实例的编译产出（错误不中止后续函数——主线程统一报告）。
struct FnOutcome {
    /// mangled 符号名（错误报告/发射用）。
    sym_name: String,
    /// 编译 + 就地 resolve 完成的结果（@N/G{N} 已全部替换为真实符号名）。
    result: Result<CompiledFunction, String>,
}

/// 一个函数任务的产出：函数结果（任务序 = 组内符号序）+ 任务私有
/// FuncRefTable（登记条目待主线程归并）+ 所属 plan 下标。
struct TaskOutcome {
    plan: usize,
    fns: Vec<FnOutcome>,
    table: FuncRefTable,
}

/// 单函数任务的带 panic 兜底入口（并行/串行两条路径共用同一任务体——
/// par_map 每个元素、串行 map 每个函数各调一次）。panic（ICE 类）→ 该
/// 函数记为 Err（不中止其它函数），主线程统一 dcx 报告后编译失败。
fn compile_fn_task_guarded<'tcx>(
    tcx: TyCtxt<'tcx>,
    plans: &[CguPlan<'tcx>],
    pi: usize,
    fi: usize,
) -> TaskOutcome {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compile_fn_task(tcx, plans, pi, fi)
    }))
    .unwrap_or_else(|payload| {
        // worker panic：该函数记为错误，主线程统一 dcx 报告后编译失败——
        // 不 abort（trace.rs panic hook 已打印现场上下文）。
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown worker panic".to_string()
        };
        TaskOutcome {
            plan: pi,
            fns: vec![FnOutcome {
                sym_name: format!("<worker panic @plan{pi} item{fi}>"),
                result: Err(format!("codegen worker panic: {msg}")),
            }],
            table: FuncRefTable::default(),
        }
    })
}

/// 编译 `plans[pi].items[fi]` 这个函数实例（组内 items 为 A1 符号序表）。
/// 任务内私有 FuncRefTable：@N/G{N} 编号由本表分配、每函数编译完就地
/// resolve 成真实符号名（本表只读查询），编号不跨函数逃逸 → 无需任何
/// 跨任务共享/加锁。tcx 查询（instance_mir/layout_of/arena 分配）由 rustc
/// 查询引擎执行——本函数可能运行在 rustc 查询池作业线程上（M4 par_map
/// 并行分支），也可运行在主线程（串行分支/非并行模式），两处都是 rustc
/// 注册线程（或主线程本身），查询 TLS 完整（WA-38/M4：自定义 std::thread
/// 无此上下文——见 codegen_crate 注释）。
fn compile_fn_task<'tcx>(
    tcx: TyCtxt<'tcx>,
    plans: &[CguPlan<'tcx>],
    pi: usize,
    fi: usize,
) -> TaskOutcome {
    let mut table = FuncRefTable::default();
    let mut fns = Vec::with_capacity(1);
    {
        let MonoItem::Fn(instance) = &plans[pi].items[fi] else {
            return TaskOutcome {
                plan: pi,
                fns,
                table,
            };
        };
        let def_id = instance.def_id();
        if crate::trace::trace_enabled("FN") {
            eprintln!("[forge] emit plan#{pi}[{fi}] {}", tcx.def_path_str(def_id),);
        }
        // 获取符号名 — 使用定义 crate 的 CrateNum 以确保
        // 外部实例（如跨 crate 单态化的泛型）的哈希与 rlib 一致
        let instantiating_crate = if def_id.is_local() {
            rustc_hir::def_id::LOCAL_CRATE
        } else {
            def_id.krate
        };
        let sym_name = rustc_symbol_mangling::symbol_name_for_instance_in_crate(
            tcx,
            *instance,
            instantiating_crate,
        );

        // MonoItem::Fn 实例（含跨 crate 单态化泛型、drop glue 等 shim）
        // 都有 MIR body 可用，统一降级；失败即报错，绝不产出桩函数。
        // 泛型实例的 body 需按实例替换（否则 Call 的 FnDef 常量 substs 含
        // 未替换参数，symbol_name_for_instance 会断言 panic）
        let result: Result<CompiledFunction, String> = (|| {
            let body = tcx.instance_mir(instance.def);
            let monomorphized = instance.instantiate_mir_and_normalize_erasing_regions(
                tcx,
                ty::TypingEnv::fully_monomorphized(),
                ty::EarlyBinder::bind(tcx, body.clone()),
            );
            let body = tcx.arena.alloc(monomorphized);
            let mut compiled = crate::compile::lower_and_compile(tcx, instance, body, &mut table)
                .map_err(|e| e.to_string())?;
            // 直接调用的重定位符号是 "@{FuncRef 序号}"，替换为真实符号名
            // GlobalAddr 重定位 "G{N}" → 真实数据符号名（两条 resolve 只读
            // 查询本任务表——就地完成，编号不跨函数逃逸）
            if crate::trace::trace_enabled("GLOBAL") {
                for r in &compiled.relocations {
                    eprintln!(
                        "[forge] reloc pre={} @{} addend={}",
                        r.symbol, r.offset, r.addend
                    );
                }
            }
            table.resolve_relocs(&mut compiled);
            table.resolve_global_relocs(&mut compiled);
            if crate::trace::trace_enabled("GLOBAL") {
                for r in &compiled.relocations {
                    if r.symbol.contains("CNT") {
                        eprintln!("[forge] reloc post={} @{}", r.symbol, r.offset);
                    }
                }
            }
            Ok(compiled)
        })();
        if crate::trace::trace_enabled("FN")
            && let Ok(code) = &result
        {
            let head: Vec<String> = code
                .code
                .iter()
                .take(8)
                .map(|b| format!("{b:02x}"))
                .collect();
            eprintln!(
                "[forge] size plan#{pi}[{fi}] {sym_name} code_bytes={} head=[{}]",
                code.code.len(),
                head.join(" ")
            );
        }
        fns.push(FnOutcome { sym_name, result });
    }
    TaskOutcome {
        plan: pi,
        fns,
        table,
    }
}
