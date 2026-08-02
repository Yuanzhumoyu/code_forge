//! 类型系统 — TypeStore 提供类型 interning、大小/对齐查询。
//!
//! 核心设计:
//! - `TypeId(u32)` 是 Copy 句柄，在 `TypeStore` 中解析
//! - 类型按结构自动去重 (interning): 相同结构的类型返回同一 TypeId
//! - 命名 struct 按名去重 (支持递归类型)
//! - 基本类型预填充为固定索引

use super::data_layout::DataLayout;
use super::entity::SigRef;
use super::entity::TypeId;
use super::string_pool::InternedStr;
use super::string_pool::StringPool;
use crate::big::FloatFormat;
use std::collections::HashMap;
use std::fmt;

// ============================================================
// TypeEntry
// ============================================================

/// 类型定义 — 存储在 TypeStore 的扁平数组中。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeEntry {
    /// 任意位宽整数: i1 (bool), i8, i32, i64...
    Int { bits: u16 },

    /// IEEE 754 浮点: f16, f32, f64, f128
    Float { bits: u16 },

    /// Brain Float 16 (ML/AI 工作负载常用)
    BFloat { bits: u16 },

    /// 固定长度向量: <N x elem>
    Vector { elem: TypeId, len: u32 },

    /// 可扩展向量 (ARM SVE / RISC-V RVV): <vscale x N x elem>
    /// 编译时未知精确大小，运行时向量长度 = vscale * min_len
    ScalableVector { elem: TypeId, min_len: u32 },

    /// 数组: [elem x len]
    Array { elem: TypeId, len: u64 },

    /// 结构体
    Struct {
        /// None = 匿名结构 (按字段结构去重)
        /// Some = 命名结构 (按名去重，支持递归类型)
        name: Option<InternedStr>,
        fields: Vec<TypeField>,
        is_packed: bool,
    },

    /// Opaque 指针 (LLVM 15+ 风格，不存储 pointee type)
    Pointer { addr_space: u32 },

    /// 函数类型
    Function {
        params: Vec<TypeId>,
        rets: Vec<TypeId>,
        is_vararg: bool,
    },

    /// Token 类型 — 异常处理 landingpad 结果类型
    /// 不能存储到内存，只能通过 phi/select 传递
    Token,

    /// 元数据引用类型 — 用于 metadata 值的类型标记
    Metadata,
}

/// 结构体字段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeField {
    pub name: Option<InternedStr>,
    pub ty: TypeId,
}

impl TypeField {
    pub fn new(ty: TypeId) -> Self {
        Self { name: None, ty }
    }

    pub fn named(name: InternedStr, ty: TypeId) -> Self {
        Self {
            name: Some(name),
            ty,
        }
    }
}

// ============================================================
// TypeStore
// ============================================================

/// 类型去重键 — 匿名类型按结构去重，命名类型按名去重。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum TypeKey {
    IntBits(u16),
    FloatBits(u16),
    BFloatBits(u16),
    Vector(TypeId, u32),
    ScalableVector(TypeId, u32),
    Array(TypeId, u64),
    StructName(InternedStr),
    StructAnon(Vec<TypeId>, bool), // (field_types, is_packed)
    Pointer(u32),
    Function(Vec<TypeId>, Vec<TypeId>, bool),
    Token,
    Metadata,
}

/// 类型存储 — 全局类型 interner。
///
/// 一个 `TypeStore` 在 `Module` 中创建，跨所有函数共享。
/// 预填充的基本类型保证固定索引，可直接通过 `store.i32_ty` 等访问。
#[derive(Clone, Debug)]
pub struct TypeStore {
    /// 所有类型的扁平存储
    entries: Vec<TypeEntry>,

    /// 去重映射: TypeKey → TypeId
    dedup: HashMap<TypeKey, TypeId>,

    /// 字符串 interner — 用于 struct 名称、字段名、参数名等
    pub strings: StringPool,

    /// 函数签名存储 — 通过 SigRef 引用
    signatures: Vec<FunctionSignature>,

