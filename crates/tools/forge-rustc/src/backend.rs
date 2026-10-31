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
//! - **B-v2（debuginfo per-CGU CU）**：`-C debuginfo>=1` 不再强制单对象——
//!   每个有函数的 CGU 对象并入**独立 DWARF CU**（.debug_info/.debug_line/
//!   .debug_aranges/.debug_frame 等，CU 内 reloc 只引用本对象已定义函数符号，
//!   满足 add_dwarf 同文件定义约束）；纯数据对象不产 dwarf。
//!
//! 门控：env `FORGE_SINGLE_OBJECT=1` 或单 CGU（`-C codegen-units=1` 回归锚）
//! 回退 **Stage A 单对象路径**（跨 CGU 摊平 + 全局符号序 + 单 writer 尾注
//! 数据/dwarf——cgu=1 行为等价保持）。默认 rustc codegen-units=16，但非增量
//! 下 rustc 会合并过小 CGU（NON_INCR_MIN_CGU_SIZE，见 WA-39）——单模块小
//! crate 通常单 CGU → 单对象路径；显式 codegen-units + 多模块/大 crate 才
//! 产生多分区 → 多对象常态路径。

use crate::alloc_runtime::build_alloc_runtime;
use crate::compile::{auto_register_isa_for_target, isa_name_for_target};
use crate::func_ref::FuncRefTable;
use crate::prelude::*;
use rustc_middle::mono::{CodegenUnit, MonoItemPartitions};

/// 这些选项的"关闭"拼写（`Debug` 形式）。用拼写而不是变体路径比较：这些枚举在
/// rustc 内部**反复改名/搬家**（`Lto`→`LtoCli`、`ControlFlowGuard`→`CFGuard`），
/// 而"关"总归是 `Unspecified`（= 目标默认）/`No`/`Off`/`Disabled`/`None` 之一。
/// **认不出就当开着**（fail-closed：宁可多拒一次并让用户看到实际拼写，
/// 绝不静默放过一个安全特性开关）。
fn option_is_off(value: &impl std::fmt::Debug) -> bool {
    matches!(
        format!("{value:?}").as_str(),
        "Unspecified" | "No" | "Off" | "Disabled" | "None" | "Default"
    )
}

/// 挑出一个**本后端无法实现、且静默忽略会改变产物语义**的代码生成选项。
///
/// 返回 `Some(说明)` ⇒ 调用方按缺口 `CODEGEN_OPTS` 明确拒绝（fail-closed）。
/// 返回 `None` ⇒ 选项要么没开，要么是"我们的实现天然满足"的那几个
/// （`no-redzone`/`force-frame-pointers`/`split-debuginfo=packed`，见调用点注释）。
/// `--emit=obj` 时把对象从 rustc 的 temp 路径**再复制到最终产物路径**（用户 `-o`）。
///
/// 为什么必须自己复制：`rustc_codegen_ssa` 的 `copy_if_one_unit` 只在"走 ssa 自带
/// write 流程"的后端里执行；自定义 `CodegenBackend` 走的是 `codegen_crate` →
/// `link_binary`，**这一跳不会替我们复制** ⇒ `--emit=obj -o foo.obj` 静默不生效
/// （实测：只留下 temp 文件，用户要的文件根本不存在；LLVM 后端则正常）。
///
/// 目标路径取自 `output_filenames().path(Object)`：它优先返回用户的 `-o`
/// （`single_output_file`），否则用目录 + 文件名词干 + `.o`；与 `from` 相同就跳过。
fn copy_object_to_requested_output(tcx: TyCtxt<'_>, from: &std::path::Path) {
    use rustc_session::config::OutputType;
    if !tcx.sess.opts.output_types.contains_key(&OutputType::Object) {
        return;
    }
    let want = tcx.output_filenames(()).path(OutputType::Object);
    if want.is_stdout() {
        return;
    }
    let target = want.as_path();
    if target == from {
        return;
    }
    if let Err(e) = std::fs::copy(from, target) {
        tcx.dcx().err(format!(
            "code-forge: 无法把对象写到 `-o {}`：{e}",
            target.display()
        ));
        return;
    }
    // 只有"用户点名了一个具体输出文件"（`-o foo.obj`）时才收拾掉我们自己的那份：
    // 那是他自己要的产物，目录里不该再多一个名字。`--out-dir` 形态（没点名文件）
    // **保留** `forge_codegen_output*.o`——确定性与数据符号两类测试按这个前缀认产物。
    //（`-C save-temps` 一律保留，便于对拍。）
    let explicit_file = tcx.output_filenames(()).single_output_file.is_some();
    if explicit_file && !tcx.sess.opts.cg.save_temps {
        let _ = std::fs::remove_file(from);
    }
}

