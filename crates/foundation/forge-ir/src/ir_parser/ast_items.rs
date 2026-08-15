//! 解析中间表示（AST）—— grammar.lalrpop 的 action 产物，semantics 消费。

use super::lexer::ElemTy;

// ── 模块级 ──

/// 模块级项：target 设置或函数。
use std::collections::HashMap;

pub enum ParsedItem {
    Target(String, String),
    Function(ParsedFunction),
    Global(ParsedGlobal),
    /// `@a = alias <ty>, <aliasee>` 模块级别名（aliasee 为常量表达式）。
    Alias(ParsedAlias),
    /// `attributes #N = { nounwind ... }`（属性组定义；id + 属性名）。
    AttrGroup(u32, Vec<String>),
    /// `!N = !{...}` metadata 节点定义（id + 节点）。
    Metadata(u32, MetadataNodeDef),
    /// `%struct.X = type {...}` LLVM 类型定义（类型名 + 定义体）。
    TypeDef(String, ParsedType),
    /// `!t = !{...}` 命名 metadata 定义（名字 + 节点）。
    NamedMetadata(String, MetadataNodeDef),
    /// `source_filename = "..."`（模块级元数据）。
    SourceFilename(String),
    /// `^N = gv: (...)` ThinLTO summary 条目（原文——语义层校验
    /// name 存在性与函数 value info;第二十三轮）。
    Summary(String),
    /// `$c = comdat any`（comdat 声明；名字去重）。
    ComdatDecl(String),
    /// `uselistorder <ty> <val>, { 1, 0 }`（LLVM use-list 顺序指令——模块级
    /// 校验索引排列与值存在性）。
    UselistOrder(String, Vec<i64>, ParsedType),
    /// `module asm "..."`（模块级内联汇编）。
    ModuleAsm(String),
}

/// metadata 节点引用（`!N` 数字 / `!name` 命名）。
#[derive(Clone, Debug)]
pub enum MetadataRef {
    /// `!N`（数字节点 id）。
    Num(u32),
    /// `!name`（命名 metadata 引用；semantics 阶段经 lookup_named 解析为 id）。
    Named(String),
}

/// metadata 节点定义：`!{...}`（tuple）/ `!named(...)`（named）。
#[derive(Clone, Debug)]
pub enum MetadataNodeDef {
    Tuple(Vec<MetadataVal>),
    Named(String, Vec<MetadataVal>),
    /// `distinct` 前缀（第二十一轮保留——DICompileUnit/Definition DISubprogram
    /// 必 distinct 校验用；去重语义不变）
    Distinct(Box<MetadataNodeDef>),
}

/// metadata 节点值。
#[derive(Clone, Debug)]
pub enum MetadataVal {
    Int(i64),
    /// 超 i64 范围的大整数（第二十三轮 Big 化——lexer 保留精确值,
    /// DI 值域校验在 check_di_node 拒绝;display 输出原文）
    IntBig(crate::big::Big),
    UInt(u64),
    Float(f64),
    Str(String),
    Null,
    /// 引用其它节点：`!N`（数字）。
    Ref(u32),
    /// 命名节点引用：`!t`（semantics 经 lookup_named 解析）。
    NamedRef(String),
    /// 嵌套节点：`!{...}` / `!named(...)`。
    Nested(Box<MetadataNodeDef>),
    /// named 节点 key:value 字段（`!DILocation(line: 10)`）——第二十一轮
    /// 恢复 key（供 DI 批量校验：必填字段/值域/重复/合法性）；build 后
    /// key 丢弃（IR 层值等价，display roundtrip 不变）。
    Field(String, Box<MetadataVal>),
}

/// metadata 裸 tuple 字面量拆分（`{!0, !3, !4}`——lexer 合并 token,
/// `key: {!0, !3, !4}` 整段拆分（lexer 合并 token）→ Field(key, tuple)。
pub fn split_md_field_tuple_lit(s: &str) -> MetadataVal {
    let Some((key, tuple)) = s.split_once(':') else {
        return MetadataVal::Null;
    };
    let key = key.trim().to_string();
    let inner = tuple.trim().trim_start_matches('{').trim_end_matches('}');
    let vals: Vec<MetadataVal> = inner
        .split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| {
            if p == "!{}" {
                MetadataVal::Nested(Box::new(MetadataNodeDef::Tuple(vec![])))
            } else if let Some(n) = p.strip_prefix('!') {
                if let Ok(id) = n.parse::<u32>() {
                    MetadataVal::Ref(id)
                } else {
                    MetadataVal::NamedRef(n.to_string())
                }
            } else {
                MetadataVal::Str(p.to_string())
            }
        })
        .collect();
    MetadataVal::Field(
        key,
        Box::new(MetadataVal::Nested(Box::new(MetadataNodeDef::Tuple(vals)))),
    )
}