    /// 目标数据布局 — 控制指针大小、对齐等
    pub data_layout: DataLayout,

    // === 预填充的基本类型 ID ===
    pub void_ty: TypeId,
    pub bool_ty: TypeId,
    pub i8_ty: TypeId,
    pub i16_ty: TypeId,
    pub i32_ty: TypeId,
    pub i64_ty: TypeId,
    pub f32_ty: TypeId,
    pub f64_ty: TypeId,
    pub ptr_ty: TypeId,
}

impl TypeStore {
    /// 创建 TypeStore 并预填充基本类型。
    pub fn new() -> Self {
        Self::with_data_layout(DataLayout::default())
    }

    /// 创建 TypeStore 并预填充基本类型，使用指定的 DataLayout。
    pub fn with_data_layout(data_layout: DataLayout) -> Self {
        let mut store = Self {
            entries: Vec::with_capacity(64),
            dedup: HashMap::with_capacity(64),
            strings: StringPool::new(),
            signatures: Vec::new(),
            data_layout,
            void_ty: TypeId(0),
            bool_ty: TypeId(0),
            i8_ty: TypeId(0),
            i16_ty: TypeId(0),
            i32_ty: TypeId(0),
            i64_ty: TypeId(0),
            f32_ty: TypeId(0),
            f64_ty: TypeId(0),
            ptr_ty: TypeId(0),
        };

        // 预填充基本类型 — 按固定顺序确保稳定 ID
        // 索引 0: void (占位)
        store.void_ty = store.raw_insert(TypeEntry::Int { bits: 0 }, TypeKey::IntBits(0));
        // 索引 1: bool (i1)
        store.bool_ty = store.raw_insert(TypeEntry::Int { bits: 1 }, TypeKey::IntBits(1));
        // 索引 2-5: 整数
        store.i8_ty = store.raw_insert(TypeEntry::Int { bits: 8 }, TypeKey::IntBits(8));
        store.i16_ty = store.raw_insert(TypeEntry::Int { bits: 16 }, TypeKey::IntBits(16));
        store.i32_ty = store.raw_insert(TypeEntry::Int { bits: 32 }, TypeKey::IntBits(32));
        store.i64_ty = store.raw_insert(TypeEntry::Int { bits: 64 }, TypeKey::IntBits(64));
        // 索引 6-7: 浮点
        store.f32_ty = store.raw_insert(TypeEntry::Float { bits: 32 }, TypeKey::FloatBits(32));
        store.f64_ty = store.raw_insert(TypeEntry::Float { bits: 64 }, TypeKey::FloatBits(64));
        // 索引 8: 指针 (地址空间 0)
        store.ptr_ty = store.raw_insert(TypeEntry::Pointer { addr_space: 0 }, TypeKey::Pointer(0));

        // 验证 TypeId 常量与 lib.rs 中的硬编码预设一致
        debug_assert_eq!(store.void_ty, TypeId::VOID, "void_ty mismatch");
        debug_assert_eq!(store.bool_ty, TypeId::BOOL, "bool_ty mismatch");
        debug_assert_eq!(store.i8_ty, TypeId::I8, "i8_ty mismatch");
        debug_assert_eq!(store.i16_ty, TypeId::I16, "i16_ty mismatch");
        debug_assert_eq!(store.i32_ty, TypeId::I32, "i32_ty mismatch");
        debug_assert_eq!(store.i64_ty, TypeId::I64, "i64_ty mismatch");
        debug_assert_eq!(store.f32_ty, TypeId::F32, "f32_ty mismatch");
        debug_assert_eq!(store.f64_ty, TypeId::F64, "f64_ty mismatch");
        debug_assert_eq!(store.ptr_ty, TypeId::PTR, "ptr_ty mismatch");

        store
    }

    /// 内部: 直接插入类型 (跳过去重检查，预填充时使用)。
    fn raw_insert(&mut self, entry: TypeEntry, key: TypeKey) -> TypeId {
        let id = TypeId(self.entries.len() as u32);
        self.entries.push(entry);
        self.dedup.insert(key, id);
        id
    }