/// 用户请求了**本后端不产出**的产物种类？返回它的 `--emit=` 拼写。
///
/// 覆盖面：只判"需要 LLVM 侧产物"的那几种。`Exe`（链接）、`Object`（对象）、
/// `Metadata`（rustc 自己写）、`DepInfo`、`Mir` 都不在此列。
///
/// 为什么必须显式拒绝：`--emit=asm`/`--emit=llvm-ir` 在本后端上**静默什么都不生成**
/// （退出码 0，用户要的文件根本不存在）——实测如此。静默不产出比报错更伤人。
fn unsupported_emit_kind(tcx: TyCtxt<'_>) -> Option<&'static str> {
    use rustc_session::config::OutputType;
    // 固定顺序，保证报错稳定（output_types 的遍历序不定）。
    // 本 rustc 的 `OutputType` 变体全集：Assembly / Bitcode / DepInfo / Exe /
    // LlvmAssembly / Metadata / Mir / Object / ThinLinkBitcode（**没有** `LlvmIr`，
    // LLVM IR 就是 `LlvmAssembly`）——所以这里只列四个"需要 LLVM 侧产物"的。
    const ORDER: [(OutputType, &str); 4] = [
        (OutputType::Assembly, "asm"),
        (OutputType::LlvmAssembly, "llvm-ir"),
        (OutputType::Bitcode, "llvm-bc"),
        (OutputType::ThinLinkBitcode, "thin-link-bitcode"),
    ];
    ORDER
        .iter()
        .find(|(ty, _)| tcx.sess.opts.output_types.contains_key(ty))
        .map(|(_, name)| *name)
}

fn unsupported_codegen_option<'tcx>(tcx: TyCtxt<'tcx>) -> Option<String> {
    let cg = &tcx.sess.opts.cg;
    let opts = &tcx.sess.opts;

    // LTO / 位码：两者都建立在"后端吐 LLVM bitcode"之上，我们吐的是成品对象文件。
    // 注意：**不拒 `-C embed-bitcode`**。它在新版 rustc 里默认就是开（rlib 里存 bitcode
    // 供下游 LTO 用）——本后端不产出 bitcode，但这个选项不改变**本次产物**的语义，
    // 只是"给下游留不留料"。拒它会拒掉每一次正常构建（实测踩过）。
    if !option_is_off(&cg.lto) {
        return Some(format!(
            "`-C lto`（当前 {:?}）不支持：LTO 需要后端产出 LLVM bitcode，\
             本后端直达机器码/对象文件。请去掉 `-C lto`（跨 crate 优化本后端不做）",
            cg.lto
        ));
    }
    // 覆盖率 / 插桩 / PGO / CFG 防护 / sanitizer：全是 LLVM pass 或 IR 插桩的语义。
    if !option_is_off(&cg.instrument_coverage) {
        return Some(
            "`-C instrument-coverage` 不支持：覆盖率计数与 `__llvm_covmap` 段由 LLVM 插桩产生，\
             本后端没有该管线。请去掉该 flag（覆盖率请用 LLVM 后端构建）"
                .to_string(),
        );
    }
    if !opts.unstable_opts.sanitizer.is_empty() {
        return Some(format!(
            "`-Zsanitizer`（当前 {:?}）不支持：sanitizer 是在 IR 上插桩的 LLVM pass，\
             本后端没有该管线 ⇒ 静默忽略会让你以为有检测、实际没有。请去掉该 flag",
            opts.unstable_opts.sanitizer
        ));
    }
    if !option_is_off(&opts.unstable_opts.instrument_mcount) {
        return Some(
            "`-Z instrument-mcount` 不支持：函数入口计数插桩由 LLVM 产生。请去掉该 flag"
                .to_string(),
        );
    }
    if !option_is_off(&cg.profile_generate) {
        return Some(
            "`-C profile-generate`（PGO 采集）不支持：插桩由 LLVM 产生，本后端没有该管线。\
             请去掉该 flag"
                .to_string(),
        );
    }
    if cg.profile_use.is_some() {
        return Some(
            "`-C profile-use`（PGO 使用）不支持：profile 指导优化由 LLVM pass 实现。请去掉该 flag"
                .to_string(),
        );
    }
    if !option_is_off(&cg.control_flow_guard) {
        return Some(format!(
            "`-C control-flow-guard`（当前 {:?}）不支持：CFG 检测在 LLVM 里插入 \
             `__guard_check_icall` 检查，本后端没有该插桩 ⇒ 静默忽略等于**安全特性没开**。\
             请去掉该 flag",
            cg.control_flow_guard
        ));
    }
    // 动态链接 / 重定位模型：我们的对象层只按"静态链接 + MSVC 默认重定位模型"发射。
    if cg.prefer_dynamic {
        return Some(
            "`-C prefer-dynamic` 不支持：对象/链接层按**静态**链接 rustc 运行时发射。\
             请去掉该 flag"
                .to_string(),
        );
    }
    if let Some(model) = cg.relocation_model
        && !option_is_off(&model)
        && !matches!(format!("{model:?}").as_str(), "Pic")
    {
        return Some(format!(
            "`-C relocation-model={:?}` 不支持：对象层按 MSVC 默认（pic：绝对 `ADDR64` 重定位）\
             发射。目前只接受默认值（`pic` 即本机默认）；其他重定位模型需要对象层的重写",
            model
        ));
    }
    None
}

/// rustc 的 `-Zcodegen-backend` 入口实现。
pub struct CodegenLibBackend;