/// Big → 数值转换 helper（第二十三轮 IntLit Big 化——grammar reducer 用;
/// 超界 → 上限哨兵,各值域校验自然拒绝）
pub fn big_as_i64(n: &crate::big::Big) -> i64 {
    i64::try_from(n.clone()).unwrap_or(i64::MAX)
}
pub fn big_as_u32(n: &crate::big::Big) -> u32 {
    u32::try_from(n.clone()).unwrap_or(u32::MAX)
}
pub fn big_as_u64(n: &crate::big::Big) -> u64 {
    u64::try_from(n.clone()).unwrap_or(u64::MAX)
}

/// uselistorder 索引收集（第二十三轮消重——grammar 6 分支共用;
/// rest 元素为 (Comma, IntLit) 的 lalrpop 展开）
pub fn collect_uselistorder_idx<T>(
    first: &crate::big::Big,
    rest: Vec<(T, crate::big::Big)>,
) -> Vec<i64> {
    let mut idx = vec![big_as_i64(first)];
    idx.extend(rest.into_iter().map(|(_, n)| big_as_i64(&n)));
    idx
}

/// `global ptr [addrspace(N)] @ref` 裸引用 init 拆分（lexer 合并 token;
/// L6 裸全局引用——第二十一轮）。返回 (指针类型, 目标全局名)。
pub fn split_global_ptr_init(s: &str) -> (ParsedType, String) {
    let rest = s.trim_start_matches("global").trim();
    if let Some(r) = rest.strip_prefix("ptr addrspace(") {
        let (n, r2) = r.split_once(')').unwrap_or(("0", ""));
        let name = r2.trim().trim_start_matches('@').to_string();
        (ParsedType::PtrAddrSpace(n.parse().unwrap_or(0)), name)
    } else {
        let name = rest
            .trim_start_matches("ptr")
            .trim()
            .trim_start_matches('@')
            .to_string();
        (ParsedType::Ptr, name)
    }
}

/// #dbg_* debug record 占位指令（语义层宽松丢弃;第二十一轮）。
pub fn dbg_record_inst() -> ParsedInst {
    ParsedInst {
        endian: false,
        volatile: false,
        align: 0,
        result: None,
        opcode: "dbg-record".to_string(),
        cond: None,
        phi_incomings: vec![],
        arg_attrs: vec![],
        call_fn_attrs: vec![],
        metadata_attach: vec![],
        flags: vec![],
        args: vec![],
    }
}

/// 模块级全局变量声明：`@g = global i32 42` / `@c = constant f32 1.5`。
pub struct ParsedGlobal {
    /// 名字（带 `@` 前缀）。
    pub name: String,
    /// true = `constant`（不可变），false = `global`（可变）。
    pub is_constant: bool,
    pub ty: ParsedType,
    /// 初始化值（i32/f32 字面量、`null`；无 init 时为 None）。
    pub init: Option<GlobalInitVal>,
    /// 对齐（`align N` 属性；缺省 0；u64 防截断——LLVM 上限 2^30 由语义层校验）。
    pub align: u64,
    /// linkage（`private`/`internal`/`external`；None = 默认 external）。
    pub linkage: Option<String>,
    /// `dso_local` 前缀（非抢占保证）。
    pub dso_local: bool,
    /// `unnamed_addr` / `local_unnamed_addr`（地址无关；布尔合并）。
    pub unnamed_addr: bool,
    /// `section "name"`（自定义段）。
    pub section: Option<String>,
    /// `addrspace(N)` 前缀（全局地址空间；0 = 默认）。
    pub addr_space: u32,
    /// `thread_local`（TLS；false = 普通全局）。
    pub thread_local: bool,
    /// visibility（`hidden`/`protected`；None = default）。
    pub visibility: Option<String>,
    /// `comdat($name)`（None = 无）。
    pub comdat: Option<String>,
    /// dll storage（`dllimport`/`dllexport`；None = 默认）。
    pub dll_storage: Option<String>,
    /// 全局尾 metadata 附加（`@g = global i32 0, !absolute_symbol !0`）。
    pub metadata: Vec<(String, MetadataRef)>,
    /// ifunc（第二十九轮:IR 表示——`@f = ifunc <retty> (<params>), ptr @resolver`;
    /// 参数类型存解析层文本供 display 原样还原）。
    pub is_ifunc: bool,
    pub ifunc_params: Vec<String>,
    pub ifunc_resolver: Option<String>,
}

/// 模块级别名：`@a = [linkage] alias <ty>, <aliasee>`（LLVM `alias`）。
#[derive(Clone, Debug)]
pub struct ParsedAlias {
    /// 名字（带 `@` 前缀）。
    pub name: String,
    /// alias 的目标类型（`alias i32, ...` 的 `i32`）。
    pub ty: ParsedType,
    /// aliasee 常量表达式（`(ParsedType, ConstExpr)`：TypeOp 带类型前缀，
    /// 括号表达式（GEP/addrspacecast 等）类型为 Void 占位）。
    pub aliasee: (ParsedType, ConstExpr),
    /// linkage（`internal`/`private` 等；None = 默认 external）。
    pub linkage: Option<String>,
    /// visibility（`hidden`/`protected`；None = default）。
    pub visibility: Option<String>,
    pub dso_local: bool,
    pub unnamed_addr: bool,
    /// 尾 metadata 附加（`@a = alias ..., !kind !N`）。
    pub metadata: Vec<(String, MetadataRef)>,
}