    /// 插入/获取类型 (自动去重)。
    fn intern(&mut self, entry: TypeEntry, key: TypeKey) -> TypeId {
        if let Some(&existing) = self.dedup.get(&key) {
            return existing;
        }
        self.raw_insert(entry, key)
    }

    // ============================================================
    // 类型工厂
    // ============================================================

    pub fn int_ty(&mut self, bits: u16) -> TypeId {
        self.intern(TypeEntry::Int { bits }, TypeKey::IntBits(bits))
    }

    pub fn float_ty(&mut self, bits: u16) -> TypeId {
        self.intern(TypeEntry::Float { bits }, TypeKey::FloatBits(bits))
    }

    pub fn vector_ty(&mut self, elem: TypeId, len: u32) -> TypeId {
        self.intern(TypeEntry::Vector { elem, len }, TypeKey::Vector(elem, len))
    }

    /// 创建可扩展向量类型: <vscale x min_len x elem> (ARM SVE / RISC-V RVV)
    pub fn scalable_vector_ty(&mut self, elem: TypeId, min_len: u32) -> TypeId {
        self.intern(
            TypeEntry::ScalableVector { elem, min_len },
            TypeKey::ScalableVector(elem, min_len),
        )
    }

    pub fn bfloat_ty(&mut self, bits: u16) -> TypeId {
        self.intern(TypeEntry::BFloat { bits }, TypeKey::BFloatBits(bits))
    }

    pub fn token_ty(&mut self) -> TypeId {
        self.intern(TypeEntry::Token, TypeKey::Token)
    }

    pub fn metadata_ty(&mut self) -> TypeId {
        self.intern(TypeEntry::Metadata, TypeKey::Metadata)
    }

    pub fn array_ty(&mut self, elem: TypeId, len: u64) -> TypeId {
        self.intern(TypeEntry::Array { elem, len }, TypeKey::Array(elem, len))
    }

    pub fn pointer_ty(&mut self, addr_space: u32) -> TypeId {
        self.intern(
            TypeEntry::Pointer { addr_space },
            TypeKey::Pointer(addr_space),
        )
    }

    pub fn function_ty(&mut self, params: Vec<TypeId>, rets: Vec<TypeId>, vararg: bool) -> TypeId {
        self.intern(
            TypeEntry::Function {
                params: params.clone(),
                rets: rets.clone(),
                is_vararg: vararg,
            },
            TypeKey::Function(params, rets, vararg),
        )
    }

    pub fn struct_named(&mut self, name: &str, fields: Vec<TypeField>, packed: bool) -> TypeId {
        let name_id = self.strings.intern(name);
        self.intern(
            TypeEntry::Struct {
                name: Some(name_id),
                fields,
                is_packed: packed,
            },
            TypeKey::StructName(name_id),
        )
    }

    /// Intern 一个字符串，返回 InternedStr 句柄。
    pub fn intern_str(&mut self, s: &str) -> InternedStr {
        self.strings.intern(s)
    }

    /// 查找 InternedStr 对应的字符串。
    pub fn lookup_str(&self, id: InternedStr) -> &str {
        self.strings.lookup(id)
    }

    pub fn struct_anon(&mut self, field_tys: Vec<TypeId>, packed: bool) -> TypeId {
        self.intern(
            TypeEntry::Struct {
                name: None,
                fields: field_tys.iter().map(|&ty| TypeField::new(ty)).collect(),
                is_packed: packed,
            },
            TypeKey::StructAnon(field_tys, packed),
        )
    }

    // ============================================================
    // 函数签名管理
    // ============================================================

    /// 注册一个预构建的函数签名，返回 SigRef。
    /// 参数名应已通过 `FunctionSignature::new()` 或类似方式 intern。
    pub fn register_signature(&mut self, sig: FunctionSignature) -> SigRef {
        let id = SigRef(self.signatures.len() as u32);
        self.signatures.push(sig);
        id
    }

