//! LLVM IR 词法器（logos）。
//!
//! 覆盖 LLVM IR 的 token 集合：关键字、类型（标量/向量）、`@`/`%` 标识符、
//! 整数/十六进制/浮点/字符串字面量、标点、`;` 行注释。
//!
//! 注意：指令名（add/sub/icmp/load…）不在此处枚举——它们作为 `Ident` 交给
//! 语法层的 `llvm_mapping` 查表（105 个 opcode 全量映射）。裸标识符 `x`
//! 只在向量/数组类型的分隔位置出现（参数/局部名总是 `%`/`@` 前缀），因此
//! `x` 关键字 token 不会与用户标识符冲突。

use logos::Logos;

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip(r"[ \t\r\n]+|;[^\n]*|(?s)/\*.*?\*/", allow_greedy = true))]
pub enum Token {
    // ── 模块级关键字 ──
    #[token("define")]
    Define,
    #[token("declare")]
    Declare,
    #[token("target")]
    Target,
    #[token("triple")]
    Triple,
    #[token("datalayout")]
    Datalayout,
    #[token("source_filename", priority = 2)]
    SourceFilenameKw,
    #[token("module asm", priority = 2)]
    ModuleAsmKw,

    // ── 终结符关键字 ──
    #[token("ret")]
    Ret,
    #[token("br")]
    Br,
    #[token("switch")]
    Switch,
    #[token("unreachable")]
    Unreachable,

