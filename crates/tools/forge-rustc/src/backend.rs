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

        for item in &instances {
            match item {
                MonoItem::Fn(instance) => {
                    let def_id = instance.def_id();

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
                                    eprintln!("[forge] reloc pre={} @{} addend={}", r.symbol, r.offset, r.addend);
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
                            if crate::trace::trace_enabled("GLOBAL") {
                                eprintln!("[forge] add_function sym={sym_name}");
                            }
                            // main 函数需要 C 名称别名（链接器入口点）。
                            // 用 def_path_str 而非 item_name——闭包/内部 shim 的 DefId
                            // 无 item_name（对 closure DefId 调用会 ICE）
                            if tcx.def_path_str(def_id).ends_with("::main") {
                                let _ = object_writer.add_function("main", &compiled_func);
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
                    tcx.dcx().err("code-forge: global_asm not supported");
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
    partitions
        .codegen_units
        .iter()
        .flat_map(|cgu| cgu.items().iter().map(|(item, _)| *item))
        .filter(|item| match item {
            MonoItem::Fn(instance) => {
                let def_id = instance.def_id();
                // 跳过 alloc crate 的 allocator shim（extern "C" 声明，C 符号名，
                // 会与手写 shim 重复定义；rustc 版内部依赖 Layout/Alignment lowering）
                let in_alloc = tcx.crate_name(def_id.krate).as_str() == "alloc";
                !(in_alloc && alloc_shims.contains(&tcx.item_name(def_id).as_str()))
            }
            _ => true,
        })
        .collect()
}