    /// 从参数描述构建并注册函数签名，返回 SigRef。
    pub fn make_signature(
        &mut self,
        params: &[(TypeId, &str)],
        returns: &[TypeId],
        cc: CallConv,
    ) -> SigRef {
        let sig = FunctionSignature::new(params, returns).with_calling_convention(cc);
        self.register_signature(sig)
    }

    /// 按 SigRef 查询函数签名。
    pub fn get_signature(&self, sr: SigRef) -> &FunctionSignature {
        &self.signatures[sr.0 as usize]
    }

    /// 返回已注册的签名数量。
    pub fn signature_count(&self) -> usize {
        self.signatures.len()
    }

    // ============================================================
    // 查询
    // ============================================================

    pub fn get(&self, id: TypeId) -> &TypeEntry {
        &self.entries[id.0 as usize]
    }

    pub fn type_count(&self) -> usize {
        self.entries.len()
    }

    // === 类型判断 ===

    pub fn is_int(&self, id: TypeId) -> bool {
        matches!(self.get(id), TypeEntry::Int { .. })
    }

    pub fn is_float(&self, id: TypeId) -> bool {
        matches!(self.get(id), TypeEntry::Float { .. })
    }

    pub fn is_ptr(&self, id: TypeId) -> bool {
        matches!(self.get(id), TypeEntry::Pointer { .. })
    }

    pub fn is_vector(&self, id: TypeId) -> bool {
        matches!(self.get(id), TypeEntry::Vector { .. })
    }

    pub fn is_scalable_vector(&self, id: TypeId) -> bool {
        matches!(self.get(id), TypeEntry::ScalableVector { .. })
    }

    pub fn is_bfloat(&self, id: TypeId) -> bool {
        matches!(self.get(id), TypeEntry::BFloat { .. })
    }

    pub fn is_token(&self, id: TypeId) -> bool {
        matches!(self.get(id), TypeEntry::Token)
    }

    pub fn is_aggregate(&self, id: TypeId) -> bool {
        matches!(
            self.get(id),
            TypeEntry::Struct { .. } | TypeEntry::Array { .. }
        )
    }

    pub fn is_void(&self, id: TypeId) -> bool {
        id == self.void_ty
    }

    /// 获取浮点格式描述。
    pub fn float_format(&self, id: TypeId) -> Option<FloatFormat> {
        match self.get(id) {
            TypeEntry::Float { bits: 16 } => Some(FloatFormat::F16),
            TypeEntry::Float { bits: 32 } => Some(FloatFormat::F32),
            TypeEntry::Float { bits: 64 } => Some(FloatFormat::F64),
            TypeEntry::Float { bits: 128 } => Some(FloatFormat::F128),
            _ => None,
        }
    }

    // === 大小 / 对齐 ===

    fn align_to(offset: u32, align: u32) -> u32 {
        offset.div_ceil(align) * align
    }

    pub fn size_bytes(&self, id: TypeId) -> u32 {
        let entry = self.get(id);
        match entry {
            TypeEntry::Int { bits } => (*bits).div_ceil(8) as u32,
            TypeEntry::Float { bits } => (*bits).div_ceil(8) as u32,
            TypeEntry::BFloat { .. } => 2, // bf16 = 2 bytes
            TypeEntry::Vector { elem, len } => self.size_bytes(*elem) * len,
            TypeEntry::ScalableVector { elem, min_len } => {
                // Minimum compile-time size: one lane * min_len
                self.size_bytes(*elem) * min_len
            }
            TypeEntry::Array { elem, len } => self.size_bytes(*elem) * (*len as u32),
            TypeEntry::Struct {
                fields, is_packed, ..
            } => {
                if *is_packed {
                    fields.iter().map(|f| self.size_bytes(f.ty)).sum()
                } else {
                    let mut offset = 0u32;
                    let mut max_align = 1u32;
                    for f in fields {
                        let align = self.alignment(f.ty);
                        let size = self.size_bytes(f.ty);
                        max_align = max_align.max(align);
                        offset = Self::align_to(offset, align);
                        offset += size;
                    }
                    Self::align_to(offset, max_align)
                }
            }
            TypeEntry::Pointer { addr_space } => self.data_layout.pointer_size(*addr_space),
            TypeEntry::Function { .. } => self.data_layout.pointer_size(0),
            TypeEntry::Token => 0,
            TypeEntry::Metadata => 0,
        }
    }