/// global 初始化值的字面量形式。
#[derive(Clone, Debug)]
pub enum GlobalInitVal {
    Int(i64),
    UInt(u64),
    Float(f64),
    /// `null` 指针初值（按指针大小填零）。
    Null,
    /// LLVM 字符串常量 `c"..."`（已解码字节；类型应为 `[N x i8]`）。
    Str(Vec<u8>),
    /// `zeroinitializer` 关键字（按类型大小填零）。
    ZeroInit,
    /// 聚合常量元素（`[i32 1, i32 2]` / `{ i32 1, i64 2 }`；单层标量）。
    Agg(Vec<GlobalInitVal>),
    /// 常量表达式（`ptrtoint (ptr @h to i32)` 等；保留树供 display 还原）。
    Expr(Box<ConstExpr>),
}

/// 全局初始化常量表达式（LLVM 常量表达式，括号包裹：`ptrtoint (ptr @h to i32)`）。
///
/// 树形保留（类型用文本 `ParsedType`），display 原样输出、semantics 求字节。
/// 未知操作名（grammar 无法拒绝的非关键字）记入 `Unsupported`，semantics 报错。
#[derive(Clone, Debug)]
pub enum ConstExpr {
    /// 全局地址叶子 `@h`。
    GlobalAddr(String),
    Int(i64),
    UInt(u64),
    Float(f64),
    Null,
    Undef,
    Poison,
    /// `ptrtoint (<op_ty> <op> to <to_ty>)`
    PtrToInt {
        op_ty: ParsedType,
        op: Box<ConstExpr>,
        to_ty: ParsedType,
    },
    /// `inttoptr (<op_ty> <op> to <to_ty>)`
    IntToPtr {
        op_ty: ParsedType,
        op: Box<ConstExpr>,
        to_ty: ParsedType,
    },
    /// `bitcast (<op_ty> <op> to <to_ty>)`
    Bitcast {
        op_ty: ParsedType,
        op: Box<ConstExpr>,
        to_ty: ParsedType,
    },
    /// `addrspacecast (<op_ty> <op> to <to_ty>)`
    AddrSpaceCast {
        op_ty: ParsedType,
        op: Box<ConstExpr>,
        to_ty: ParsedType,
    },
    /// `getelementptr (<indexed_ty>, <op_ty> <ptr>, <idx_ty> <idx>, ...)`
    GetElementPtr {
        indexed_ty: ParsedType,
        op_ty: ParsedType,
        ptr: Box<ConstExpr>,
        indices: Vec<(ParsedType, ConstExpr)>,
    },
    /// 二元折叠表达式：`add (i64 <l>, i64 <r>)`（flags 原样还原——
    /// `add nuw nsw (...)`）。semantics 求值折叠为常量。
    Binary {
        op: String,
        flags: Vec<String>,
        lhs: Box<ConstExpr>,
        rhs: Box<ConstExpr>,
    },
    /// 向量常量：`<2 x i32> <i32 3, i32 4>`（常量表达式位置；
    /// semantics 求值宽松返回 0——标量折叠无法表达向量）。
    Vector(ParsedType, Vec<VecLane>),
    /// 转换常量折叠表达式：`trunc (i64 42 to i32)` / `zext (i32 -1 to i64)` /
    /// `sext (...) to ...`（src_ty 为源类型——zext/sext 求值需要位宽）。
    Cast {
        op: String,
        src_ty: ParsedType,
        src: Box<ConstExpr>,
        to: ParsedType,
    },
    /// 未知操作名（`frobnicate (ptr @h to i32)`）——semantics 阶段报语义错误。
    Unsupported(String),
    /// `ptrauth (<ty> <base>, <ty> <key>[, <ty> <int_disc>, <ty> <addr_disc>])`
    /// ——AArch64 指针认证常量;参数携带至语义层做值级校验
    /// (base 指针类/key i32 常量/int_disc i64 常量/addr_disc 指针类)。
    Ptrauth(Vec<(ParsedType, ConstExpr)>),
}

impl ConstExpr {
    /// 操作数文本（不含类型前缀）：`@h` / `42` / `1.5` / `null`。
    fn value_text(&self) -> String {
        match self {
            ConstExpr::GlobalAddr(g) => format!("@{g}"),
            ConstExpr::Int(n) => n.to_string(),
            ConstExpr::UInt(n) => format!("{n}"),
            ConstExpr::Float(f) => {
                // 与 display 的浮点输出风格一致（整数值带 .0 便于 lexer 识别）
                if f.fract() == 0.0 && f.is_finite() {
                    format!("{f:.1}")
                } else {
                    format!("{f}")
                }
            }
            ConstExpr::Null => "null".to_string(),
            ConstExpr::Undef => "undef".to_string(),
            ConstExpr::Poison => "poison".to_string(),
            // 向量常量：值文本只含 `<lanes>`（类型前缀由 operand_text 的
            // op_ty 提供——第二十九轮:原走 other 输出完整 `ty <lanes>`
            // 致类型重复 `<2 x i32> <2 x i32> <...>`,reparse 拒绝）
            ConstExpr::Vector(_ty, lanes) => {
                if lanes.is_empty() {
                    "zeroinitializer".to_string()
                } else {
                    let lanes_s: Vec<String> = lanes
                        .iter()
                        .map(|l| match l {
                            VecLane::Int(n) => format!("i32 {n}"),
                            VecLane::UInt(n) => format!("i32 {n}"),
                            VecLane::Float(f) => format!("float {f}"),
                        })
                        .collect();
                    format!("<{}>", lanes_s.join(", "))
                }
            }
            // 嵌套表达式操作数：自身带括号（`bitcast (ptr @h to ptr)`），无类型前缀
            other => other.to_llvm_string(),
        }
    }

