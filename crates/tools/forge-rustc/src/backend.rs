//! CodegenBackend 外壳：rustc 入口 → 实例收集 → 逐实例降级编译 →
//! 对象文件写出 → 增量元数据（WorkProduct）。

use crate::alloc_runtime::build_alloc_runtime;
use crate::compile::{auto_register_isa_for_target, isa_name_for_target, lower_and_compile};
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

        // 模块级 FuncRef 表：直接调用的 "@N" 重定位 → 真实符号名
        let mut func_ref_table = FuncRefTable::default();
        // B1：per-function per-statement 行号表（(符号, [(机器码偏移, 行)])）
        let mut fn_line_tables: Vec<(String, Vec<(u32, u32)>)> = Vec::new();
        // CU high_pc：所有发射函数的代码总字节（含 main 别名副本——见下方
        // add_function("main")），dwarf CU DIE 范围用（gdb pc→CU 映射）。
        let mut code_span: u64 = 0;
        // 每函数代码字节（subprogram DW_AT_high_pc——gdb 需函数结束地址才能
        // 建 function block，缺则变量 DIE 被丢（对照 gcc 实证））。
        let mut fn_sizes: std::collections::HashMap<String, u64> = Default::default();

        for (item_i, item) in instances.iter().enumerate() {
            match item {
                MonoItem::Fn(instance) => {
                    let def_id = instance.def_id();
                    if crate::trace::trace_enabled("FN") {
                        eprintln!("[forge] emit#{item_i} {}", tcx.def_path_str(def_id),);
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
                    let body = tcx.instance_mir(instance.def);
                    let monomorphized = instance.instantiate_mir_and_normalize_erasing_regions(
                        tcx,
                        ty::TypingEnv::fully_monomorphized(),
                        ty::EarlyBinder::bind(tcx, body.clone()),
                    );
                    let body = tcx.arena.alloc(monomorphized);
                    match lower_and_compile(tcx, instance, body, &mut func_ref_table) {
                        Ok(mut compiled_func) => {
                            // 直接调用的重定位符号是 "@{FuncRef 序号}"，替换为真实符号名
                            func_ref_table.resolve_relocs(&mut compiled_func);
                            // GlobalAddr 重定位 "G{N}" → 真实数据符号名
                            if crate::trace::trace_enabled("GLOBAL") {
                                for r in &compiled_func.relocations {
                                    eprintln!(
                                        "[forge] reloc pre={} @{} addend={}",
                                        r.symbol, r.offset, r.addend
                                    );
                                }
                            }
                            func_ref_table.resolve_global_relocs(&mut compiled_func);
                            if crate::trace::trace_enabled("GLOBAL") {
                                for r in &compiled_func.relocations {
                                    if r.symbol.contains("CNT") {
                                        eprintln!("[forge] reloc post={} @{}", r.symbol, r.offset);
                                    }
                                }
                            }
                            let _ = object_writer.add_function(&sym_name, &compiled_func);
                            code_span += compiled_func.code.len() as u64;
                            fn_sizes.insert(sym_name.clone(), compiled_func.code.len() as u64);
                            // B1：收集该函数的 per-statement 行号表（主库 emission
                            // 输出 (机器码偏移, 行)）——debuginfo 开启时 dwarf.rs
                            // 生成 .debug_line 的每语句条目。
                            if !compiled_func.line_entries.is_empty() {
                                fn_line_tables
                                    .push((sym_name.clone(), compiled_func.line_entries.clone()));
                            }
                            if crate::trace::trace_enabled("GLOBAL") {
                                eprintln!("[forge] add_function sym={sym_name}");
                            }
                            if crate::trace::trace_enabled("FN") {
                                let code = compiled_func.code.len();
                                let head: Vec<String> = compiled_func
                                    .code
                                    .iter()
                                    .take(8)
                                    .map(|b| format!("{b:02x}"))
                                    .collect();
                                eprintln!(
                                    "[forge] size#{item_i} {sym_name} code_bytes={code} head=[{}]",
                                    head.join(" ")
                                );
                            }
                            // main 函数需要 C 名称别名（链接器入口点）。
                            // 用 def_path_str 而非 item_name——闭包/内部 shim 的 DefId
                            // 无 item_name（对 closure DefId 调用会 ICE）
                            if tcx.def_path_str(def_id).ends_with("::main") {
                                let _ = object_writer.add_function("main", &compiled_func);
                                code_span += compiled_func.code.len() as u64;
                            }
                        }
                        Err(e) => {
                            tcx.dcx()
                                .err(format!("code-forge: failed to compile '{sym_name}': {e}"));
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
        for (sym, bytes, relocs, align) in func_ref_table.vtables() {
            let reloc_refs: Vec<(usize, RelocKind, &str, i64)> = relocs
                .iter()
                .map(|(o, k, s, a)| (*o, *k, s.as_str(), *a))
                .collect();
            let _ = object_writer.add_data_with_relocs(sym, bytes, &reloc_refs, *align);
        }

        // promoted/slice 常量（`&"str"`、`&[1,2,3]` 字面量）：纯数据无重定位，
        // 落 .rodata（无 [WA-01] ADDR64 限制）。
        for (sym, bytes, align) in func_ref_table.promoted() {
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