    pub fn alignment(&self, id: TypeId) -> u32 {
        let entry = self.get(id);
        match entry {
            TypeEntry::Int { bits } => self.data_layout.integer_align(*bits),
            TypeEntry::Float { bits } => self.data_layout.float_align(*bits),
            TypeEntry::BFloat { .. } => 2,
            TypeEntry::Vector { elem: _, len: _ } => {
                let total_bits = self.size_bytes(id) * 8;
                self.data_layout.vector_align(total_bits)
            }
            TypeEntry::ScalableVector { elem, .. } => self.alignment(*elem),
            TypeEntry::Array { elem, .. } => self.alignment(*elem),
            TypeEntry::Struct {
                fields, is_packed, ..
            } => {
                if *is_packed {
                    1
                } else if self.data_layout.aggregate_align > 0 {
                    self.data_layout.aggregate_align
                } else {
                    fields
                        .iter()
                        .map(|f| self.alignment(f.ty))
                        .max()
                        .unwrap_or(1)
                        .min(self.data_layout.max_alignment)
                }
            }
            TypeEntry::Pointer { addr_space } => self.data_layout.pointer_align(*addr_space),
            TypeEntry::Function { .. } => self.data_layout.pointer_size(0),
            TypeEntry::Token => 1,
            TypeEntry::Metadata => 1,
        }
    }

    /// 获取向量类型的 lane 数量。
    pub fn vector_len(&self, id: TypeId) -> Option<u32> {
        match self.get(id) {
            TypeEntry::Vector { len, .. } => Some(*len),
            _ => None,
        }
    }

    /// 获取向量/数组的元素类型。
    pub fn element_type(&self, id: TypeId) -> Option<TypeId> {
        match self.get(id) {
            TypeEntry::Vector { elem, .. } | TypeEntry::Array { elem, .. } => Some(*elem),
            _ => None,
        }
    }
}

impl Default for TypeStore {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 函数签名 (存储在 TypeStore 中)
// ============================================================

/// 调用约定。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CallConv {
    #[default]
    Default,
    /// System V AMD64 ABI (Linux, macOS, BSD)
    SystemV,
    /// Microsoft x64 calling convention
    WindowsX64,
    /// Fast call (pass args in registers when possible)
    Fast,
    /// C declaration (caller cleans stack)
    CDecl,
    /// Internal convention (compiler-private)
    Internal,
    /// ARM Architecture Procedure Call Standard
    Aapcs,
    /// AAPCS with VFP (hard-float for ARM)
    AapcsVfp,
    /// RISC-V ILP32 (32-bit ints/pointers)
    RiscvIlp32,
    /// RISC-V LP64 (64-bit)
    RiscvLp64,
    /// WebAssembly basic C ABI
    WasmBasic,
    /// stdcall (Win32, callee cleans stack)
    StdCall,
    /// vectorcall (pass vector args in registers)
    VectorCall,
    /// Preserve most registers (callee-saved heavy, for hot calls)
    PreserveMost,
    /// Preserve all registers (callee saves everything)
    PreserveAll,
    /// Cold function (optimize for size, not speed)
    Cold,
}

/// 函数签名 (存储为 TypeEntry::Function 的辅助查询结构)。
///
/// 签名的权威副本存储在 `TypeStore::signatures` 中，通过 `SigRef` 引用。
/// 参数名用 `String` — 注册到 TypeStore 时自动通过 StringPool intern。
#[derive(Clone, Debug)]
pub struct FunctionSignature {
    pub params: Vec<(TypeId, String)>,
    pub returns: Vec<TypeId>,
    pub calling_convention: CallConv,
}