    /// 操作数位置文本（带类型前缀的叶子；嵌套表达式裸输出）。
    fn operand_text(&self, op_ty: &ParsedType, op: &ConstExpr) -> String {
        match op {
            ConstExpr::PtrToInt { .. }
            | ConstExpr::IntToPtr { .. }
            | ConstExpr::Bitcast { .. }
            | ConstExpr::AddrSpaceCast { .. }
            | ConstExpr::GetElementPtr { .. }
            | ConstExpr::Cast { .. }
            | ConstExpr::Ptrauth(_) => op.to_llvm_string(),
            // 二元折叠表达式：带类型前缀（`i32 add (i32 5, i32 -5)`——
            // 第二十九轮:原无前缀致 `add (5, -5)` 丢类型,reparse 拒;
            // 操作数与结果同类型（LLVM 语义）;op_ty Void 占位（顶层
            // init 无前缀形态）时仍无前缀）
            ConstExpr::Binary { op, flags, lhs, rhs } if !matches!(op_ty, ParsedType::Void) => {
                let mut out = format!("{} {op}", fmt_parsed_type(op_ty));
                for f in flags {
                    out.push(' ');
                    out.push_str(f);
                }
                out.push_str(&format!(
                    " ({}, {})",
                    self.operand_text(op_ty, lhs),
                    self.operand_text(op_ty, rhs)
                ));
                out
            }
            _ => format!("{} {}", fmt_parsed_type(op_ty), op.value_text()),
        }
    }

    /// 完整 LLVM 常量表达式文本（无外层括号；供 display 输出）。
    pub fn to_llvm_string(&self) -> String {
        match self {
            // 顶层 init 是裸全局引用（LLVM：`@p = global ptr @h` 的 init 为 `@h`）
            ConstExpr::GlobalAddr(g) => format!("@{g}"),
            ConstExpr::Int(n) => n.to_string(),
            ConstExpr::UInt(n) => format!("{n}"),
            ConstExpr::Float(f) => {
                if f.fract() == 0.0 && f.is_finite() {
                    format!("{f:.1}")
                } else {
                    format!("{f}")
                }
            }
            ConstExpr::Null => "null".to_string(),
            ConstExpr::Undef => "undef".to_string(),
            ConstExpr::Poison => "poison".to_string(),
            ConstExpr::PtrToInt { op_ty, op, to_ty } => {
                format!(
                    "ptrtoint ({} to {})",
                    self.operand_text(op_ty, op),
                    fmt_parsed_type(to_ty)
                )
            }
            ConstExpr::IntToPtr { op_ty, op, to_ty } => {
                format!(
                    "inttoptr ({} to {})",
                    self.operand_text(op_ty, op),
                    fmt_parsed_type(to_ty)
                )
            }
            ConstExpr::Bitcast { op_ty, op, to_ty } => {
                format!(
                    "bitcast ({} to {})",
                    self.operand_text(op_ty, op),
                    fmt_parsed_type(to_ty)
                )
            }
            ConstExpr::AddrSpaceCast { op_ty, op, to_ty } => {
                format!(
                    "addrspacecast ({} to {})",
                    self.operand_text(op_ty, op),
                    fmt_parsed_type(to_ty)
                )
            }
            ConstExpr::GetElementPtr {
                indexed_ty,
                op_ty,
                ptr,
                indices,
            } => {
                let mut out = format!(
                    "getelementptr ({}, {}",
                    fmt_parsed_type(indexed_ty),
                    self.operand_text(op_ty, ptr)
                );
                for (t, v) in indices {
                    out.push_str(&format!(", {}", self.operand_text(t, v)));
                }
                out.push(')');
                out
            }
            // 二元折叠表达式：`add nuw nsw (<lhs>, <rhs>)`（操作数裸输出——
            // 嵌套表达式无类型前缀；该路径仅在不折叠场景出现）
            ConstExpr::Binary {
                op,
                flags,
                lhs,
                rhs,
            } => {
                let mut out = op.clone();
                for f in flags {
                    out.push(' ');
                    out.push_str(f);
                }
                out.push_str(&format!(
                    " ({}, {})",
                    lhs.to_llvm_string(),
                    rhs.to_llvm_string()
                ));
                out
            }
            // 向量常量：`<2 x i32> <i32 3, i32 4>`
            ConstExpr::Vector(ty, lanes) => {
                if lanes.is_empty() {
                    // 向量 zeroinitializer（`<2 x ptr> zeroinitializer`——
                    // 第二十九轮:空 lanes 原输出 `<>`,reparse 拒绝）
                    return format!("{} zeroinitializer", fmt_parsed_type(ty));
                }
                let lanes_s: Vec<String> = lanes
                    .iter()
                    .map(|l| match l {
                        VecLane::Int(n) => format!("i32 {n}"),
                        VecLane::UInt(n) => format!("i32 {n}"),
                        VecLane::Float(f) => {
                            if f.fract() == 0.0 && f.is_finite() {
                                format!("float {f:.1}")
                            } else {
                                format!("float {f}")
                            }
                        }
                    })
                    .collect();
                format!("{} <{}>", fmt_parsed_type(ty), lanes_s.join(", "))
            }
            // 转换折叠：`trunc (i64 42 to i32)`
            ConstExpr::Cast {
                op,
                src_ty,
                src,
                to,
            } => format!(
                "{} ({} to {})",
                op,
                self.operand_text(src_ty, src),
                fmt_parsed_type(to)
            ),
            // ptrauth：`ptrauth (ptr @v, i32 0[, i64 5, ptr @x])`
            ConstExpr::Ptrauth(args) => {
                let parts: Vec<String> =
                    args.iter().map(|(t, v)| self.operand_text(t, v)).collect();
                format!("ptrauth ({})", parts.join(", "))
            }
            ConstExpr::Unsupported(name) => format!("{name} (unsupported)"),
        }
    }
}