    // ── 类型关键字 ──
    #[token("void")]
    VoidTy,
    #[token("label")]
    LabelTy,
    #[token("ptr")]
    PtrTy,
    #[token("opaque", priority = 2)]
    OpaqueKw,
    // 可伸缩向量类型（单 token）：`<vscale x 4 x i32>`（SVE/RVV）——语法层
    // 5-token 序列致 LALR 状态爆炸（第十一轮实测），lexer 合并最简
    #[regex(r"<vscale +x +[0-9]+ +x +([ib][1-9][0-9]*|f(16|32|64|128)|bfloat|half|float|double|fp128|x86_fp80|ppc_fp128|ptr)>", |lex| {
        let s = lex.slice();
        let body = s.trim_start_matches("<vscale x ").trim_end_matches('>');
        let (n, rest) = body.split_once(" x ").unwrap_or(("0", "i32"));
        let len = n.parse().unwrap_or(0);
        let bits = rest.trim();
        let elem = if bits == "ptr" {
            ElemTy::Ptr
        } else if bits.starts_with('i') || bits.starts_with('b') {
            ElemTy::Int(bits[1..].parse().unwrap_or(32))
        } else {
            ElemTy::Float(bits.parse().unwrap_or(64))
        };
        VecElem { len, elem }
    })]
    VscaleTy(VecElem),
    #[token("token", priority = 2)]
    TokenKw,
    #[token("x", priority = 2)]
    X,
    // 指令关键字（LLVM 保留字，priority 高于 Ident）
    #[token("icmp", priority = 2)]
    Icmp,
    #[token("fcmp", priority = 2)]
    Fcmp,
    // 二元指令（S2：专用 token 使 BinaryOp 规则 first 集与 Ident 分离，
    // 支持 LLVM 现代语法 `add i32 %a, %b` 单类型第二操作数）
    #[token("add", priority = 2)]
    Add,
    #[token("sub", priority = 2)]
    Sub,
    #[token("mul", priority = 2)]
    Mul,
    #[token("udiv", priority = 2)]
    Udiv,
    #[token("sdiv", priority = 2)]
    Sdiv,
    #[token("urem", priority = 2)]
    Urem,
    #[token("srem", priority = 2)]
    Srem,
    #[token("shl", priority = 2)]
    Shl,
    #[token("lshr", priority = 2)]
    Lshr,
    #[token("ashr", priority = 2)]
    Ashr,
    #[token("and", priority = 2)]
    And,
    #[token("or", priority = 2)]
    Or,
    #[token("xor", priority = 2)]
    Xor,
    #[token("fadd", priority = 2)]
    Fadd,
    #[token("fsub", priority = 2)]
    Fsub,
    #[token("fmul", priority = 2)]
    Fmul,
    #[token("fdiv", priority = 2)]
    Fdiv,
    #[token("frem", priority = 2)]
    Frem,
    #[token("call", priority = 2)]
    Call,
    #[token("tail", priority = 2)]
    Tail,
    #[token("musttail", priority = 2)]
    Musttail,
    #[token("notail", priority = 2)]
    Notail,
    #[token("alloca", priority = 2)]
    Alloca,
    #[token("load", priority = 2)]
    Load,
    #[token("store", priority = 2)]
    Store,
    #[token("getelementptr", priority = 2)]
    GetElementPtr,
    #[token("inbounds", priority = 2)]
    Inbounds,
    // 原子指令关键字
    #[token("atomicrmw", priority = 2)]
    AtomicRmwKw,
    #[token("cmpxchg", priority = 2)]
    CmpxchgKw,
    #[token("fence", priority = 2)]
    FenceKw,
    #[token("weak", priority = 2)]
    WeakKw,
    #[token("addrspace", priority = 2)]
    AddrspaceKw,
    #[token("unnamed_addr", priority = 2)]
    UnnamedAddrKw,
    #[token("local_unnamed_addr", priority = 2)]
    LocalUnnamedAddrKw,
    #[token("section", priority = 2)]
    SectionKw,
    #[token("thread_local", priority = 2)]
    ThreadLocalKw,
    // thread_local(模型)（LLVM：localdynamic/initialexec/localexec/generaldynamic）
    // ——lexer 合并单 token 避免 LALR 2-lookahead
    #[regex(r"thread_local\([a-z_]+\)", |lex| {
        let s = lex.slice();
        s.trim_start_matches("thread_local(").trim_end_matches(')').to_string()
    })]
    ThreadLocalModelKw(String),
    #[token("hidden", priority = 2)]
    HiddenKw,
    #[token("protected", priority = 2)]
    ProtectedKw,
    #[token("comdat", priority = 2)]
    ComdatKw,
    #[token("type", priority = 2)]
    TypeKw,
    #[token("extractvalue", priority = 2)]
    ExtractValueKw,
    #[token("extractelement", priority = 2)]
    ExtractElementKw,
    #[token("insertelement", priority = 2)]
    InsertElementKw,
    #[token("shufflevector", priority = 2)]
    ShuffleVectorKw,
    #[token("insertvalue", priority = 2)]
    InsertValueKw,
    // 全局 linkage
    #[token("private", priority = 2)]
    Private,
    #[token("internal", priority = 2)]
    Internal,
    #[token("external", priority = 2)]
    External,
    // 全局 linkage 补全（2.6）：weak/linkonce/appending/available_externally + dll storage
    #[token("weak_odr", priority = 2)]
    WeakOdrKw,
    #[token("linkonce", priority = 2)]
    LinkonceKw,
    #[token("linkonce_odr", priority = 2)]
    LinkonceOdrKw,
    #[token("appending", priority = 2)]
    AppendingKw,
    #[token("externally_initialized", priority = 2)]
    ExternallyInitializedKw,
    #[token("available_externally", priority = 2)]
    AvailableExternallyKw,
    #[token("common", priority = 2)]
    CommonKw,
    #[token("dllimport", priority = 2)]
    DllImportKw,
    #[token("dllexport", priority = 2)]
    DllExportKw,
    #[token("dso_local", priority = 2)]
    DsoLocal,
    // LLVM 字符串常量：c"abc"（带转义）
    #[regex(r#"c"([^"\\]|\\.)*""#, |lex| lex.slice().to_string())]
    CString(String),
    // metadata：`!N`（节点引用）/ `!name`（附加名）
    #[regex(r"![0-9]+", |lex| lex.slice()[1..].parse::<u32>().unwrap_or(0))]
    MetadataIdKw(u32),
    // 命名 metadata 定义（LLVM：`!t = !{...}`）——`!name =`（含等号）比 `!name`
    // 匹配更长，logos 最长匹配优先自动消歧；grammar 无需 epsilon 链。
    #[regex(r"![a-zA-Z_\\][a-zA-Z0-9_.\\-]*[ \t]*=", |lex| {
        lex.slice()[1..].trim_end_matches('=').trim_end().to_string()
    })]
    MetadataDefKw(String),
    #[regex(r"![a-zA-Z_\\][a-zA-Z0-9_.\\-]*", |lex| lex.slice()[1..].to_string())]
    MetadataNameKw(String),
    // `metadata` 关键字：pre-opaque 旧语法（`!0 = metadata !{}`）的前缀——
    // 专用 token 使其不匹配 Ident，旧语法 parse 直接拒绝（仅最新版 LLVM）
    #[token("metadata", priority = 2)]
    MetadataKw,
    // 内存属性关键字
    #[token("volatile", priority = 2)]
    Volatile,
    #[token("align", priority = 2)]
    Align,
    // 算术标志（LLVM：`add nsw i32 %a, i32 %b`）
    #[token("nsw", priority = 2)]
    Nsw,
    #[token("nuw", priority = 2)]
    Nuw,
    #[token("exact", priority = 2)]
    Exact,
    // 新标志（M3 标志类）：`zext nneg` / `or disjoint` / `icmp samesign` /
    // GEP `nuw nusw inrange(...)`
    #[token("nneg", priority = 2)]
    Nneg,
    #[token("disjoint", priority = 2)]
    Disjoint,
    #[token("samesign", priority = 2)]
    Samesign,
    #[token("nusw", priority = 2)]
    Nusw,
    #[token("inrange", priority = 2)]
    InrangeKw,
    // fast-math 标志
    #[token("fast", priority = 2)]
    Fast,
    #[token("nnan", priority = 2)]
    Nnan,
    #[token("ninf", priority = 2)]
    Ninf,
    #[token("nsz", priority = 2)]
    Nsz,
    #[token("arcp", priority = 2)]
    Arcp,
    #[token("contract", priority = 2)]
    Contract,
    #[token("afn", priority = 2)]
    Afn,
    #[token("reassoc", priority = 2)]
    Reassoc,
    // 参数属性（LLVM：`i32 signext %a`）
    #[token("signext", priority = 2)]
    Signext,
    #[token("immarg", priority = 2)]
    ImmargKw,
    #[token("zeroext", priority = 2)]
    Zeroext,
    #[token("noalias", priority = 2)]
    Noalias,
    #[token("alias", priority = 2)]
    AliasKw,
    #[token("noundef", priority = 2)]
    Noundef,
    #[token("readonly", priority = 2)]
    Readonly,
    #[token("writeonly", priority = 2)]
    Writeonly,
    #[token("extern_weak", priority = 2)]
    ExternWeakKw,
    #[token("nocapture", priority = 2)]
    Nocapture,
    #[token("nonnull", priority = 2)]
    Nonnull,
    // 参数/返回属性补全（2.7）：inreg / byval(Type) / sret(Type)
    #[token("inreg", priority = 2)]
    InregKw,
    #[token("byval", priority = 2)]
    ByvalKw,
    #[token("sret", priority = 2)]
    SretKw,
    // 调用约定/函数属性
    #[token("fastcc", priority = 2)]
    Fastcc,
    #[token("win64cc", priority = 2)]
    Win64cc,
    #[token("cc", priority = 2)]
    Cc,
    #[token("attributes", priority = 2)]
    Attributes,
    #[token("nounwind", priority = 2)]
    Nounwind,
    #[token("noinline", priority = 2)]
    Noinline,
    #[token("alwaysinline", priority = 2)]
    Alwaysinline,
    #[token("norecurse", priority = 2)]
    Norecurse,
    #[token("optnone", priority = 2)]
    Optnone,
    // 转换指令关键字
    #[token("sext", priority = 2)]
    Sext,
    #[token("zext", priority = 2)]
    Zext,
    #[token("trunc", priority = 2)]
    Trunc,
    #[token("fptrunc", priority = 2)]
    Fptrunc,
    #[token("fpext", priority = 2)]
    Fpext,
    #[token("fptosi", priority = 2)]
    Fptosi,
    #[token("sitofp", priority = 2)]
    Sitofp,
    #[token("fptoui", priority = 2)]
    Fptoui,
    #[token("uitofp", priority = 2)]
    Uitofp,
    #[token("ptrtoint", priority = 2)]
    Ptrtoint,
    #[token("inttoptr", priority = 2)]
    Inttoptr,
    #[token("bitcast", priority = 2)]
    Bitcast,
    #[token("addrspacecast", priority = 2)]
    AddrspaceCastKw,
    #[token("landingpad", priority = 2)]
    LandingPadKw,
    #[token("invoke", priority = 2)]
    InvokeKw,
    #[token("inalloca", priority = 2)]
    InallocaKw,
    #[token("byref", priority = 2)]
    ByrefKw,
    #[token("uwtable", priority = 2)]
    UwtableKw,
    #[token("nosync", priority = 2)]
    NosyncKw,
    #[token("builtin", priority = 2)]
    BuiltinKw,
    #[token("nobuiltin", priority = 2)]
    NobuiltinKw,
    #[token("captures", priority = 2)]
    CapturesKw,
    #[token("resume", priority = 2)]
    ResumeKw,
    #[token("va_arg", priority = 2)]
    VaArgKw,
    #[token("phi", priority = 2)]
    Phi,
    #[token("to", priority = 2)]
    To,
    // 值关键字
    #[token("true", priority = 2)]
    True,
    #[token("false", priority = 2)]
    False,
    #[token("null", priority = 2)]
    Null,
    #[token("undef", priority = 2)]
    Undef,
    #[token("poison", priority = 2)]
    Poison,
    #[token("none", priority = 2)]
    NoneKw,
    #[token("syncscope", priority = 2)]
    SyncscopeKw,
    #[token("elementwise", priority = 2)]
    ElementwiseKw,
    #[token("ifunc", priority = 2)]
    IfuncKw,
    #[token("uselistorder", priority = 2)]
    UselistorderKw,
    #[token("uselistorder_bb", priority = 2)]
    UselistorderBbKw,
    #[token("dereferenceable", priority = 2)]
    DereferenceableKw,
    #[token("dereferenceable_or_null", priority = 2)]
    DereferenceableOrNullKw,
    #[token("ssp", priority = 2)]
    SspKw,
    #[token("dso_local_equivalent", priority = 2)]
    DsoLocalEquivalentKw,
    #[token("ptrtoaddr", priority = 2)]
    PtrtoaddrKw,
    #[token("elementtype", priority = 2)]
    ElementtypeKw,
    // elementtype + 内嵌函数类型合并单 token（`ptr elementtype(void ()) @func`
    // ——GC statepoint;Type 后 LParen/RParen 的 2-lookahead 致 grammar 双分支
    // 885 冲突,lexer 单层括号 regex 消歧(参数列表内无函数类型,一层足够;
    // 标量形态 elementtype(void) 不含内层括号,仍走 ElementtypeKw 旧路径)
    #[regex(r"elementtype\([^()]*\([^()]*\)\)", |lex| lex.slice().to_string())]
    ElementtypeFnKw(String),
    // invoke GC live 列表合并单 token（statepoint：`["gc-live"(ptr %b, ...)]`
    // ——中括号平衡不可正则,宽松吞到本行首个 `]`(gc-live 参数为地址值,
    // 无数组字面量形态);grammar 必选单 token 分支经 lookahead 消歧）
    #[regex(r#"\["gc-live"[^\]]*\]"#, |lex| lex.slice().to_string())]
    GcLiveKw(String),
    #[token("callbr", priority = 2)]
    CallbrKw,
    #[token("convergent", priority = 2)]
    ConvergentKw,
    #[token("strictfp", priority = 2)]
    StrictfpKw,
    #[token("returned", priority = 2)]
    ReturnedKw,
    #[token("fneg", priority = 2)]
    FnegKw,
    #[token("asm", priority = 2)]
    AsmKw,
    #[token("blockaddress", priority = 2)]
    BlockaddressKw,
    #[token("gc", priority = 2)]
    GcKw,
    #[token("ptrauth", priority = 2)]
    PtrauthKw,
    // define + linkage 合并单 token（第十五轮：grammar 层方案四轮均 LALR 冲突——
    // lexer 最长匹配消歧，payload 为 linkage 名）
    #[regex(r"define[ \t]+(internal|private|external|weak|linkonce|linkonce_odr|appending|available_externally|common)", |lex| {
        lex.slice().trim_start_matches("define").trim().to_string()
    })]
    DefineLinkageKw(String),
    // declare + 前缀 metadata 合并单 token（`declare !bar !1 void @bar()`——
    // LLVM 已弃用的 callee-type 前缀语法；grammar 层 Declare 前缀与
    // DeclareTail 的 MetadataAttach+ 闭包状态合并冲突（第十一轮 543 类），
    // lexer 最长匹配消歧，前缀宽松丢弃（payload 保留原文）
    #[regex(r"declare[ \t]+![a-zA-Z_\\][a-zA-Z0-9_.\\-]*[ \t]+![0-9]+", |lex| lex.slice().to_string())]
    DeclarePrefixMdKw(String),
    // 开放函数属性集合（readnone/speculatable 等——lexer 合并防 Ident 冲突；
    // 第十二轮；writeonly 不在此列——它是参数属性,有专用 Writeonly token,
    // 留在此会抢占 ParamAttr 位置）
    #[regex(r"(readnone|speculatable|willreturn|nofree|norecurse|nosync|mustprogress|noreturn|safestack|argmemonly|inaccessiblememonly|nocallback)", |lex| lex.slice().to_string())]
    FuncOpenAttrKw(String),
    // 带参开放属性（第十四轮）：range(...)/nofpclass(...)/allockind(...)/memory(...)
    // ——lexer 合并整段（内含逗号/引号，regex 防 LALR）
    #[regex(r"(range|nofpclass|allockind|memory)\([^)]*\)", |lex| lex.slice().to_string())]
    ParenAttrKw(String),
    // DI flag 组合（`DIFlagTypePassByValue | DIFlagTypePassByReference`）
    #[token("|")]
    Pipe,
    #[token("prefix", priority = 2)]
    PrefixKw,
    #[token("prologue", priority = 2)]
    PrologueKw,
    // riscv_vls_cc(N)（RISC-V 可伸缩向量调用约定——lexer 合并防 Ident 冲突）
    #[regex(r"riscv_vls_cc\([0-9]+\)", |lex| {
        let s = lex.slice();
        s.trim_start_matches("riscv_vls_cc(").trim_end_matches(')').parse().unwrap_or(0)
    })]
    RiscvVlsCcKw(u32),
    #[token("splat", priority = 2)]
    SplatKw,
    // denormal_fpenv(模式)（LLVM 函数属性——lexer 合并防 Ident 冲突;
    // 第二十九轮:保留完整文本含模式,原丢弃 (ieee) 致 display 还原缺参）
    #[regex(r"denormal_fpenv\([a-z_:|\-,\s]+\)", |lex| lex.slice().to_string())]
    DenormalFpenvKw(String),
    #[token("presplitcoroutine", priority = 2)]
    PresplitKw,
    // vconst 向量常量（forge 扩展）与 lane 端序标记
    #[token("vconst", priority = 2)]
    VconstOp,
    #[token("big", priority = 2)]
    Big,

    // ── 标量类型 ──
    // 溢出（> u32::MAX 的 iN）返回 0——语义层拒绝（LLVM 上限 2^23；
    // u32 足以覆盖 i838608，避免 u16 截断成位宽 0 的误报）
    #[regex(r"[ib][1-9][0-9]*", |lex| lex.slice()[1..].parse::<u32>().unwrap_or(0))]
    IntTy(u32),
    #[regex(r"f(16|32|64|128)|bfloat|half|float|double|fp128|x86_fp80|ppc_fp128", |lex| match lex.slice() {
        "half" | "bfloat" => 16,
        "float" => 32,
        "double" => 64,
        "fp128" | "x86_fp80" | "ppc_fp128" => 128,
        s => s[1..].parse::<u16>().unwrap_or(64),
    })]
    FloatTy(u16),

    // ── 向量类型（整体，元素为标量 iN/fN/ptr）──
    #[regex(r"<[0-9]+ +x +([ib][1-9][0-9]*|f(16|32|64|128)|bfloat|half|float|double|fp128|x86_fp80|ppc_fp128|ptr( addrspace\([0-9]+\))?)>", |lex| {
        let inner = lex.slice().trim_start_matches('<').trim_end_matches('>');
        let (n, elem) = inner.split_once('x').map(|(n, e)| (n.trim(), e.trim())).unwrap_or(("0", "i32"));
        VecElem {
            len: n.parse().unwrap_or(0),
            elem: if let Some(bits) = elem.strip_prefix('i').or_else(|| elem.strip_prefix('b')) {
                // iN 与 b<N>（bit-precise）同 IR 表示 Int(bits)
                ElemTy::Int(bits.parse().unwrap_or(32))
            } else if let Some(bits) = elem.strip_prefix('f') {
                ElemTy::Float(bits.parse().unwrap_or(64))
            } else {
                ElemTy::Ptr
            },
        }
    })]
    VecTy(VecElem),

    // ── 向量常量字面量（单 token）：`<4 x float> <1.5, 2.5, -3.5, 4.25>` ──
    // VecTy + 紧跟的 lane 列表整体；比 VecTy 长，logos 最长匹配优先。
    // lane 值形态：数字 / `name 数字` / `name undef|poison|null|true|false`（select 掩码等）
    #[regex(r"<[0-9]+ +x +([ib][1-9][0-9]*|f(16|32|64|128)|bfloat|half|float|double|fp128|x86_fp80|ppc_fp128|ptr)> <([-0-9.,eE\s]+|([a-zA-Z][a-zA-Z0-9]* (undef|poison|null|true|false|zeroinitializer|[0-9.-]+))(, [a-zA-Z][a-zA-Z0-9]* (undef|poison|null|true|false|zeroinitializer|[0-9.-]+))*)>", |lex| lex.slice().to_string())]
    VecConstLit(String),
    // 裸 `zeroinitializer` 关键字（类型在别处：`[4 x i32] zeroinitializer`）
    #[token("zeroinitializer", priority = 2)]
    ZeroInitKw,
    // ── zeroinitializer 单 token（标量）：`i32 zeroinitializer` ──
    #[regex(r"(i[1-9][0-9]*|f(16|32|64|128)|bfloat|half|float|double|fp128|ptr|void) zeroinitializer", |lex| lex.slice().to_string())]
    ZeroInitLit(String),
    // ── zeroinitializer 单 token（向量）：`<4 x float> zeroinitializer` ──
    #[regex(r"<[0-9]+ +x +([ib][1-9][0-9]*|f(16|32|64|128)|bfloat|half|float|double|fp128|ptr)> zeroinitializer", |lex| lex.slice().to_string())]
    VecZeroInitLit(String),

    // shufflevector 掩码（单 token）：`<i32 0, i32 1, i32 2, i32 3>`（带类型 lane）
    #[regex(r"<i[1-9][0-9]*\s+(-?[0-9]+)(\s*,\s*i[1-9][0-9]*\s+-?[0-9]+)*>", |lex| lex.slice().to_string())]
    MaskVecLit(String),
    // ── 标识符 ──
    #[regex(r"@[a-zA-Z0-9_$.]+", |lex| lex.slice().to_string())]
    // 引号全局名（`@"_ZTV1N3FooE"`——第十五轮 index-value-order）
    #[regex(r#"@"[^"]*""#, |lex| lex.slice().to_string())]
    GlobalId(String),
    // 旧式 typed pointer（`%fum*`——LLVM 废弃语法；第十五轮 lexer 合并防
    // LocalId 状态爆炸，payload 保留原名）
    #[regex(r"%[a-zA-Z_][a-zA-Z0-9_.-]*\*", |lex| lex.slice().trim_end_matches('*').to_string())]
    // 匿名聚合指针（`{{i32},{float, double}}*`——insertextractvalue.ll；
    // 单层嵌套花括号 regex 可表达；最长匹配优先，无星号形态仍走
    // grammar 的 LBrace StructList 路径）
    #[regex(r"\{[^{}]*(\{[^{}]*\}[^{}]*)*\}\*", |lex| lex.slice().trim_end_matches('*').to_string())]
    TypedPtrKw(String),
    // comdat 名：`comdat($myc)`（LLVM 用 `$` 前缀）
    #[regex(r"\$[a-zA-Z0-9_$.]+", |lex| lex.slice().to_string())]
    ComdatName(String),
    // 字符串标签：普通（`%a`/`%2`/`%-3`——`-` 为数字型字符串标签简写）与
    // 带引号（`%"2"`——LLVM 数字型字符串标签）
    #[regex(r#"%("[^"\r\n\x00-\x1f]*"|[a-zA-Z0-9_$.+-]+)"#, |lex| lex.slice().to_string())]
    LocalId(String),
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_$.]*", |lex| lex.slice().to_string(), priority = 1)]
    Ident(String),
    // 数字型字符串标签（LLVM：`-N-:`——`-` 开头的无引号字符串标签）
    #[regex(r"-[a-zA-Z_][a-zA-Z0-9_$-]*", |lex| lex.slice().to_string())]
    LabelStr(String),

    // ── 字面量 ──
    #[regex(r"-?0x[0-9a-fA-F]+", |lex| {
        let (neg, digits) = lex.slice().strip_prefix('-').map(|r| (true, r)).unwrap_or((false, lex.slice()));
        let v = u64::from_str_radix(digits.trim_start_matches("0x").replace('_', "").as_str(), 16).unwrap_or(0);
        if neg {
            crate::big::Big::Signed(-dashu::Integer::from(v))
        } else {
            crate::big::Big::Unsigned(dashu::Natural::from(v))
        }
    })]
    HexLit(crate::big::Big),
    // 浮点十六进制字面量：`0xHBC00`（half）/`0xR3149`（bfloat）/`0xL...`
    // （fp128）/`0xM...`（x86_fp80）/`0xK...`/`0xJ...`（ppc_fp128）——
    // 前缀字母后为位模式 hex（第二十九轮:原宽松 0 丢位模式,
    // bfloat/half 常量 roundtrip 类型变 i16）
    #[regex(r"(0x[HJKLMR]|f0x)[0-9a-fA-F_]+", |lex| {
        let s = lex.slice();
        let hex = s
            .trim_start_matches("0x")
            .trim_start_matches(['H', 'J', 'K', 'L', 'M', 'R'])
            .replace('_', "");
        i64::from_str_radix(&hex, 16).unwrap_or(0)
    })]
    FloatHexLit(i64),
    #[regex(r"[+-]?[0-9]+((\.[0-9]*)?[eE][+-]?[0-9]+|\.[0-9]+)", |lex| lex.slice().parse::<f64>().unwrap_or(0.0))]
    // inf/nan 字面量（LLVM：`+inf`/`-inf`/`nan`——float-literals 类）
    #[regex(r"[+-]?(inf|[qs]?nan(\(0x[0-9a-fA-F]+\))?)", |lex| match lex.slice() {
        s if s.ends_with("inf") && s.starts_with('-') => f64::NEG_INFINITY,
        s if s.ends_with("inf") => f64::INFINITY,
        _ => f64::NAN,
    })]
    FloatLit(f64),
    // C99 十六进制浮点（`0x1.0p-32`——LLVM 测试 float-literals；宽松 0）
    #[regex(r"[+-]?0x[0-9A-Fa-f.]*p[+-]?[0-9]+", |_| 0.0)]
    HexFloatLit(f64),
    // 任意精度整数（第二十三轮 Big 化：dashu Integer——任意长度十进制,
    // 溢出不再折叠哨兵,值域校验/display 保留精确值;
    // invalid-diexpression-large 的 u64::MAX vs 超限值自然区分）
    #[regex(r"-?[0-9]+", |lex| {
        let s = lex.slice();
        crate::big::Big::Signed(s.parse::<dashu::Integer>().unwrap_or_default())
    })]
    IntLit(crate::big::Big),
    // metadata 裸 tuple 字面量（`operands: {!0, !3, !4}`——含 key 前缀整段
    // 合并,与非 metadata 语法及 `!named = !{...}` 定义不相交;
    // 单层嵌套 `!{}` 覆盖——generic-debug-node.ll;第二十一轮）
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*[ \t]*:[ \t]*\{[ \t]*![0-9]*(?:\{[^}]*\})?[^}]*\}", |lex| lex.slice().to_string())]
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*[ \t]*:[ \t]*\{\}", |lex| lex.slice().to_string())]
    MetadataFieldTupleLit(String),
    // `global ptr [addrspace(N)] @ref` 裸引用 init 合并单 token
    // （`@B = global ptr @A`——L6 裸全局引用;GlobalId 后 Eq vs 换行的
    // 2-lookahead 歧义经 lexer 消解;形态限定:类型必须 ptr,其余 init
    // 表达式形态（trunc/inttoptr/addrspacecast 等）不匹配,无误吞）
    #[regex(r"global[ \t]+ptr( addrspace\([0-9]+\))?[ \t]+@[a-zA-Z0-9_$.]+", |lex| lex.slice().to_string())]
    GlobalPtrInitKw(String),
    // #dbg_* debug record 前缀（参数交 grammar 做数量/形态校验——
    // dbg-record-invalid-2/6/7/8;未知类型如 #dbg_invalid 不匹配 →
    // lexer 拒绝;终结符后无 grammar 位置 → parse 拒绝;第二十一轮）
    #[token("#dbg_value", priority = 2)]
    DbgValueKw,
    #[token("#dbg_declare", priority = 2)]
    DbgDeclareKw,
    #[token("#dbg_declare_value", priority = 2)]
    DbgDeclareValueKw,
    #[token("#dbg_assign", priority = 2)]
    DbgAssignKw,
    #[token("#dbg_label", priority = 2)]
    DbgLabelKw,
    #[token("#dbg_kill", priority = 2)]
    DbgKillKw,
    // summary 条目（`^N = gv: (...)` 等 ThinLTO summary——行级合并保留原文;
    // 语义层做 name 存在性与函数 value info 校验;第二十三轮）
    #[regex(r"\^[0-9]+ = [^\r\n]*", |lex| lex.slice().to_string(), allow_greedy = true)]
    SummaryKw(String),
    #[regex(r#""([^"\\\r\n\x00-\x1f]|\\.)*""#, |lex| {
        // 去掉首尾边界引号并**解码转义**（`\"` → `"`、`\\` → `\`、`\n`/`\t`/`\r`）——
        // 与 display 的 fmt_quoted 编码对称，保证多次 round-trip 幂等。
        let s = lex.slice();
        let inner = &s[1..s.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some(other) => {
                    // 未知转义保留原样（不破坏）
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        }
        out
    })]
    StrLit(String),
    // attribute group：`#0`（LLVM 属性组引用）
    #[regex(r"#[0-9]+", |lex| lex.slice()[1..].parse::<u32>().unwrap_or(0))]
    AttrGroupId(u32),

    // ── 标点 ──
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("<")]
    LAngle,
    #[token(">")]
    RAngle,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token("=")]
    Eq,
    #[token("*")]
    Star,
    #[token("!")]
    Bang,
    // 可变参数标记（LLVM：`declare i32 @printf(ptr, ...)`）
    #[token("...")]
    Ellipsis,
}