impl FunctionSignature {
    /// 创建一个新的函数签名。
    pub fn new(params: &[(TypeId, &str)], returns: &[TypeId]) -> Self {
        Self {
            params: params.iter().map(|(t, n)| (*t, n.to_string())).collect(),
            returns: returns.to_vec(),
            calling_convention: CallConv::default(),
        }
    }

    pub fn void() -> Self {
        Self {
            params: Vec::new(),
            returns: Vec::new(),
            calling_convention: CallConv::default(),
        }
    }

    pub fn with_calling_convention(mut self, cc: CallConv) -> Self {
        self.calling_convention = cc;
        self
    }

    /// 获取参数类型列表（无参数名）。
    pub fn param_types(&self) -> Vec<TypeId> {
        self.params.iter().map(|(t, _)| *t).collect()
    }
}

// ============================================================
// Display
// ============================================================

impl fmt::Display for TypeStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for i in 0..self.entries.len() {
            let id = TypeId(i as u32);
            writeln!(f, "  {} = {}", id, self.fmt_type(id))?;
        }
        Ok(())
    }
}

impl TypeStore {
    /// 格式化单个类型。
    pub fn fmt_type(&self, id: TypeId) -> String {
        match self.get(id) {
            TypeEntry::Int { bits: 0 } => "void".to_string(),
            TypeEntry::Int { bits: 1 } => "bool".to_string(),
            TypeEntry::Int { bits } => format!("i{}", bits),
            TypeEntry::Float { bits } => format!("f{}", bits),
            TypeEntry::BFloat { bits } => format!("bf{}", bits),
            TypeEntry::Vector { elem, len } => format!("<{} x {}>", len, self.fmt_type(*elem)),
            TypeEntry::ScalableVector { elem, min_len } => {
                format!("<vscale x {} x {}>", min_len, self.fmt_type(*elem))
            }
            TypeEntry::Array { elem, len } => format!("[{} x {}]", len, self.fmt_type(*elem)),
            TypeEntry::Struct {
                name,
                fields,
                is_packed: _,
            } => {
                let name_str = name
                    .as_ref()
                    .map(|id| self.strings.lookup(*id))
                    .unwrap_or("anon");
                let fields_str: Vec<String> = fields.iter().map(|f| self.fmt_type(f.ty)).collect();
                format!("struct_{}({})", name_str, fields_str.join(", "))
            }
            TypeEntry::Pointer { addr_space } => {
                if *addr_space == 0 {
                    "ptr".to_string()
                } else {
                    format!("ptr_as{}", addr_space)
                }
            }
            TypeEntry::Function {
                params,
                rets,
                is_vararg,
            } => {
                let p: Vec<String> = params.iter().map(|t| self.fmt_type(*t)).collect();
                let r: Vec<String> = rets.iter().map(|t| self.fmt_type(*t)).collect();
                let va = if *is_vararg { ", ..." } else { "" };
                format!("fn({}) -> ({}){}", p.join(", "), r.join(", "), va)
            }
            TypeEntry::Token => "token".to_string(),
            TypeEntry::Metadata => "metadata".to_string(),
        }
    }
}

// ============================================================
// TypeContext — shared, interior-mutable TypeStore reference
// ============================================================

use std::ops::Deref;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// A shared, interior-mutable reference to a `TypeStore`.
///
/// Created once in a `Module` and shared across all builders, functions,
/// verifiers, and display contexts via cheap `Arc`-based `Clone`.
///
/// Uses `Arc<RwLock<TypeStore>>` for `Send + Sync` thread safety
/// when the optimization framework accesses types across threads.
///
/// # Usage
///
/// ```text
/// let ctx = TypeContext::new();
/// let i32_ty = ctx.i32_ty();              // convenience accessor
/// let sig = ctx.borrow().get_signature(sr); // transparent Deref → RwLock
/// ctx.borrow_mut().register_signature(s);    // mutable access
/// ```
#[derive(Clone, Debug)]
pub struct TypeContext(Arc<RwLock<TypeStore>>);