pub struct ParsedModule {
    pub items: Vec<ParsedItem>,
    /// 属性组定义（`attributes #N = { ... }`）——semantics 展开到函数/指令。
    pub attr_groups: HashMap<u32, Vec<String>>,
}

// ── 函数/块/指令 ──

pub struct ParsedFunction {
    pub name: String,
    pub ret_ty: ParsedType,
    pub params: Vec<(ParsedType, String)>,
    pub blocks: Vec<ParsedBlock>,
    pub declare: bool,
    /// 调用约定（`fastcc`；None = 默认）。
    pub call_conv: Option<String>,
    /// 函数属性名列表（`nounwind`/`noinline`/...；None = 无）。
    pub attrs: Vec<String>,
    /// 每个参数的属性名列表（`i32 signext %a`；与 params 一一对应）。
    pub param_attrs: Vec<Vec<String>>,
    /// 返回类型属性（`define signext i8 @g`；空 = 无）。
    pub ret_attrs: Vec<String>,
    /// `declare dso_local`（非抢占声明）。
    pub dso_local: bool,
    /// 函数尾 metadata 附加：`define ... !dbg !0`（(名, 节点引用) 对）。
    pub metadata: Vec<(String, MetadataRef)>,
    /// 可变参数（LLVM：`declare i32 @printf(ptr, ...)` 的 `...`）。
    pub varargs: bool,
    /// personality 函数（LLVM：`define void @f() personality ptr @__gxx_personality_v0`）。
    pub personality: Option<String>,
    /// 函数尾 comdat 子句（`comdat $name`；None = 无；
    /// `Some("")` = 裸 comdat 无名形态——数字函数名 + 无名 comdat 语义层拒绝）。
    pub comdat: Option<String>,
}

pub struct ParsedBlock {
    /// 块内 uselistorder 指令（终结符后；LLVM use-list 特例）——(值名, 索引)。
    pub uselistorders: Vec<(String, Vec<i64>, ParsedType)>,
    pub label: String,
    /// 块标签是否为显式数字标签（`3:`——参与 LLVM 数字块 id 编号；
    /// 无标签块(隐式——LLVM NumberedVals 编号空间仅含无标签块;
    /// 第二十九轮重建方案:显式 entry 与无标签块的区分标志)。
    pub implicit: bool,
    /// 字符串标签 `"2"`/`-3` 解码后可能是数字形态但非数字标签）。
    pub label_is_num: bool,
    pub insts: Vec<ParsedInst>,
    pub terminator: ParsedTerminator,
}

pub struct ParsedInst {
    /// 结果名（`%r = ...`）；None 表示无结果指令。
    pub result: Option<String>,
    /// vconst 向量常量端序（true = Big，默认 Little）。
    pub endian: bool,
    /// load/store 的 `volatile` 关键字（LLVM）。
    pub volatile: bool,
    /// load/store/alloca 的 `align N` 属性（0 = 缺省；u64 防截断——值域语义层校验）。
    pub align: u64,
    /// 算术标志名（`nsw`/`nuw`/`exact`；非算术指令为空）。
    pub flags: Vec<String>,
    /// call 实参属性（与 args 并行；其它指令为空）。
    pub arg_attrs: Vec<Vec<String>>,
    /// call-site 函数属性（`call ... nounwind` / `call ... #0`；仅 call）。
    pub call_fn_attrs: Vec<String>,
    /// 指令尾 metadata 附加：`... !dbg !0`（(名, 节点引用) 对；命名引用预处理期解析）。
    pub metadata_attach: Vec<(String, MetadataRef)>,
    /// LLVM 指令名（add/load/...）。
    pub opcode: String,
    /// icmp/fcmp 条件（eq/slt/...）。
    pub cond: Option<String>,
    /// 类型化操作数（call 的首个操作数是 callee，load/store 地址也在此）。
    pub args: Vec<ParsedOperand>,
    /// phi 指令的入边（`phi i32 [ %v, %pred ], ...`）；非 phi 指令恒空。
    pub phi_incomings: Vec<PhiIncoming>,
}