/// 向量类型元素（枚举，避免字符串）。
#[derive(Clone, Debug, PartialEq)]
pub enum ElemTy {
    Int(u32),
    Float(u16),
    Ptr,
}

/// 向量类型（长度 + 元素）。
#[derive(Clone, Debug, PartialEq)]
pub struct VecElem {
    pub len: u32,
    pub elem: ElemTy,
}

/// 便捷词法器入口（Logos::lexer 包装，供 lalrpop 解析器消费）。
pub fn lexer(src: &str) -> logos::Lexer<'_, Token> {
    Token::lexer(src)
}

/// 词法错误（lalrpop 需要 Location/Error 类型）。
#[derive(Debug, Clone, PartialEq)]
pub struct LexError;

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unexpected character")
    }
}

impl std::error::Error for LexError {}

impl<'a> Iterator for TokenStream<'a> {
    type Item = Result<(usize, Token, usize), LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.lexer.next() {
            Some(Ok(tok)) => {
                let span = self.lexer.span();
                Some(Ok((span.start, tok, span.end)))
            }
            Some(Err(_)) => Some(Err(LexError)),
            None => None,
        }
    }
}

/// 带位置的 token 流（lalrpop 消费：`Result<(usize, Token, usize), LexError>`）。
pub struct TokenStream<'a> {
    lexer: logos::Lexer<'a, Token>,
}