impl TypeContext {
    /// Create a new TypeContext wrapping a fresh TypeStore.
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(TypeStore::new())))
    }

    /// Create a TypeContext from an existing TypeStore.
    pub fn from_store(store: TypeStore) -> Self {
        Self(Arc::new(RwLock::new(store)))
    }

    /// Create with a custom DataLayout.
    pub fn with_data_layout(data_layout: DataLayout) -> Self {
        Self(Arc::new(RwLock::new(TypeStore::with_data_layout(
            data_layout,
        ))))
    }

    /// Immutable read access to the TypeStore.
    pub fn borrow(&self) -> RwLockReadGuard<'_, TypeStore> {
        self.0.read().expect("TypeStore RwLock poisoned")
    }

    /// Mutable write access to the TypeStore.
    pub fn borrow_mut(&self) -> RwLockWriteGuard<'_, TypeStore> {
        self.0.write().expect("TypeStore RwLock poisoned")
    }

    // === Pre-filled type convenience accessors (immutable borrow) ===

    pub const fn void_ty(&self) -> TypeId {
        TypeId::VOID
    }
    pub const fn bool_ty(&self) -> TypeId {
        TypeId::BOOL
    }
    pub const fn i8_ty(&self) -> TypeId {
        TypeId::I8
    }
    pub const fn i16_ty(&self) -> TypeId {
        TypeId::I16
    }
    pub const fn i32_ty(&self) -> TypeId {
        TypeId::I32
    }
    pub const fn i64_ty(&self) -> TypeId {
        TypeId::I64
    }
    pub const fn f32_ty(&self) -> TypeId {
        TypeId::F32
    }
    pub const fn f64_ty(&self) -> TypeId {
        TypeId::F64
    }
    pub const fn ptr_ty(&self) -> TypeId {
        TypeId::PTR
    }

    // === Delegated read-only methods ===

    pub fn is_int(&self, id: TypeId) -> bool {
        self.borrow().is_int(id)
    }
    pub fn is_float(&self, id: TypeId) -> bool {
        self.borrow().is_float(id)
    }
    pub fn is_ptr(&self, id: TypeId) -> bool {
        self.borrow().is_ptr(id)
    }
    pub fn is_void(&self, id: TypeId) -> bool {
        self.borrow().is_void(id)
    }
    pub fn is_vector(&self, id: TypeId) -> bool {
        self.borrow().is_vector(id)
    }
    pub fn size_bytes(&self, id: TypeId) -> u32 {
        self.borrow().size_bytes(id)
    }
    pub fn alignment(&self, id: TypeId) -> u32 {
        self.borrow().alignment(id)
    }
    pub fn fmt_type(&self, id: TypeId) -> String {
        self.borrow().fmt_type(id)
    }
    pub fn type_count(&self) -> usize {
        self.borrow().type_count()
    }
    pub fn signature_count(&self) -> usize {
        self.borrow().signature_count()
    }
    pub fn lookup_str(&self, id: InternedStr) -> String {
        self.borrow().strings.lookup(id).to_string()
    }

    // === Delegated mutable methods (require borrow_mut) ===

    pub fn int_ty(&self, bits: u16) -> TypeId {
        self.borrow_mut().int_ty(bits)
    }
    pub fn float_ty(&self, bits: u16) -> TypeId {
        self.borrow_mut().float_ty(bits)
    }
    pub fn bfloat_ty(&self, bits: u16) -> TypeId {
        self.borrow_mut().bfloat_ty(bits)
    }
    pub fn vector_ty(&self, elem: TypeId, len: u32) -> TypeId {
        self.borrow_mut().vector_ty(elem, len)
    }
    pub fn array_ty(&self, elem: TypeId, len: u64) -> TypeId {
        self.borrow_mut().array_ty(elem, len)
    }
    pub fn pointer_ty(&self, addr_space: u32) -> TypeId {
        self.borrow_mut().pointer_ty(addr_space)
    }
    pub fn register_signature(&self, sig: FunctionSignature) -> SigRef {
        self.borrow_mut().register_signature(sig)
    }
    pub fn get_signature(&self, sr: SigRef) -> FunctionSignature {
        self.borrow().get_signature(sr).clone()
    }
    pub fn intern_str(&self, s: &str) -> InternedStr {
        self.borrow_mut().intern_str(s)
    }
}