/// phi 的一条入边：`[ <值>, %<前驱块> ]`。
pub struct PhiIncoming {
    /// 入边值（`%name` 或裸字面量；类型由 phi 头类型决定）。
    pub val: ParsedOperand,
    /// 前驱块引用（`%2` 数字 id 或 `%"2"`/`%name` 字符串名）。
    pub pred: LabelRef,
}

/// 块标签引用：`%2`（数字块 id——LLVM NumberedVals 编号）或
/// `%"2"`/`%name`/`%-3`（字符串标签名）。数字引用经编号映射解析。
#[derive(Clone, Debug)]
pub struct LabelRef {
    /// 名字（不带 `%`；字符串标签已去引号解码）。
    pub name: String,
    /// 是否为数字块 id 引用（`%N` 纯数字形态）。
    pub is_num: bool,
}

impl LabelRef {
    pub fn named(name: String) -> Self {
        let is_num = name.parse::<u32>().is_ok();
        LabelRef { name, is_num }
    }
}

impl ParsedInst {
    pub fn with_result(mut self, name: String) -> Self {
        self.result = Some(name);
        self
    }
}

#[derive(Clone, Debug)]
pub struct ParsedOperand {
    pub ty: ParsedType,
    pub op: Operand,
}

/// 聚合字面量序列化（`extractvalue [4 x i32] [i32 1, ...], 2` 的 agg 文本）。
/// 元素为标量字面量（Int/Float/Null/Undef/嵌套 Agg/ZeroInit）。
/// ParsedType → LLVM 类型文本（聚合字面量序列化用——语义简单版）。
pub fn fmt_parsed_type(t: &ParsedType) -> String {
    match t {
        ParsedType::Int(bits) => format!("i{bits}"),
        ParsedType::Float(bits) => format!("f{bits}"),
        ParsedType::Ptr => "ptr".to_string(),
        ParsedType::PtrAddrSpace(n) => format!("ptr addrspace({n})"),
        ParsedType::Vec(n, e) => format!("<{n} x {}>", fmt_parsed_type(e)),
        ParsedType::Array(n, e) => format!("[{n} x {}]", fmt_parsed_type(e)),
        ParsedType::Struct(tys) => {
            let inner = tys
                .iter()
                .map(fmt_parsed_type)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {inner} }}")
        }
        ParsedType::StructPacked(tys) => {
            let inner = tys
                .iter()
                .map(fmt_parsed_type)
                .collect::<Vec<_>>()
                .join(", ");
            format!("<{{ {inner} }}>")
        }
        ParsedType::Named(n) => format!("%{n}"),
        ParsedType::Void | ParsedType::Metadata => "void".to_string(),
        ParsedType::Opaque => "opaque".to_string(),
        ParsedType::Token => "token".to_string(),
        ParsedType::VecScalable(n, e) => {
            format!("<vscale x {n} x {}>", fmt_parsed_type(e))
        }
    }
}

pub fn fmt_agg_literal(elems: &[ParsedOperand], is_struct: bool) -> String {
    let inner = elems
        .iter()
        .map(|e| {
            let ty = match &e.ty {
                ParsedType::Struct(_) => format!(
                    "{{ {} }}",
                    fmt_agg_literal(e.op.agg_elems().unwrap_or_default(), true)
                ),
                _ => fmt_parsed_type(&e.ty),
            };
            format!("{} {}", ty, fmt_agg_op(&e.op))
        })
        .collect::<Vec<_>>()
        .join(", ");
    if is_struct {
        format!("{{ {inner} }}")
    } else {
        format!("[{inner}]")
    }
}

fn fmt_agg_op(op: &Operand) -> String {
    match op {
        Operand::Int(n) => n.to_string(),
        Operand::UInt(n) => n.to_string(),
        Operand::Float(f) => format!("{f}"),
        Operand::Null => "null".to_string(),
        Operand::Undef => "undef".to_string(),
        Operand::Poison => "poison".to_string(),
        Operand::Global(g) => format!("@{g}"),
        Operand::Bool(b) => format!("{b}"),
        _ => "?".to_string(),
    }
}

