//! CodegenBackend 外壳：rustc 入口 → 实例收集 → 逐实例降级编译 →
//! 对象文件写出 → 增量元数据（WorkProduct）。

use crate::alloc_runtime::build_alloc_runtime;
use crate::compile::{auto_register_isa_for_target, isa_name_for_target};
use crate::func_ref::FuncRefTable;
use crate::prelude::*;

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

        // 收集需要编译的实例
        let instances = collect_instances(tcx);

        if instances.is_empty() {
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

        // 创建对象文件写入器
        let config = TargetConfig::host().unwrap_or_default();

        let mut object_writer = ObjectWriter::new(&config).expect("create object writer");

        // ── M3/M4 Stage A（并行 CGU，-C codegen-units=N）：函数降级任务
        //    提交到 **rustc 查询池**（根治 WA-38 的 std::thread 限制）──
        // rustc 对 -Zcodegen-backend 只调 codegen_crate 一次、无按 CGU 回调
        // （LLVM 在自己的 codegen_crate 内并行，forge 不可复用）→ 函数粒度
        // 任务化：A1 排序实例表里的**每个函数**是一个独立任务，任务私有
        // FuncRefTable（@N/G{N} 编号只以重定位符号留机器码、每函数编译完
        // 就地 resolve——不跨函数逃逸，任务私有零加锁）+ 任务内登记的
        // vtable/promoted/line/var/enum 条目（merge_task_table 归并取回，
        // 按任务序=全局函数首见序，与旧串行路径一致）。主线程按原实例序
        // 串行 emission——单对象单模块语义不变、产物与 T=1 逐字节一致、
        // 与 -C codegen-units 无关。Static/GlobalAsm/alloc_runtime/dwarf
        // 照旧主线程串行。
        //
        // ⚠️ 宿主线程上下文（WA-38，M4 根治）：rustc 1.99/1.100 查询引擎
        // 统一为 WorkerLocal——tcx 查询只能在 rustc 自建池线程上执行
        // （WorkerLocal registry / 作业 TLV ImplicitCtxt / per-thread
        // SessionGlobals 三者只由 rustc 池线程持有且**无注册 API**）→ M3
        // 的 `std::thread` 自建池 + WorkerPayload 能力探针方案是死路
        // （2026-09 已删）。根治：直接复用 rustc 自家并行原语
        // `rustc_data_structures::sync::par_map`——rustc_codegen_ssa 在
        // -Z threads>=2（`sess.opts.jobs.frontend.is_some()`）时用同一原语
        // 把 per-CGU 编译作为**嵌套池作业**并行（该原语与 LLVM 无关）。
        // forge 的 codegen_crate 本身运行在 rustc 池作业线程上（可安全跑
        // tcx 查询），par_map 的嵌套作业继承全套线程上下文 → 单函数任务
        // 即为 par_map 的一个元素。par_* 在非并行模式（未设 -Z threads，
        // sync mode 未置位）自动退化为**当前线程按输入序串行**——行为与
        // 旧 T=1 恒等（默认路径与改造前逐字节一致）。
        let fn_positions: Vec<usize> = instances
            .iter()
            .enumerate()
            .filter(|(_, it)| matches!(it, MonoItem::Fn(_)))
            .map(|(i, _)| i)
            .collect();
        // M4 并行门控：env 显式要求并行（FORGE_CODEGEN_THREADS>1）&& rustc
        // 前端并行池存在（-Z threads>=2 / --jobs-frontend——`jobs.frontend`
        // 仅在 >1 时为 Some，interface.rs 据此 set_dyn_thread_safe_mode）
        // && 函数数 >1。三条件缺一即走串行 map（非并行模式 par_* 也是
        // 当前线程串行——两条路径共用同一单函数任务体，产物一致）。
        let forge_threads = match std::env::var("FORGE_CODEGEN_THREADS") {
            Ok(v) => v.trim().parse::<usize>().unwrap_or(0),
            Err(_) => 0,
        };
        let rustc_pool = tcx.sess.opts.jobs.frontend.is_some();
        let parallel = forge_threads > 1 && rustc_pool && fn_positions.len() > 1;
        if parallel {
            eprintln!(
                "[forge] FORGE_CODEGEN_THREADS>1 + rustc 并行前端（-Z threads）：\
                 {} 个函数降级任务提交到 rustc 查询池（par_map，确定序保序）",
                fn_positions.len()
            );
        } else if forge_threads > 1 && !rustc_pool && fn_positions.len() > 1 {
            // 仅提示不改行为：产物与串行路径一致（M4 后无"能力探针"可言——
            // par_* 在非并行模式自动串行，此处保持同一代码路径）。
            eprintln!(
                "[forge] FORGE_CODEGEN_THREADS={forge_threads} 但 rustc 无并行前端\
                 （-Z threads>=2，经 RUSTFLAGS 传入）——forge 函数并行需 rustc 查询池\
                 （WA-38/M4），当前串行编译（产物不变）"
            );
        }
        let tasks: Vec<TaskOutcome> = if parallel {
            rustc_data_structures::sync::par_map(
                fn_positions.iter().copied(),
                |i| compile_fn_task_guarded(tcx, &instances, i),
            )
        } else {
            // 串行：同一单函数任务体按原实例序逐任务跑（= 旧 T=1 路径的
            // 行为；par_map 非并行模式亦如此——此处显式走 map 使默认
            // 编译不依赖 rustc 的 dyn_thread_safe 全局态，T=1 恒等）。
            fn_positions
                .iter()
                .copied()
                .map(|i| compile_fn_task_guarded(tcx, &instances, i))
                .collect()
        };

        // 模块级 FuncRef 表：任务条目归并（任务按原实例序 = 全局符号序）→
        // alloc runtime 构建 + dwarf 读取共用。
        let mut func_ref_table = FuncRefTable::default();
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

        let mut fn_outcomes: Vec<FnOutcome> = Vec::with_capacity(fn_positions.len());
        for mut task in tasks {
            func_ref_table.merge_task_table(&mut task.table);
            fn_outcomes.append(&mut task.fns);
        }
        debug_assert_eq!(
            fn_outcomes.len(),
            fn_positions.len(),
            "M3: worker 归并结果数必须等于函数实例数"
        );

        // 单对象按原实例序串行 emission：Fn 消费预编译结果（任务归并序 =
        // 原实例序 = 全局符号序）、Static 现场求值——与旧串行路径逐条
        // 同序，产物不变。
        let mut fn_iter = fn_outcomes.into_iter();
        for item in instances.iter() {
            match item {
                MonoItem::Fn(instance) => {
                    let outcome = fn_iter.next().expect("M3: Fn emission 顺序错位");
                    match outcome.result {
                        Ok(compiled_func) => {
                            if crate::trace::trace_enabled("GLOBAL") {
                                eprintln!("[forge] add_function sym={}", outcome.sym_name);
                            }
                            let _ = object_writer.add_function(&outcome.sym_name, &compiled_func);
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
                                let _ = object_writer.add_function("main", &compiled_func);
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
                                let _ = object_writer.add_data(&sd.sym, &sd.data, sd.align);
                            } else {
                                let _ = object_writer.add_rodata(&sd.sym, &sd.data, sd.align);
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
        debug_assert!(fn_iter.next().is_none(), "M3: 残留未消费的 fn 结果");

        // 注入 alloc 运行时（__rust_alloc/dealloc/realloc/alloc_zeroed，基于
        // VirtualAlloc/VirtualFree），使 no_std + extern crate alloc 可用
        let isa_name = isa_name_for_target(&target_triple);
        match build_alloc_runtime(tcx, &mut func_ref_table, isa_name) {
            Ok(runtime_fns) => {
                for (sym, mut compiled) in runtime_fns {
                    func_ref_table.resolve_relocs(&mut compiled);
                    let _ = object_writer.add_function(&sym, &compiled);
                }
            }
            Err(e) => {
                tcx.dcx().err(format!("code-forge: alloc runtime: {e}"));
            }
        }

        // 写入 unsize cast 生成的 vtable 数据段（.data——方法指针表）
        // 注：[WA-01] MSVC 链接器对 .rodata 段的 ADDR64 重定位不应用
        // （实测链接后全 0），vtable 指针表必须放 .data 段
        // （add_data_with_relocs）才能正确解析方法地址。
        for (_, sym, bytes, relocs, align) in func_ref_table.vtables() {
            let reloc_refs: Vec<(usize, RelocKind, &str, i64)> = relocs
                .iter()
                .map(|(o, k, s, a)| (*o, *k, s.as_str(), *a))
                .collect();
            let _ = object_writer.add_data_with_relocs(sym, bytes, &reloc_refs, *align);
        }

        // promoted/slice 常量（`&"str"`、`&[1,2,3]` 字面量）：纯数据无重定位，
        // 落 .rodata（无 [WA-01] ADDR64 限制）。
        for (_, sym, bytes, align) in func_ref_table.promoted() {
            let _ = object_writer.add_rodata(sym, bytes, *align);
        }

        // C1/C2 DebugInfo：`-C debuginfo` 非 None 时生成 DWARF 段
        // （.debug_line/.debug_info/.debug_abbrev）并入主对象。
        // C1（=1 line-tables）：每函数行号；C2（=2 full）：+ 变量/参数 DIE
        //（lower_body 采集的 var_entries：名/槽偏移/类型/参数标志/行）。
        let debuginfo_on = tcx.sess.opts.debuginfo != rustc_session::config::DebugInfo::None;
        let debuginfo_full = tcx.sess.opts.debuginfo == rustc_session::config::DebugInfo::Full;
        if debuginfo_on && !func_ref_table.line_entries().is_empty() {
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
            // rustc_span::FileName 无 Display（1.100 已移除）；Real 变体经
            // local_path() 取磁盘路径。gdb `list`/源码断点按此打开文件。
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
            if let Err(e) = object_writer.add_dwarf(&dwarf_sections) {
                tcx.dcx()
                    .warn(format!("code-forge: dwarf emission failed: {e}"));
            }
        }

        // 写入对象文件到磁盘
        let obj_path = outdir.join("forge_codegen_output.o");
        match object_writer.write_to_file(&obj_path) {
            Ok(()) => {
                compiled_modules.push(CompiledModule {
                    name: "forge_codegen_output".to_string(),
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

/// 使用 rustc 单态化收集所有需要代码生成的实例（函数 + 静态数据）。
/// 返回按符号名排序的实例列表（A1 确定性：消除 rustc CGU 顺序/内部
/// HashMap 遍历对函数布局与 FuncRef 序号分配的影响——同一输入两次
/// 编译除地址外产物一致，便于回归比对与可复现调试）。
fn collect_instances<'tcx>(tcx: TyCtxt<'tcx>) -> Vec<MonoItem<'tcx>> {
    let partitions = tcx.collect_and_partition_mono_items(());

    // 遍历所有 codegen unit 收集实例（Fn 与 Static 都保留——Static 是
    // `const {allocN}` 引用的数据段符号，缺失会导致 static 读取错误）。
    // 跳过 alloc crate 的 allocator shim 实例（__rust_alloc/dealloc/realloc/
    // alloc_zeroed）：这些是 extern "C" 声明（符号名 = C 名），若编译会产生
    // 与手写 shim（build_alloc_runtime 注入的 forge-ir 版）重复定义冲突，
    // 且其 rustc 版内部依赖 Layout/Alignment 复杂 lowering。
    let alloc_shims = [
        "__rust_alloc",
        "__rust_dealloc",
        "__rust_realloc",
        "__rust_alloc_zeroed",
    ];
    let mut items: Vec<MonoItem<'tcx>> = partitions
        .codegen_units
        .iter()
        .flat_map(|cgu| cgu.items().iter().map(|(item, _)| *item))
        .filter(|item| match item {
            MonoItem::Fn(instance) => {
                let def_id = instance.def_id();
                // 跳过 alloc crate 的 allocator shim（extern "C" 声明，C 符号名，
                // 会与手写 shim 重复定义；rustc 版内部依赖 Layout/Alignment lowering）。
                // 注意：闭包 DefId 无 item_name（ICE），用 def_path_str 兜底——
                // allocator shim 是普通 fn，def_path_str 以 "__rust_alloc" 结尾。
                let in_alloc = tcx.crate_name(def_id.krate).as_str() == "alloc";
                if !in_alloc {
                    return true;
                }
                let name = if let Some(n) = tcx.opt_item_name(def_id) {
                    n.as_str().to_string()
                } else {
                    tcx.def_path_str(def_id)
                };
                !alloc_shims.contains(&name.as_str())
            }
            _ => true,
        })
        .collect();
    // E1 说明（WA-04 LNK2019）：裸 rustc 场景下，forge 编译的用户实例
    // （如 slice::iter::Iter::next）引用 core 泛型辅助函数（如
    // unchecked_sub::precondition_check），但 rustc collect 认为 core 由
    // 预编译 rlib 提供（rlib 无该符号）→ LNK2019。尝试沿调用图补生成
    // 失败：裸 rustc 下 core rlib 只有优化码，`instance_mir` 查询 core
    // 实例触发 ICE（"does not have optimized_mir"，panic_nounwind_fmt）。
    // 根治路径：README 已记录的 `-Zshare-generics=yes`（cargo build-std
    // 场景 LLVM 生成 core 实例）——裸 rustc 场景受 rustc 后端架构限制，
    // 无法用当前 CodegenBackend trait 干预 collect 阶段。slice_iter 类
    // 用例标记 known_failure（WA-19）。
    // A1：稳定排序——GlobalAsm 排最后（会报错终止），其余按 mangled 符号名。
    items.sort_by_key(|a| mono_item_sort_key(tcx, *a));
    items
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
// M3/M4 Stage A：并行 CGU——函数降级任务（提交 rustc 查询池）
// ============================================================

/// 单个函数实例的编译产出（错误不中止后续函数——主线程统一报告）。
struct FnOutcome {
    /// mangled 符号名（错误报告/发射用）。
    sym_name: String,
    /// 编译 + 就地 resolve 完成的结果（@N/G{N} 已全部替换为真实符号名）。
    result: Result<CompiledFunction, String>,
}

/// 一个函数任务的产出：函数结果（任务序 = 原实例序）+ 任务私有
/// FuncRefTable（登记条目待主线程 merge_task_table 归并）。
struct TaskOutcome {
    fns: Vec<FnOutcome>,
    table: FuncRefTable,
}

/// 单函数任务的带 panic 兜底入口（并行/串行两条路径共用同一任务体——
/// par_map 每个元素、串行 map 每个函数各调一次）。panic（ICE 类）→ 该
/// 函数记为 Err（不中止其它函数），主线程统一 dcx 报告后编译失败。
fn compile_fn_task_guarded<'tcx>(
    tcx: TyCtxt<'tcx>,
    instances: &[MonoItem<'tcx>],
    i: usize,
) -> TaskOutcome {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compile_fn_task(tcx, instances, i)
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
            fns: vec![FnOutcome {
                sym_name: format!("<worker panic @instance {i}>"),
                result: Err(format!("codegen worker panic: {msg}")),
            }],
            table: FuncRefTable::default(),
        }
    })
}

/// 编译第 `i` 个函数实例（`instances` 为 A1 符号序表；`i` 是其 Fn 下标）。
/// 任务内私有 FuncRefTable：@N/G{N} 编号由本表分配、每函数编译完就地
/// resolve 成真实符号名（本表只读查询），编号不跨函数逃逸 → 无需任何
/// 跨任务共享/加锁。tcx 查询（instance_mir/layout_of/arena 分配）由 rustc
/// 查询引擎执行——本函数可能运行在 rustc 查询池作业线程上（M4 par_map
/// 并行分支），也可运行在主线程（串行分支/非并行模式），两处都是 rustc
/// 注册线程（或主线程本身），查询 TLS 完整（WA-38/M4：自定义 std::thread
/// 无此上下文——见 codegen_crate 注释）。
fn compile_fn_task<'tcx>(
    tcx: TyCtxt<'tcx>,
    instances: &[MonoItem<'tcx>],
    i: usize,
) -> TaskOutcome {
    let mut table = FuncRefTable::default();
    let mut fns = Vec::with_capacity(1);
    {
        let MonoItem::Fn(instance) = &instances[i] else {
            return TaskOutcome { fns, table };
        };
        let def_id = instance.def_id();
        if crate::trace::trace_enabled("FN") {
            eprintln!("[forge] emit#{i} {}", tcx.def_path_str(def_id),);
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
                "[forge] size#{i} {sym_name} code_bytes={} head=[{}]",
                code.code.len(),
                head.join(" ")
            );
        }
        fns.push(FnOutcome { sym_name, result });
    }
    TaskOutcome { fns, table }
}