impl CodegenBackend for CodegenLibBackend {
    fn name(&self) -> &'static str {
        "code-forge"
    }

    fn target_cpu(&self, _sess: &Session) -> String {
        "generic".to_string()
    }

    /// **基线目标特性**（v27 M6-82）：申报本后端**确实会发**的指令集，而不是空集。
    ///
    /// 为什么要申报：rustc 用这份集合做**调用点检查**。`extern "C" fn() -> u128` 在
    /// win64 上按约定用 XMM0 返回，rustc 要求调用方具备 `sse` —— 空集会让这类签名在
    /// 前端就被拒（`this function call requires the sse target feature`），而我们的
    /// 后端本来就在发 SSE2（f32/f64 标量、XMM 传参/返回）。
    ///
    /// 只申报 **x86-64 基线**（该架构规范强制 SSE2）：不申报 AVX/AVX2/AVX-512——
    /// 那些要看具体机器能力（`FORGE_ASSUME_AVX512` 只影响向量降级策略，不该让
    /// `cfg(target_feature)` 说谎）。其它架构不申报任何特性（谱面自己决定）。
    fn target_config(&self, sess: &Session) -> rustc_codegen_ssa::TargetConfig {
        let mut internal_target_features = rustc_data_structures::unord::UnordSet::default();
        if sess.target.arch == rustc_target::spec::Arch::X86_64 {
            for f in ["sse", "sse2", "fxsr"] {
                internal_target_features.insert(rustc_span::Symbol::intern(f));
            }
        }
        rustc_codegen_ssa::TargetConfig {
            internal_target_features,
            has_reliable_f16: true,
            has_reliable_f16_math: true,
            has_reliable_f128: true,
            has_reliable_f128_math: true,
        }
    }

    fn codegen_crate<'tcx>(&self, tcx: TyCtxt<'tcx>) -> Box<dyn Any> {
        // **IR 优化管线（v27 M5/M6）**：此前 rustc 后端**从未**启用过 `forge-opt`
        // （`CompileSession::with_opt_level` 只在测试里被调用）⇒ 所有 MIR 局部都留在栈槽、
        // `-C opt-level` 对产物几乎没有影响。现在按 `-C opt-level` 映射到 `forge-opt` 档位，
        // 与 rustc 的语义对齐（0 = 不跑优化；1/2/3 = O1/O2/O3；s/z = O2）。
        //
        // `FORGE_OPT_LEVEL=0|1|2|3` 可**覆盖**（调试/对照用：`0` = 强制不优化）。
        {
            use code_forge::optimize::OptimizationLevel as L;
            let level = match std::env::var("FORGE_OPT_LEVEL").ok().as_deref() {
                Some("0") => None,
                Some("1") => Some(L::O1),
                Some("2") => Some(L::O2),
                Some("3") => Some(L::O3),
                // 未覆盖 ⇒ 跟随 rustc 的 `-C opt-level`。
                // 变体名在不同 nightly 上会漂（`Default` 曾改过名），而它只有 6 个稳定拼写
                // ⇒ 用 Debug 文本映射，未知拼写按 O2 兜底（优化但不激进）。
                _ => match format!("{:?}", tcx.sess.opts.optimize).as_str() {
                    "No" => None,
                    "Less" => Some(L::O1),
                    "Size" | "SizeMin" => Some(L::O2),
                    "Aggressive" => Some(L::O3),
                    _ => Some(L::O2),
                },
            };
            code_forge::backend::pipeline_hooks::ensure_registered();
            code_forge::backend::pipeline::compiler::opt_default::set_default_opt_level(level);
        }
        crate::trace::install_panic_hook();

        // v27 M4-② / `WA-04`：**用户显式**指定 `-Zshare-generics=yes` 时明确拒绝。
        //
        // 为什么在编译期拒绝：开了它以后，跨 crate 的泛型实例会被"共享"给上游 crate 生成，
        // 而本后端不生成辅助实例 ⇒ 到**链接期**才缺符号（LNK2019），错误离现场很远。
        // 只拒"用户显式指定"（`Some(true)`）；rustc 内部默认决策（`None`）不受影响。
        if tcx.sess.opts.unstable_opts.share_generics == Some(true) {
            tcx.dcx().err(format!(
                "{}",
                IrError::capability(
                    "SHARE_GENERICS",
                    "本后端不支持 `-Zshare-generics=yes`：跨 crate 的泛型实例不会被外部 crate \
                     生成，链接期会缺符号（LNK2019）。请去掉该 flag"
                )
            ));
            return Box::new(CompiledModules {
                modules: Vec::new(),
                allocator_module: None,
            });
        }

        // `[cap:CODEGEN_OPTS]`：**靠 LLVM 管线实现的代码生成选项一律明确拒绝**。
        //
        // 为什么必须拒绝而不是忽略：这些 flag 的语义都由 LLVM 后端实现（LTO 要吃 bitcode、
        // 覆盖率/`-Zsanitizer`/CFG 防护要在 IR 上插桩、PGO 要用 profile 指导优化）。
        // 本后端不吃 bitcode、也不跑那些 pass ⇒ 静默忽略会**悄悄产出与用户预期不同的
        // 产物**（最坏是安全问题：用户以为开了 CFG/ASAN）。实测（2026-10-19）这些 flag
        // 此前全部编译成功、一个都没报。
        //
        // 真正生效且我们**确实实现**的选项不在此列：`-C opt-level`（→ forge-opt）、
        // `-C debuginfo`（→ DWARF5）、`-C codegen-units`（→ 多 CGU）、`-C panic=abort`
        // （`panic=unwind` 另有 `[cap:UNWIND]` 拒绝）、`-C overflow-checks`（→ `Assert` 降级）、
        // `-C no-redzone`（我们从不使用红区）、`-C force-frame-pointers`（我们恒维护 RBP 帧指针）、
        // `-C split-debuginfo=packed`（调试信息本来就并进对象文件）。
        if let Some(msg) = unsupported_codegen_option(tcx) {
            tcx.dcx()
                .err(format!("{}", IrError::capability("CODEGEN_OPTS", msg)));
            return Box::new(CompiledModules {
                modules: Vec::new(),
                allocator_module: None,
            });
        }
        // **产物种类**：本后端只产出**对象文件**（`--emit=obj` / `--emit=link`）。其余
        // LLVM 专属产物我们不产出——此前是**静默什么都不生成**（退出码 0、用户要的文件
        // 根本不存在）：`--emit=asm` / `--emit=llvm-ir` 实测就是"成功但没产物"。
        if let Some(kind) = unsupported_emit_kind(tcx) {
            tcx.dcx().err(format!(
                "{}",
                IrError::capability(
                    "EMIT_KINDS",
                    format!(
                        "`--emit={kind}` 不支持：本后端直接产出机器码对象                         （`--emit=obj`/`--emit=link`）；汇编 / LLVM IR / bitcode 需要 LLVM 侧产物，                         我们既不生成也不伪造（静默不产出比报错更糟）"
                    )
                )
            ));
            return Box::new(CompiledModules {
                modules: Vec::new(),
                allocator_module: None,
            });
        }
        // 1. 从环境变量加载外部 ISA 插件
        let _plugin_loader = code_forge::forge_plugin::PluginLoader::load_from_env();

        // 2. 根据目标三元组自动注册内置 ISA 后端
        let target_triple = format!("{}", tcx.sess.opts.target_triple);
        auto_register_isa_for_target(&target_triple);

        let mut compiled_modules = Vec::new();

        // G5 / v27 M2：`panic=unwind` **明确拒绝**（当前后端只支持 panic=abort——没有
        // SEH 展开表（`.pdata`/`.xdata`）、没有 `__rust_eh_personality`、也没有 rustc
        // 侧的 landing pad 降级 ⇒ 真按 unwind 编会产出未定义行为/链接错误）。
        // v27 M2 起这条拒绝**带缺口 ID**（进 `backend-gaps.md` 台账、被
        // `capability_gaps_guard` 双向钉住）——原来只是一个裸字符串，既不在台账里、
        // 也没有守卫覆盖（读者无法从缺口全貌里看到它）。
        let panic_abort = tcx.sess.panic_strategy() == rustc_target::spec::PanicStrategy::Abort;
        if !panic_abort {
            tcx.dcx().err(format!(
                "code-forge: {}",
                IrError::capability(
                    "UNWIND",
                    "panic=unwind 不支持：缺 SEH 展开表（.pdata/.xdata）与 __rust_eh_personality，                     请用 -C panic=abort（决策见 backend-gaps.md 的 UNWIND 行）"
                )
            ));
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

        // B-v2 门控：debuginfo 关闭或开启都可多对象（开启时每 CGU 对象带
        // 独立 DWARF CU——per-CGU CU，reloc 目标符号=本对象内定义，见下方
        // dwarf 块）；FORGE_SINGLE_OBJECT=1 逃生口强制单对象；单 CGU 天然
        // 单对象（`-C codegen-units=1` 回归锚 = Stage A 语义）。
        let debuginfo_on = tcx.sess.opts.debuginfo != rustc_session::config::DebugInfo::None;
        let debuginfo_full = tcx.sess.opts.debuginfo == rustc_session::config::DebugInfo::Full;
        let force_single = std::env::var("FORGE_SINGLE_OBJECT")
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
        let multi_object = !force_single && codegen_units.len() > 1;

        // 实例分组计划：单对象 = 1 个（跨 CGU 摊平）计划；多对象 = 每 CGU 一个
        //（组内过滤 allocator shim 等 + 按符号名排序 + 跨 CGU 同名去重）。
        let mut plans = build_plans(tcx, codegen_units, multi_object);
        // v27 M2：`global_asm!` 的汇编文本收集器（跨 CGU 汇总成**一份**对象——
        // 与 cranelift 后端同构：文本拼一起、交系统汇编器、报 global_asm_object）。
        let mut global_asm_text: Option<crate::global_asm::GlobalAsmText> = None;

        // v27 TLS：先扫一遍计划，决定要不要生成动态 TLS 运行时（无 TLS ⇒ 不留死代码）。
        // 必须在循环前算：运行时要落到**首个** CGU 对象里，而 TLS 静态可能出现在任何 CGU。
        let tls_any = plans.iter().any(|p| {
            p.items.iter().any(|item| match item {
                MonoItem::Static(d) => tcx.is_thread_local_static(*d),
                _ => false,
            })
        });

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

        // 创建对象文件写入器（每 CGU/每对象独立实例——自包含可多实例）。
        //
        // **必须按 rustc 的 `--target` 建，不能 `TargetConfig::host()`**（v27 M6-98）：
        // 后者让 `--target aarch64-unknown-none` 也写出 **host 的 COFF/x86-64** 对象
        // （实测：段格式/机器类型全是 x86_64，A64 机器码套在 COFF 里，链接器直接拒）。
        let config = match TargetConfig::from_triple(&target_triple) {
            Ok(c) => c,
            Err(e) => {
                tcx.dcx().err(format!(
                    "code-forge: 目标 `{target_triple}` 的对象格式不受支持：{e}"
                ));
                return Box::new(CompiledModules {
                    modules: Vec::new(),
                    allocator_module: None,
                });
            }
        };

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
        let mut tasks: Vec<TaskOutcome<'tcx>> = if parallel {
            rustc_data_structures::sync::par_map(work.iter().copied(), |(pi, fi)| {
                compile_fn_task_guarded(tcx, &plans, pi, fi)
            })
        } else {
            work.iter()
                .copied()
                .map(|(pi, fi)| compile_fn_task_guarded(tcx, &plans, pi, fi))
                .collect()
        };

        // ── 按需补编：`AvailableExternally` 实例（-O1+ 的 MIR inliner 缺口）──
        // rustc 在 -O1+ 会把 `#[inline]` 实例内联进调用者、并把独立定义标成
        // available-externally（**不进 CGU 分区**）；forge 优化器没有内联 pass，
        // 直接调用点会留下未定义符号（sort 的 `CopyOnDrop::drop`/`revswap`）。
        // 这里用 lowering 期间登记的 (符号名, Instance) 做 fixpoint，把缺失实例
        // 补编到**第一个 CGU**（不新增 CGU/对象：多对象与 DWARF per-CGU 的
        // 对象数契约不变）。
        let mut extra_items: Vec<MonoItem<'tcx>> = Vec::new();
        let mut extra_outcomes: Vec<TaskOutcome<'tcx>> = Vec::new();
        {
            const EXTRA_PI: usize = 0;
            let mut emitted: std::collections::HashSet<String> = tasks
                .iter()
                .flat_map(|t| t.fns.iter())
                .filter(|f| f.result.is_ok())
                .map(|f| f.sym_name.clone())
                .collect();
            let mut seen: std::collections::HashSet<String> = emitted.clone();
            let mut queue: Vec<(String, Instance<'tcx>)> = Vec::new();
            for t in &tasks {
                for f in &t.fns {
                    queue.extend(f.referenced.iter().cloned());
                }
            }
            loop {
                let mut newly: Vec<(String, Instance<'tcx>)> = Vec::new();
                for (sym, inst) in queue.drain(..) {
                    if emitted.contains(&sym) || !seen.insert(sym.clone()) {
                        continue;
                    }
                    // 上游 crate 已提供定义（非泛型/非 inline 的外部项，如
                    // `core::panicking::panic`）：不在本 crate 发射，也不能按需补编
                    //（`instance_mir` 对这种实例没有 MIR）。`upstream_monomorphization`
                    // 表达的正是"上游会 codegen"；`#[inline]`/泛型实例返回 None
                    // ——这些即使 impl 无泛型参数（`<i32 as Ord>::cmp`）也必须在本
                    // crate 单态化发射（kitchen 探针实测：漏了会链接缺符号）。
                    if inst.upstream_monomorphization(tcx).is_some() {
                        continue;
                    }
                    // 没有 MIR 的实例（`extern` 块声明、无 body 的项）也不能补编：
                    // `instance_mir` 会 ICE（`can't type-check body of …`）。
                    if !tcx.is_mir_available(inst.def_id()) {
                        continue;
                    }
                    newly.push((sym, inst));
                }
                if newly.is_empty() {
                    break;
                }
                for (sym, inst) in newly {
                    if crate::trace::trace_enabled("FN") {
                        eprintln!("[forge] on-demand instance {sym}");
                    }
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        compile_instance(tcx, &inst, EXTRA_PI)
                    }))
                    .unwrap_or_else(|_| TaskOutcome {
                        plan: EXTRA_PI,
                        fns: Vec::new(),
                        table: FuncRefTable::default(),
                    });
                    // 补编失败（含 worker panic）⇒ 不把这个 item 放进 CGU0：
                    // 发射按 `plan.items` 与 outcomes 对位，空 outcome 会错位。
                    if outcome.fns.is_empty() {
                        continue;
                    }
                    for f in &outcome.fns {
                        queue.extend(f.referenced.iter().cloned());
                        if f.result.is_ok() {
                            emitted.insert(f.sym_name.clone());
                        }
                    }
                    extra_items.push(MonoItem::Fn(inst));
                    extra_outcomes.push(outcome);
                }
            }
            if extra_items.is_empty() {
                seen.clear();
            }
        }
        if !extra_items.is_empty() {
            if crate::trace::trace_enabled("FN") {
                eprintln!(
                    "[forge] on-demand: {} extra instances appended to CGU0",
                    extra_items.len()
                );
            }
            // 追加到 CGU0 的 items 末尾，并把对应 TaskOutcome 插到 CGU0 任务段之后
            //（归并循环按 plan 连续消费 tasks）。
            let split = tasks.iter().take_while(|t| t.plan == 0).count();
            for (k, mut t) in extra_outcomes.into_iter().enumerate() {
                t.plan = 0;
                tasks.insert(split + k, t);
            }
            plans[0].items.extend(extra_items);
        }
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
        let mut outcomes_by_plan: Vec<Vec<FnOutcome<'tcx>>> = Vec::new();
        outcomes_by_plan.resize_with(plans.len(), Vec::new);
        {
            let mut task_iter = tasks.into_iter().peekable();
            let mut total_fns = 0usize;
            for (pi, plan_outcomes) in outcomes_by_plan.iter_mut().enumerate() {
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
                            debug_assert_eq!(
                                old_bytes, &bytes,
                                "same vtable sym must have same bytes"
                            );
                            debug_assert_eq!(
                                old_relocs, &relocs,
                                "same vtable sym must have same relocs"
                            );
                            debug_assert_eq!(
                                old_align, &align,
                                "same vtable sym must have same align"
                            );
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
                            debug_assert_eq!(
                                old_bytes, bytes,
                                "same promoted sym must have same bytes"
                            );
                            debug_assert_eq!(
                                old_align, align,
                                "same promoted sym must have same align"
                            );
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
                    plan_outcomes.append(&mut t.fns);
                }
            }
            debug_assert!(
                task_iter.next().is_none(),
                "M6: 残留未归并的任务（plans/任务序错位）"
            );
            // 计划里的 Fn 实例数 = 初始 work + 按需补编追加到 CGU0 的实例。
            let expected_fns = plans
                .iter()
                .flat_map(|p| p.items.iter())
                .filter(|item| matches!(item, MonoItem::Fn(_)))
                .count();
            debug_assert_eq!(
                total_fns, expected_fns,
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
        // B-v2（per-CGU CU）：CU high_pc = 各对象发射函数代码总字节（含 main
        // 别名副本——见下方 add_function("main")）——**每 plan 独立**：dwarf
        // CU DIE 范围用（gdb pc→CU 映射）。
        let mut plan_code_spans: Vec<u64> = vec![0; plans.len()];
        // 每对象成功发射的函数符号集（dwarf 行/变量条目按对象过滤用——
        // per-CGU CU 只含本对象定义的符号，add_dwarf 的 reloc 目标因此
        // 同文件可解析）。
        let mut plan_fn_syms: Vec<std::collections::HashSet<String>> = (0..plans.len())
            .map(|_| std::collections::HashSet::new())
            .collect();
        // 每函数代码字节（subprogram DW_AT_high_pc——gdb 需函数结束地址才能
        // 建 function block，缺则变量 DIE 被丢（对照 gcc 实证））。
        let mut fn_sizes: std::collections::HashMap<String, u64> = Default::default();
        // M2：每函数 .debug_frame CFI（符号, 代码字节, 行集——emission 对
        // x86 prologue 扫描产物；与 fn_sizes 同键控、同迭代序收集；无 CFI
        //（非 x86/形态不符）不进表 → dwarf.rs 只对这些符号产 FDE）。
        let mut fn_cfi: Vec<(String, u64, FunctionCfi)> = Vec::new();

        // B-v2：debuginfo 上下文的（producer, 源文件路径）——与 CGU 无关，
        // 每 CU 复用同一串（每 CGU 一个 DWARF CU、共享同一源文件）。
        let dwarf_ctx = if debuginfo_on {
            let producer = format!(
                "code-forge {} (rustc {})",
                env!("CARGO_PKG_VERSION"),
                option_env!("CFG_VERSION").unwrap_or("")
            );
            // 源文件名：CU DW_AT_name + .debug_line file 条目必须指向磁盘上的
            // 真实源文件（gdb `list`/源码断点按此打开文件；crate 名匹配不到
            // .rs 文件）。取本地 crate 根模块所在文件。
            // rustc_span::FileName 无 Display（1.100 已移除）；Real 变体经
            // local_path() 取磁盘路径。
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
            Some((producer, src_file))
        } else {
            None
        };

        // v27 M4-T1：没有该目标的 ISA 后端 ⇒ **明确拒绝**（此前会静默回落 x86_64）。
        let Some(isa_name) = isa_name_for_target(&target_triple) else {
            tcx.dcx().err(format!(
                "{}",
                IrError::capability(
                    "TARGET",
                    format!(
                        "本后端没有目标 `{target_triple}` 的 ISA 后端（已支持：x86_64 / aarch64）"
                    )
                )
            ));
            return Box::new(CompiledModules {
                modules: Vec::new(),
                allocator_module: None,
            });
        };
        // 目标约定的**链接层重定位表**（数据来自 conventions/*.toml）：对象层只
        // 按 `(格式, ISA 语义名)` 查表落链接器类型，没有 arch 分支（v27 M6-98）。
        let link_table = match code_forge::backend::pipeline::conv_registry::link_table(
            isa_name,
            config.link_format(),
        ) {
            Ok(t) => t,
            Err(e) => {
                tcx.dcx().err(format!("code-forge: {e}"));
                return Box::new(CompiledModules {
                    modules: Vec::new(),
                    allocator_module: None,
                });
            }
        };
        for (pi, plan) in plans.iter().enumerate() {
            if crate::trace::trace_enabled("CGU") {
                eprintln!(
                    "[forge] plan#{pi} cgu={} items={}",
                    plan.cgu_name.as_deref().unwrap_or("<single>"),
                    plan.items.len()
                );
            }
            let mut writer = match ObjectWriter::new(&config) {
                Ok(w) => w.with_link_table(link_table.clone()),
                Err(e) => {
                    tcx.dcx().err(format!(
                        "code-forge: 目标 `{target_triple}` 的对象文件无法创建：{e}"
                    ));
                    continue;
                }
            };
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
                                plan_code_spans[pi] += compiled_func.code.len() as u64;
                                plan_fn_syms[pi].insert(outcome.sym_name.clone());
                                fn_sizes.insert(
                                    outcome.sym_name.clone(),
                                    compiled_func.code.len() as u64,
                                );
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
                                    plan_code_spans[pi] += compiled_func.code.len() as u64;
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
                        // v27 M2：**线程局部静态明确拒绝**。此前它会走下面的普通 static
                        // 路径 → 发射成 `.data`/`.rodata` ⇒ **线程私有语义静默丢失**（所有
                        // 线程共享一份），与「宁可明确拒绝，不要静默错值」冲突。
                        // 真实现需要：`.tls$` 段 + TLS 回调/`__tls_index`（Windows）或
                        // `%fs:` 相对寻址 + 访问模型的降级（见 backend-gaps.md 的 TLS 行）。
                        if reject_link_section(tcx, *def_id) {
                            continue;
                        }
                        if tcx.is_thread_local_static(*def_id) {
                            // v27 TLS（动态模型）：线程局部静态**不**发普通 .data 符号——
                            // 发两个隐藏符号：初值模板 `__forge_tls_init_<sym>` 与索引槽
                            // `__forge_tls_slot_<sym>`；地址物化走动态 TLS 运行时
                            // （见 `tls_runtime.rs` 与 `lower::const_data_addr`）。
                            match crate::global_data::build_static_data(tcx, *def_id) {
                                Ok(sd) => {
                                    let init_sym = crate::tls_runtime::tls_init_sym(&sd.sym);
                                    let slot_sym = crate::tls_runtime::tls_slot_sym(&sd.sym);
                                    let _ = writer.add_data(&init_sym, &sd.data, sd.align);
                                    let _ = writer.add_data(&slot_sym, &[0u8; 8], 8);
                                }
                                Err(msg) => {
                                    let _ = tcx.dcx().err(format!("code-forge: {msg}"));
                                }
                            }
                            continue;
                        }
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
                    MonoItem::GlobalAsm(item_id) => {
                        // v27 M2：不再是 `[WA-03] 不支持`——把模板展开进收集器，
                        // 循环外统一汇编成**独立对象**交给 rustc 链接（见 global_asm.rs）。
                        let collector = global_asm_text
                            .get_or_insert_with(|| crate::global_asm::GlobalAsmText::new(isa_name));
                        if let Err(e) = collector.push_item(tcx, *item_id) {
                            tcx.dcx().err(format!("code-forge: {e}"));
                        }
                    }
                }
            }
            debug_assert!(plan_fns.next().is_none(), "M6: 残留未消费的 fn 结果");

            // v27 TLS：动态 TLS 运行时（只在真有线程局部静态时生成）。
            if pi == 0 && tls_any {
                match crate::tls_runtime::build_tls_runtime(&mut func_ref_table, isa_name) {
                    Ok(runtime_fns) => {
                        for (sym, mut compiled) in runtime_fns {
                            func_ref_table.resolve_relocs(&mut compiled);
                            let _ = writer.add_function(&sym, &compiled);
                        }
                    }
                    Err(e) => {
                        let _ = tcx
                            .dcx()
                            .err(format!("code-forge: TLS 运行时生成失败：{e}"));
                    }
                }
            }

            // 注入 alloc 运行时（__rust_alloc/dealloc/realloc/alloc_zeroed，基于
            // VirtualAlloc/VirtualFree），使 **no_std + extern crate alloc** 可用。
            // Stage A 语义 = 单对象尾注；Stage B 多对象 = 首 CGU 对象（确定性
            // 归属——无跨对象重复定义）。
            //
            // v27 M3 / `WA-02`：**链接 std 的 crate 不能注入**——std 的 rlib 自带
            // `__rust_alloc` 一族，再注入我们的手写 shim 就是重复定义（LNK2005）。
            // 这就是 `WA-02` 的正解：不是"过滤掉 rustc 侧的实例"，而是**按需注入**
            // （no_std 时注入，链 std 时让 std 自己提供）。判定用 `tcx.crates(())`
            // ——与 `crate_name` 同源的现成惯用法（见本文件 `in_alloc` 处）。
            let links_std = tcx
                .crates(())
                .iter()
                .any(|&c| tcx.crate_name(c).as_str() == "std");
            // **目标门控**（v27 M6-98）：这个 shim 的实现是 Windows 的
            // `VirtualAlloc/VirtualFree`（x64 约定），对 aarch64/其它目标既编不出
            // （降级层 = 对应 ISA 的规则集）也没有意义 ⇒ 按**运行时自己声明的
            // 可用面**（manifest 的 `alloc_shim` + Windows）注入，
            // 而不是把 `arch == X86_64` 写进 codegen 后端（那等于给指令集开后门）。
            let target_arch = format!("{:?}", tcx.sess.target.arch).to_lowercase();
            let alloc_runtime_ok = tcx.sess.target.is_like_windows
                && code_forge::backend::arch::has_alloc_shim(&target_arch);
            if pi == 0 && !links_std && alloc_runtime_ok {
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

            // C1/C2 DebugInfo（B-v2 per-CGU CU）：`-C debuginfo` 非 None 时
            // **每个有函数的对象**生成独立 DWARF CU（.debug_line/.debug_line_str/
            // .debug_info/.debug_aranges/.debug_abbrev/.debug_frame）并入该对象
            // ——CU 内引用的符号 = 本对象已定义的函数（add_function 已登记），
            // 满足 add_dwarf 的同文件定义约束；纯数据对象不产 dwarf。
            // 单对象模式 = 单个完整 CU（与 Stage A 逐字节等价——per-plan
            // 子集 = 全集，顺序/字节不变）。
            if debuginfo_on && !func_ref_table.line_entries().is_empty() {
                let sym_set = &plan_fn_syms[pi];
                // B1：entries（每函数声明行）+ fn_line_tables（per-statement）→
                // fns: (符号, 声明行, [(指令偏移, 行)])——dwarf gen_debug_line
                // 生成函数级 + 每语句行号条目（COFF addend 隐式：占位写偏移）。
                // 按本对象函数集过滤（保留全局登记序 = 组内符号序）。
                let entries: Vec<(String, u32)> = func_ref_table
                    .line_entries()
                    .iter()
                    .filter(|(s, _)| sym_set.contains(s))
                    .cloned()
                    .collect();
                if !entries.is_empty() {
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
                    let vars: Vec<crate::dwarf::FnVarEntries> = func_ref_table
                        .var_entries()
                        .iter()
                        .filter(|fv| sym_set.contains(&fv.sym))
                        .cloned()
                        .collect();
                    let fn_cfi_plan: Vec<(String, u64, FunctionCfi)> = fn_cfi
                        .iter()
                        .filter(|(s, _, _)| sym_set.contains(s))
                        .cloned()
                        .collect();
                    let (producer, src_file) =
                        dwarf_ctx.as_ref().expect("debuginfo_on ⇒ dwarf_ctx 存在");
                    let sections = crate::dwarf::build_dwarf_sections(
                        &fns,
                        &vars,
                        producer,
                        src_file,
                        plan_code_spans[pi],
                        &fn_sizes,
                        func_ref_table.enum_types(),
                        debuginfo_full,
                        &fn_cfi_plan,
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
            }

            // 写入对象文件到磁盘（单对象 = forge_codegen_output.o——Stage A 的同名锚，
            // 多对象 = forge_codegen_output.{cgu 名}.o）。**这个名字保持不变**：
            // 确定性与数据符号两类测试都按 `forge_codegen_output*` 前缀认产物。
            // 用户要的 `--emit=obj -o <path>` 由随后的 `copy_object_to_requested_output`
            // 补一份（自定义后端不走 `rustc_codegen_ssa` 的 `copy_if_one_unit`）。
            let module_name = match &plan.cgu_name {
                Some(cgu) => format!("forge_codegen_output.{cgu}"),
                None => "forge_codegen_output".to_string(),
            };
            let obj_path = outdir.join(format!("{module_name}.o"));
            match writer.write_to_file(&obj_path) {
                Ok(()) => {
                    // 只有**单对象**才复制到用户路径：多 CGU 时 rustc 自己的口径是
                    // "警告 + 保留编号文件"（`IgnoringOutput`），逐个 CGU 覆盖同一目标
                    // 会静默丢掉其它 CGU 的代码。
                    if plan.cgu_name.is_none() {
                        copy_object_to_requested_output(tcx, &obj_path);
                    }
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

        // v27 M2：把收集到的 global_asm 文本汇编成对象，并以
        // `CompiledModule::global_asm_object` 报给 rustc（rustc 的 link.rs 会把它
        // 加进链接命令）。失败**不静默**：`dcx().err` 并点名缺口 ID。
        if let Some(collector) = &global_asm_text
            && !collector.is_empty()
        {
            match collector.assemble(&outdir) {
                Ok(obj) => {
                    if let Some(first) = compiled_modules.first_mut() {
                        first.global_asm_object = Some(obj);
                        crate::trace::trace_enabled("GLOBAL").then(|| {
                            eprintln!(
                                "[forge] global_asm: {} 项已汇编为独立对象并报给 rustc",
                                collector.count()
                            )
                        });
                    } else {
                        // 没有对象模块（不应发生：至少有一个 CGU 计划）——不静默。
                        let _ = tcx.dcx().err(
                            "code-forge: [cap:GLOBAL_ASM] 有 global_asm 项但没有对象模块可挂载",
                        );
                    }
                }
                Err(e) => {
                    let _ = tcx.dcx().err(format!("code-forge: {e}"));
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
fn dedup_by_symbol<'tcx>(tcx: TyCtxt<'tcx>, items: Vec<MonoItem<'tcx>>) -> Vec<MonoItem<'tcx>> {
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
struct FnOutcome<'tcx> {
    /// mangled 符号名（错误报告/发射用）。
    sym_name: String,
    /// 编译 + 就地 resolve 完成的结果（@N/G{N} 已全部替换为真实符号名）。
    result: Result<CompiledFunction, String>,
    /// 本函数**直接引用**的被调实例（符号名 + Instance）：`-O1+` 时 rustc 的
    /// `#[inline]` 实例可能被 MIR inliner 内联掉、不进 CGU 分区；forge 没有
    /// 内联 pass ⇒ 需要按需补编这些实例（`codegen_crate` 的 fixpoint）。
    referenced: Vec<(String, Instance<'tcx>)>,
}

/// 一个函数任务的产出：函数结果（任务序 = 组内符号序）+ 任务私有
/// FuncRefTable（登记条目待主线程归并）+ 所属 plan 下标。
struct TaskOutcome<'tcx> {
    plan: usize,
    fns: Vec<FnOutcome<'tcx>>,
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
) -> TaskOutcome<'tcx> {
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
                referenced: Vec::new(),
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
/// `#[link_section]` 会被本后端**静默忽略**（它只发 `.text`/`.data`/`.rodata` 三段）
/// ⇒ 这里**明确拒绝**。缺口 ID 只在这一处出现（`capability_gaps_guard` 要求恰好一处）。
fn reject_link_section(tcx: TyCtxt<'_>, def_id: rustc_hir::def_id::DefId) -> bool {
    let Some(section) = tcx.codegen_fn_attrs(def_id).link_section else {
        return false;
    };
    tcx.dcx().err(format!(
        "{}",
        IrError::capability(
            "LINK_SECTION",
            format!(
                "`#[link_section = \"{section}\"]` 不支持：本后端只发射 .text/.data/.rodata \
                 三段（段名/段标志由发射层决定），放错段既不报错也不生效。请去掉该属性；\
                 或把它迁到对象层的\"任意段\"支持（方案 M4-T4）"
            )
        )
    ));
    true
}

fn compile_fn_task<'tcx>(
    tcx: TyCtxt<'tcx>,
    plans: &[CguPlan<'tcx>],
    pi: usize,
    fi: usize,
) -> TaskOutcome<'tcx> {
    let MonoItem::Fn(instance) = &plans[pi].items[fi] else {
        return TaskOutcome {
            plan: pi,
            fns: Vec::new(),
            table: FuncRefTable::default(),
        };
    };
    compile_instance(tcx, instance, pi)
}

/// 编译单个 `Instance`（CGU 分区里的普通函数任务，或 `-O1+` 缺失实例的
/// **按需补编**共用）。`plan` 只影响产出分组/诊断。
fn compile_instance<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: &Instance<'tcx>,
    pi: usize,
) -> TaskOutcome<'tcx> {
    let mut table = FuncRefTable::default();
    let mut fns = Vec::with_capacity(1);
    let mut referenced: Vec<(String, Instance<'tcx>)> = Vec::new();
    // 目标校验已在 `codegen_crate` 入口做过（缺口 ID `TARGET` 只在那里出现一次）⇒
    // 这里是**不变量**：走到这里说明目标已被接受。推导失败即内部错误，不静默回落。
    let isa_name = crate::compile::isa_name_for_target(&format!("{}", tcx.sess.opts.target_triple))
        .expect("codegen_crate 已校验目标：这里不应出现未支持的目标");
    {
        let def_id = instance.def_id();
        if crate::trace::trace_enabled("FN") {
            eprintln!("[forge] emit plan#{pi} {}", tcx.def_path_str(def_id),);
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
            if reject_link_section(tcx, instance.def_id()) {
                return Err("`#[link_section]` 已拒绝（诊断见上）".to_string());
            }
            let body = tcx.instance_mir(instance.def);
            let monomorphized = instance.instantiate_mir_and_normalize_erasing_regions(
                tcx,
                ty::TypingEnv::fully_monomorphized(),
                ty::EarlyBinder::bind(tcx, body.clone()),
            );
            let body = tcx.arena.alloc(monomorphized);
            let mut compiled = crate::compile::lower_and_compile(
                tcx,
                instance,
                body,
                &mut table,
                &mut referenced,
                isa_name,
            )
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
                "[forge] size plan#{pi} {sym_name} code_bytes={} head=[{}]",
                code.code.len(),
                head.join(" ")
            );
        }
        fns.push(FnOutcome {
            sym_name,
            result,
            referenced,
        });
    }
    TaskOutcome {
        plan: pi,
        fns,
        table,
    }
}