impl Deref for TypeContext {
    type Target = RwLock<TypeStore>;

    fn deref(&self) -> &RwLock<TypeStore> {
        &self.0
    }
}

impl Default for TypeContext {
    fn default() -> Self {
        Self::new()
    }
}

impl From<TypeStore> for TypeContext {
    fn from(store: TypeStore) -> Self {
        Self::from_store(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_types() {
        let store = TypeStore::new();
        assert!(store.is_int(store.i32_ty));
        assert!(store.is_float(store.f64_ty));
        assert!(store.is_ptr(store.ptr_ty));
        assert_eq!(store.size_bytes(store.i32_ty), 4);
        assert_eq!(store.size_bytes(store.i64_ty), 8);
        assert_eq!(store.size_bytes(store.f64_ty), 8);
        assert_eq!(store.size_bytes(store.ptr_ty), 8);
        assert_eq!(store.alignment(store.i32_ty), 4);
        assert_eq!(store.alignment(store.i64_ty), 8);
    }

    #[test]
    fn test_int_types() {
        let mut store = TypeStore::new();
        let i128 = store.int_ty(128);
        assert_eq!(store.size_bytes(i128), 16);

        // 相同位宽应该返回相同 TypeId (interning)
        let i128b = store.int_ty(128);
        assert_eq!(i128, i128b);
    }

    #[test]
    fn test_vector_types() {
        let mut store = TypeStore::new();
        let v4i32 = store.vector_ty(store.i32_ty, 4);
        assert!(store.is_vector(v4i32));
        assert_eq!(store.size_bytes(v4i32), 16); // 4 * 4
        assert_eq!(store.vector_len(v4i32), Some(4));
        assert_eq!(store.element_type(v4i32), Some(store.i32_ty));
    }

    #[test]
    fn test_array_types() {
        let mut store = TypeStore::new();
        let arr = store.array_ty(store.i32_ty, 10);
        assert!(store.is_aggregate(arr));
        assert_eq!(store.size_bytes(arr), 40); // 10 * 4
        assert_eq!(store.element_type(arr), Some(store.i32_ty));
    }

    #[test]
    fn test_struct_packed() {
        let mut store = TypeStore::new();
        let s = store.struct_anon(vec![store.i8_ty, store.i32_ty], true);
        assert_eq!(store.size_bytes(s), 5); // 1 + 4
    }

    #[test]
    fn test_struct_aligned() {
        let mut store = TypeStore::new();
        let s = store.struct_anon(vec![store.i8_ty, store.i32_ty], false);
        assert_eq!(store.size_bytes(s), 8); // 1 + 3 pad + 4
        assert_eq!(store.alignment(s), 4);
    }

    #[test]
    fn test_pointer_types() {
        let mut store = TypeStore::new();
        let p = store.pointer_ty(0);
        assert!(store.is_ptr(p));
        assert_eq!(store.size_bytes(p), 8);

        // 相同地址空间应去重
        let p2 = store.pointer_ty(0);
        assert_eq!(p, p2);

        // 不同地址空间
        let p3 = store.pointer_ty(1);
        assert_ne!(p, p3);
    }

    #[test]
    fn test_function_types() {
        let mut store = TypeStore::new();
        let ft = store.function_ty(vec![store.i32_ty, store.i32_ty], vec![store.i64_ty], false);
        match store.get(ft) {
            TypeEntry::Function { params, rets, .. } => {
                assert_eq!(params.len(), 2);
                assert_eq!(rets.len(), 1);
            }
            _ => panic!("expected Function type"),
        }
    }
}