impl Operand {
    fn agg_elems(&self) -> Option<&[ParsedOperand]> {
        match self {
            Operand::Agg(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Operand {
    Local(String),
    Global(String),
    Int(i64),
    UInt(u64),
    Float(f64),
    Bool(bool),
    Null,
    Undef,
    Poison,
    /// 向量常量字面量：`<4 x float> <1.5, 2.5, -3.5, 4.25>`（lane 列表）。
    VecConst(Vec<VecLane>),
    /// `zeroinitializer` 关键字（类型由 ty 字段给出）。
    ZeroInit,
    /// 常量表达式操作数：`add (i64 <l>, i64 <r>)` / `ptrtoint (ptr @h to i64)`
    /// 等（ret/global init/call 实参等操作数位置；无类型前缀——类型由 ty
    /// 字段或上下文给出；semantics 求值折叠为常量）。
    ConstExpr(ConstExpr),
    /// 聚合常量字面量：`[i32 1, i32 2]`（数组）/ `{ i32 1, i64 2 }`（struct）。
    /// 元素为带类型的操作数；括号风格由 agg 类型（Array/Struct）决定。
    Agg(Vec<ParsedOperand>),
}

/// call/invoke 的 callee 操作数 → 函数名（`Operand::Global` 值含 `@`，与
/// semantics 的 func_refs key（含 `@`）一致；非全局 fallback 原样）。
pub fn format_operand_callee(op: &Operand) -> String {
    match op {
        Operand::Global(g) => g.clone(),
        // Local callee(间接调用——):保留裸局部名,
        // display 识别 % 开头输出(第二十九轮:原 Debug 格式被当函数名)
        Operand::Local(l) => l.clone(),
        other => format!("{other:?}"),
    }
}

/// 向量常量字面量的单个 lane 值（整数/浮点；类型由向量元素类型决定）。
#[derive(Clone, Copy, Debug)]
pub enum VecLane {
    Int(i64),
    UInt(u64),
    Float(f64),
}

/// 拆分向量常量字面量单 token：`<4 x float> <1.5, 2.5, -3.5, 4.25>`
/// → (向量类型, lane 列表)。
pub fn split_vec_const_lit(lit: &str) -> (ParsedType, Vec<VecLane>) {
    let (ty_part, lanes_part) = lit.split_once('>').unwrap_or((lit, ""));
    let ty = parse_vec_type(ty_part.trim());
    let lanes = parse_vec_lanes(lanes_part.trim());
    (ty, lanes)
}

/// 解析向量类型文本（`<4 x float` 或 `<4 x float>`）→ ParsedType::Vec。
fn parse_vec_type(s: &str) -> ParsedType {
    let inner = s.trim().trim_start_matches('<').trim_end_matches('>');
    let (n, elem) = inner
        .split_once('x')
        .map(|(n, e)| (n.trim(), e.trim()))
        .unwrap_or(("0", "i32"));
    let elem = match elem {
        "ptr" => ParsedType::Ptr,
        _ if elem.starts_with('i') => ParsedType::Int(elem[1..].parse::<u32>().unwrap_or(32)),
        _ => ParsedType::Float(match elem {
            "half" | "bfloat" => 16,
            "float" => 32,
            "double" => 64,
            "fp128" => 128,
            _ => elem[1..].parse::<u16>().unwrap_or(64),
        }),
    };
    ParsedType::Vec(n.parse::<u32>().unwrap_or(0), Box::new(elem))
}

/// 解析标量 zeroinitializer 单 token：`i32 zeroinitializer` → 标量类型。
pub fn split_scalar_ty(lit: &str) -> ParsedType {
    let ty = lit.trim_end_matches("zeroinitializer").trim();
    match ty {
        "ptr" => ParsedType::Ptr,
        "void" => ParsedType::Void,
        _ if ty.starts_with('i') => ParsedType::Int(ty[1..].parse::<u32>().unwrap_or(32)),
        _ => ParsedType::Float(match ty {
            "half" | "bfloat" => 16,
            "float" => 32,
            "double" => 64,
            "fp128" => 128,
            _ => ty[1..].parse::<u16>().unwrap_or(64),
        }),
    }
}

/// 解码 LLVM 字符串常量 token：`c"abc\00\n"` → 字节。
/// 支持转义：`\hh`（hex 字节）、`\n`/`\t`/`\r`、`\\`、`\"`、`\'`。
pub fn decode_c_string(s: &str) -> Vec<u8> {
    let inner = s
        .trim()
        .trim_start_matches('c')
        .trim_start_matches('"')
        .trim_end_matches('"');
    let bytes: Vec<char> = inner.chars().collect();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != '\\' {
            out.push(bytes[i] as u8);
            i += 1;
            continue;
        }
        i += 1; // 跳过 '\'
        if i >= bytes.len() {
            break;
        }
        match bytes[i] {
            'n' => {
                out.push(b'\n');
                i += 1;
            }
            't' => {
                out.push(b'\t');
                i += 1;
            }
            'r' => {
                out.push(b'\r');
                i += 1;
            }
            '\\' => {
                out.push(b'\\');
                i += 1;
            }
            '"' => {
                out.push(b'"');
                i += 1;
            }
            '\'' => {
                out.push(b'\'');
                i += 1;
            }
            '0'..='9' | 'a'..='f' | 'A'..='F' => {
                // \hh — 最多两位 hex
                let mut hex = String::new();
                while i < bytes.len() && hex.len() < 2 && bytes[i].is_ascii_hexdigit() {
                    hex.push(bytes[i]);
                    i += 1;
                }
                out.push(u8::from_str_radix(&hex, 16).unwrap_or(0));
            }
            other => {
                out.push(other as u8);
                i += 1;
            }
        }
    }
    out
}

/// 解析 lexer 的向量常量 lane 列表：`<1.5, 2.5, -3.5, 4.25>`。/// 按内容启发式区分：含 `.`/`e`/`E` → 浮点；`0x` 前缀 → 十六进制整数；
/// 否则十进制整数（类型由向量元素类型在语义层决定）。
pub fn parse_vec_lanes(s: &str) -> Vec<VecLane> {
    let inner = s.trim().trim_start_matches('<').trim_end_matches('>');
    inner
        .split(',')
        .map(|t| {
            let t = t.trim();
            if t.is_empty() {
                return VecLane::Int(0);
            }
            // 关键字 lane（`i1 true`/`i1 false`/`i1 undef`/`i1 poison`/`i1 null`/
            // `i32 zeroinitializer`）——先于浮点判定（false/true 含 'e' 会误判；
            // 第十二轮修复）
            if t.ends_with("true") {
                return VecLane::Int(1);
            }
            if t.ends_with("false") {
                return VecLane::Int(0);
            }
            if t.ends_with("undef")
                || t.ends_with("poison")
                || t.ends_with("null")
                || t.ends_with("zeroinitializer")
            {
                return VecLane::Int(0);
            }
            if t.contains('.') || t.contains('e') || t.contains('E') {
                VecLane::Float(t.parse::<f64>().unwrap_or(0.0))
            } else if let Some(rest) = t.strip_prefix("-0x").or_else(|| t.strip_prefix("0x")) {
                let v = u64::from_str_radix(rest.replace('_', "").as_str(), 16).unwrap_or(0);
                if t.starts_with('-') {
                    VecLane::Int(-(v as i64))
                } else {
                    VecLane::UInt(v)
                }
            } else if let Some(rest) = t.strip_prefix('-') {
                VecLane::Int(rest.parse::<i64>().unwrap_or(0).wrapping_neg())
            } else {
                match t.parse::<i64>() {
                    Ok(n) => VecLane::Int(n),
                    Err(_) => match t.parse::<u64>() {
                        Ok(u) => VecLane::UInt(u),
                        Err(_) => VecLane::Int(0),
                    },
                }
            }
        })
        .collect()
}

pub enum ParsedTerminator {
    Return(Vec<ParsedOperand>, Vec<(String, MetadataRef)>),
    /// 无条件跳转：`br label %t`（标准 LLVM；块参数已由 phi 表达）。
    Jump(LabelRef, Vec<(String, MetadataRef)>),
    /// 条件分支：`br i1 %c, label %t, label %f`。
    Branch(
        Option<ParsedOperand>,
        LabelRef,
        LabelRef,
        Vec<(String, MetadataRef)>,
    ),
    /// switch：discriminant + default + cases[(val, target)]。
    Switch(
        ParsedOperand,
        LabelRef,
        Vec<(i64, LabelRef)>,
        Vec<(String, MetadataRef)>,
    ),
    /// invoke：callee + ret_ty + args + normal + unwind（`to label %ok unwind label %pad`；
    /// result = 可选返回值绑定 `%r = invoke ...`）。
    Invoke {
        result: Option<String>,
        callee: String,
        ret_ty: ParsedType,
        args: Vec<ParsedOperand>,
        normal: LabelRef,
        unwind: LabelRef,
        metas: Vec<(String, MetadataRef)>,
    },
    /// resume：`resume <ty> %val`（重新抛出）。
    Resume(ParsedOperand, Vec<(String, MetadataRef)>),
    Unreachable,
}

// ── 类型树（语义层转 forge TypeId）──
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParsedType {
    Int(u32),
    Float(u16),
    Ptr,
    /// `ptr addrspace(N)`（N != 0）。
    PtrAddrSpace(u32),
    /// 命名类型引用：`%struct.X`（须已由 TypeDef 定义）。
    Named(String),
    Void,
    Metadata,
    Vec(u32, Box<ParsedType>),
    Array(u64, Box<ParsedType>),
    Struct(Vec<ParsedType>),
    /// 打包结构体（packed struct）：`<{ i32, i32 }>`（LLVM 现代语法——
    /// 无对齐填充；`TypeEntry::Struct.is_packed` 落位）。
    StructPacked(Vec<ParsedType>),
    /// 可伸缩向量（LLVM `<vscale x 4 x i32>`——SVE/RVV；`min_len` 为
    /// 最小 lane 数，实际长度运行时确定）。
    VecScalable(u32, Box<ParsedType>),
    /// 不透明占位类型（LLVM `opaque`——与 ptr 并列的容器外类型；无大小，
    /// 只能作占位，load/store 目标报错）。
    Opaque,
    /// token 类型（LLVM `token`——coroutine/异常处理专用；只能经
    /// phi/select 传递，不能存储）。
    Token,
}

impl From<ElemTy> for ParsedType {
    fn from(e: ElemTy) -> Self {
        match e {
            ElemTy::Int(bits) => ParsedType::Int(bits),
            ElemTy::Float(bits) => ParsedType::Float(bits),
            ElemTy::Ptr => ParsedType::Ptr,
        }
    }
}