impl<'a> TokenStream<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            lexer: Token::lexer(src),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_all(src: &str) -> Vec<Token> {
        Token::lexer(src).map(|r| r.expect("lex ok")).collect()
    }

    #[test]
    fn lex_keywords() {
        let toks = lex_all("define declare ret br switch unreachable");
        assert_eq!(
            toks,
            vec![
                Token::Define,
                Token::Declare,
                Token::Ret,
                Token::Br,
                Token::Switch,
                Token::Unreachable,
            ]
        );
    }

    #[test]
    fn lex_types() {
        let toks = lex_all("i32 i1 f64 ptr void label <4 x i32> <2 x ptr> <4 x b64>");
        assert_eq!(
            toks,
            vec![
                Token::IntTy(32),
                Token::IntTy(1),
                Token::FloatTy(64),
                Token::PtrTy,
                Token::VoidTy,
                Token::LabelTy,
                Token::VecTy(VecElem {
                    len: 4,
                    elem: ElemTy::Int(32)
                }),
                Token::VecTy(VecElem {
                    len: 2,
                    elem: ElemTy::Ptr
                }),
                Token::VecTy(VecElem {
                    len: 4,
                    elem: ElemTy::Int(64)
                }),
            ]
        );
    }

    #[test]
    fn lex_ids() {
        let toks = lex_all("@add %s %0 @main");
        assert_eq!(
            toks,
            vec![
                Token::GlobalId("@add".to_string()),
                Token::LocalId("%s".to_string()),
                Token::LocalId("%0".to_string()),
                Token::GlobalId("@main".to_string()),
            ]
        );
    }

    #[test]
    fn lex_literals() {
        let toks = lex_all("42 -1 0x2A 1.5e0 -0.25 \"hello\"");
        assert_eq!(
            toks,
            vec![
                Token::IntLit(crate::big::Big::from(42)),
                Token::IntLit(crate::big::Big::from(-1)),
                Token::HexLit(crate::big::Big::from(42u64)),
                Token::FloatLit(1.5),
                Token::FloatLit(-0.25),
                Token::StrLit("hello".to_string()),
            ]
        );
    }

    #[test]
    fn lex_punctuation_and_comment() {
        // `;` 注释被跳过；分号本身不是 token
        let toks = lex_all("( ) { } [ ] , : = * ! ; this is a comment");
        assert_eq!(
            toks,
            vec![
                Token::LParen,
                Token::RParen,
                Token::LBrace,
                Token::RBrace,
                Token::LBracket,
                Token::RBracket,
                Token::Comma,
                Token::Colon,
                Token::Eq,
                Token::Star,
                Token::Bang,
            ]
        );
    }

    #[test]
    fn lex_instruction_names_as_ident() {
        // 指令名：二元指令是专用 token（S2——Add/Sub），icmp/load/store 亦专用；
        // 其余指令名走 Ident（交给 llvm_mapping 查表）
        let toks = lex_all("add sub icmp load store");
        assert_eq!(
            toks,
            vec![
                Token::Add,
                Token::Sub,
                Token::Icmp,
                Token::Load,
                Token::Store,
            ]
        );
    }

    #[test]
    fn lex_bad_char_errors() {
        let mut lex = Token::lexer("?");
        assert!(lex.next().unwrap().is_err());
    }
}
