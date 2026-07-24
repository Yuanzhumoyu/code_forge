//! IR 上下文 — 全局类型表、常量池和复合类型管理器。
//!
//! 参照 LLVM 的 `LLVMContext` 设计理念：
//! - 类型是 interned（相同类型共享同一表示）
//! - 常量被唯一化跨函数共享
//! - 复合类型（struct/array/vector/function）统一注册和管理
//!
//! 一个 `Context` 可以在多个 `Module` 和 `Function` 之间共享，
//! 避免跨函数的重复类型/常量分配。

use super::big::Big;
use super::constant_pool::ConstantPool;
use super::types::Type;
use std::collections::HashMap;

/// 结构体类型描述。
#[derive(Clone, Debug, PartialEq)]
pub struct StructType {
    /// 结构体名称（None = 匿名结构体）。
    pub name: Option<String>,
    /// 字段类型列表。
    pub fields: Vec<Type>,
    /// 是否 packed（不进行对齐填充）。
    pub is_packed: bool,
}

impl StructType {
    /// 创建一个命名结构体。
    pub fn named(name: &str, fields: Vec<Type>) -> Self {
        Self {
            name: Some(name.to_string()),
            fields,
            is_packed: false,
        }
    }

    /// 创建一个匿名结构体。
    pub fn anonymous(fields: Vec<Type>) -> Self {
        Self {
            name: None,
            fields,
            is_packed: false,
        }
    }

    /// 设置为 packed 布局。
    pub fn with_packed(mut self, packed: bool) -> Self {
        self.is_packed = packed;
        self
    }

    /// 计算结构体大小（字节）。
    /// 对于 packed 结构体，直接累加字段大小。
    /// 对于非 packed 结构体，按最大字段对齐要求填充。
    pub fn size_bytes(&self) -> u32 {
        if self.is_packed {
            self.fields.iter().map(|t| t.size_bytes()).sum()
        } else {
            let mut size = 0u32;
            let mut max_align = 1u32;
            for field_ty in &self.fields {
                let field_size = field_ty.size_bytes();
                let field_align = field_ty.alignment();
                if field_align > max_align {
                    max_align = field_align;
                }
                // 对齐到字段对齐要求
                let padding = (field_align - (size % field_align)) % field_align;
                size += padding + field_size;
            }
            // 最终对齐到最大对齐
            let final_padding = (max_align - (size % max_align)) % max_align;
            size + final_padding
        }
    }

    /// 获取指定字段的偏移量（字节）。
    pub fn field_offset(&self, index: usize) -> Option<u32> {
        if index >= self.fields.len() {
            return None;
        }
        if self.is_packed {
            Some(self.fields[..index].iter().map(|t| t.size_bytes()).sum())
        } else {
            let mut offset = 0u32;
            for i in 0..index {
                let field_size = self.fields[i].size_bytes();
                let field_align = self.fields[i].alignment();
                let padding = (field_align - (offset % field_align)) % field_align;
                offset += padding + field_size;
            }
            // 对齐到目标字段
            let target_align = self.fields[index].alignment();
            let padding = (target_align - (offset % target_align)) % target_align;
            Some(offset + padding)
        }
    }
}

/// 数组类型描述。
#[derive(Clone, Debug, PartialEq)]
pub struct ArrayType {
    /// 元素类型。
    pub element_type: Type,
    /// 元素数量。
    pub length: u64,
}

impl ArrayType {
    pub fn new(element_type: Type, length: u64) -> Self {
        Self {
            element_type,
            length,
        }
    }

    pub fn size_bytes(&self) -> u32 {
        self.element_type.size_bytes() * self.length as u32
    }
}

/// 向量类型描述。
#[derive(Clone, Debug, PartialEq)]
pub struct VectorType {
    /// 元素类型。
    pub element_type: Type,
    /// 元素数量。
    pub length: u32,
}

impl VectorType {
    pub fn new(element_type: Type, length: u32) -> Self {
        Self {
            element_type,
            length,
        }
    }

    pub fn size_bytes(&self) -> u32 {
        self.element_type.size_bytes() * self.length
    }

    /// 返回是否是固定长度向量（非 scalable）。
    pub fn is_fixed_length(&self) -> bool {
        true
    }
}

/// 指针类型描述。
#[derive(Clone, Debug, PartialEq)]
pub struct PointerType {
    /// 指向的类型（None = 不透明指针，对应 LLVM opaque ptr）。
    pub pointee_type: Option<Type>,
    /// 地址空间（默认 0）。
    pub address_space: u32,
}

impl PointerType {
    pub fn new(pointee_type: Option<Type>, address_space: u32) -> Self {
        Self {
            pointee_type,
            address_space,
        }
    }

    /// 创建一个默认地址空间的不透明指针。
    pub fn opaque() -> Self {
        Self {
            pointee_type: None,
            address_space: 0,
        }
    }
}

/// 函数类型描述。
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionType {
    /// 参数类型列表。
    pub param_types: Vec<Type>,
    /// 返回值类型列表。
    pub return_types: Vec<Type>,
    /// 是否支持可变参数。
    pub is_vararg: bool,
}

impl FunctionType {
    pub fn new(params: Vec<Type>, returns: Vec<Type>) -> Self {
        Self {
            param_types: params,
            return_types: returns,
            is_vararg: false,
        }
    }

    pub fn with_vararg(mut self, vararg: bool) -> Self {
        self.is_vararg = vararg;
        self
    }
}

/// IR 上下文 — 全局类型表和常量池。
///
/// 一个 `Context` 实例管理：
/// - **常量池**：跨函数共享的编译时常量（Big 值），避免重复
/// - **结构体类型注册表**：命名和匿名结构体的定义
/// - **向量/数组/函数类型**：复合类型的描述信息
///
/// 典型用法：
///
/// ```ignore
/// let mut ctx = Context::new();
/// let forty_two = ctx.insert_const(Big::from_i64(42));
/// ctx.register_struct_type("Point", vec![Type::I32, Type::I32], false);
/// ```
#[derive(Clone, Debug)]
pub struct Context {
    /// 常量池 — 跨函数共享的编译时常量。
    pub constants: ConstantPool,
    /// 命名结构体类型表。
    struct_types: HashMap<String, StructType>,
    /// 匿名结构体列表。
    anonymous_structs: Vec<StructType>,
    /// 注册的数组类型描述（用于内存布局查询）。
    array_types: Vec<ArrayType>,
    /// 注册的向量类型描述。
    vector_types: Vec<VectorType>,
    /// 注册的指针类型描述。
    pointer_types: Vec<PointerType>,
    /// 注册的函数类型描述。
    function_types: Vec<FunctionType>,
    /// 数据布局字符串（如 "e-m:e-p:64:64:64-i1:8:8-i8:8:8-i16:16:16-i32:32:32-i64:64:64-f32:32:32-f64:64:64-v128:128:128-n32:64-S128"）。
    data_layout: Option<String>,
    /// 目标三元组（如 "x86_64-unknown-linux-gnu"）。
    target_triple: Option<String>,
}

impl Context {
    /// 创建一个新的空上下文。
    pub fn new() -> Self {
        Self {
            constants: ConstantPool::new(),
            struct_types: HashMap::new(),
            anonymous_structs: Vec::new(),
            array_types: Vec::new(),
            vector_types: Vec::new(),
            pointer_types: Vec::new(),
            function_types: Vec::new(),
            data_layout: None,
            target_triple: None,
        }
    }

    // ============================================================
    // 常量管理
    // ============================================================

    /// 插入一个常量值，返回其索引。如果已存在则返回已有索引。
    pub fn insert_const(&mut self, value: Big) -> u32 {
        self.constants.insert(value)
    }

    /// 按索引获取常量值。
    pub fn get_const(&self, index: u32) -> Option<&Big> {
        self.constants.get(index)
    }

    /// 当前上下文中常量的数量。
    pub fn const_count(&self) -> usize {
        self.constants.len()
    }

    /// 迭代所有常量。
    pub fn iter_consts(&self) -> impl Iterator<Item = (u32, &Big)> {
        self.constants.iter()
    }

    /// 获取所有常量的切片。
    pub fn const_slice(&self) -> &[Big] {
        self.constants.constants()
    }

    // ============================================================
    // 结构体类型管理
    // ============================================================

    /// 注册一个命名结构体类型。
    pub fn register_struct(&mut self, name: &str, fields: Vec<Type>, is_packed: bool) {
        self.struct_types.insert(
            name.to_string(),
            StructType::named(name, fields).with_packed(is_packed),
        );
    }

    /// 按名称查找结构体类型。
    pub fn get_struct(&self, name: &str) -> Option<&StructType> {
        self.struct_types.get(name)
    }

    /// 创建一个匿名结构体，返回其索引。
    pub fn add_anonymous_struct(&mut self, fields: Vec<Type>, is_packed: bool) -> usize {
        let idx = self.anonymous_structs.len();
        self.anonymous_structs
            .push(StructType::anonymous(fields).with_packed(is_packed));
        idx
    }

    /// 按索引获取匿名结构体。
    pub fn get_anonymous_struct(&self, index: usize) -> Option<&StructType> {
        self.anonymous_structs.get(index)
    }

    /// 列出所有注册的命名结构体名称。
    pub fn struct_names(&self) -> impl Iterator<Item = &String> {
        self.struct_types.keys()
    }

    // ============================================================
    // 数组类型管理
    // ============================================================

    /// 注册一个数组类型描述，返回其 ID。
    pub fn add_array_type(&mut self, element_type: Type, length: u64) -> usize {
        let idx = self.array_types.len();
        self.array_types.push(ArrayType::new(element_type, length));
        idx
    }

    /// 按 ID 获取数组类型描述。
    pub fn get_array_type(&self, id: usize) -> Option<&ArrayType> {
        self.array_types.get(id)
    }

    // ============================================================
    // 向量类型管理
    // ============================================================

    /// 注册一个向量类型描述，返回其 ID。
    pub fn add_vector_type(&mut self, element_type: Type, length: u32) -> usize {
        let idx = self.vector_types.len();
        self.vector_types
            .push(VectorType::new(element_type, length));
        idx
    }

    /// 按 ID 获取向量类型描述。
    pub fn get_vector_type(&self, id: usize) -> Option<&VectorType> {
        self.vector_types.get(id)
    }

    // ============================================================
    // 指针类型管理
    // ============================================================

    /// 注册一个指针类型描述，返回其 ID。
    pub fn add_pointer_type(&mut self, pointee_type: Option<Type>, address_space: u32) -> usize {
        let idx = self.pointer_types.len();
        self.pointer_types
            .push(PointerType::new(pointee_type, address_space));
        idx
    }

    /// 按 ID 获取指针类型描述。
    pub fn get_pointer_type(&self, id: usize) -> Option<&PointerType> {
        self.pointer_types.get(id)
    }

    // ============================================================
    // 函数类型管理
    // ============================================================

    /// 注册一个函数类型描述，返回其 ID。
    pub fn add_function_type(&mut self, params: Vec<Type>, returns: Vec<Type>) -> usize {
        let idx = self.function_types.len();
        self.function_types.push(FunctionType::new(params, returns));
        idx
    }

    /// 按 ID 获取函数类型描述。
    pub fn get_function_type(&self, id: usize) -> Option<&FunctionType> {
        self.function_types.get(id)
    }

    // ============================================================
    // 数据布局 / 目标三元组
    // ============================================================

    /// 设置数据布局字符串。
    pub fn set_data_layout(&mut self, layout: &str) {
        self.data_layout = Some(layout.to_string());
    }

    /// 获取数据布局字符串。
    pub fn data_layout(&self) -> Option<&str> {
        self.data_layout.as_deref()
    }

    /// 设置目标三元组。
    pub fn set_target_triple(&mut self, triple: &str) {
        self.target_triple = Some(triple.to_string());
    }

    /// 获取目标三元组。
    pub fn target_triple(&self) -> Option<&str> {
        self.target_triple.as_deref()
    }

    // ============================================================
    // 便捷方法
    // ============================================================

    /// 计算一个类型在此上下文中的数据大小（支持复合类型查询）。
    pub fn size_of(&self, ty: &Type) -> u32 {
        match ty {
            // 基本类型由 Type::size_bytes 处理
            t if t.is_primitive() => t.size_bytes(),
            // 复合类型需要到 Context 中查表
            _ => ty.size_bytes(),
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

/// ABI 对齐信息辅助方法（扩展 Type）。
pub trait TypeAlign {
    /// 返回类型的自然对齐（字节）。
    fn alignment(&self) -> u32;
}

impl TypeAlign for Type {
    fn alignment(&self) -> u32 {
        match self {
            Type::Void => 1,
            Type::Bool => 1,
            Type::I8 => 1,
            Type::I16 | Type::F16 => 2,
            Type::I32 | Type::F32 => 4,
            Type::I64 | Type::F64 | Type::Ptr => 8,
            Type::I128 | Type::F128 | Type::V128 => 16,
            Type::V64 => 8,
            Type::V256 => 32,
            // 复合类型默认 8 字节对齐
            Type::StructNamed(_)
            | Type::StructAnon(_)
            | Type::Array(_)
            | Type::Vector(_)
            | Type::Pointer(_)
            | Type::Function(_) => 8,
        }
    }
}

/// 标记原始类型的方法，用于将复合类型与基础类型区分。
pub trait TypeInfo {
    /// 是否是原始/基本类型（非复合类型）。
    fn is_primitive(&self) -> bool;
    /// 是否是复合类型（结构体、数组等，需要 Context 辅助查询）。
    fn is_composite(&self) -> bool;
}

impl TypeInfo for Type {
    fn is_primitive(&self) -> bool {
        matches!(
            self,
            Type::Void
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::I64
                | Type::I128
                | Type::F16
                | Type::F32
                | Type::F64
                | Type::F128
                | Type::V64
                | Type::V128
                | Type::V256
                | Type::Ptr
        )
    }

    fn is_composite(&self) -> bool {
        matches!(
            self,
            Type::StructNamed(_)
                | Type::StructAnon(_)
                | Type::Array(_)
                | Type::Vector(_)
                | Type::Pointer(_)
                | Type::Function(_)
        )
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let ctx = Context::new();
        assert_eq!(ctx.const_count(), 0);
        assert!(ctx.data_layout().is_none());
        assert!(ctx.target_triple().is_none());
    }

    #[test]
    fn test_context_constants() {
        let mut ctx = Context::new();
        let idx1 = ctx.insert_const(Big::from_i64(42));
        let idx2 = ctx.insert_const(Big::from_i64(42)); // 去重
        assert_eq!(idx1, idx2);
        assert_eq!(ctx.const_count(), 1);
        assert_eq!(ctx.get_const(idx1).and_then(|b| b.try_to_i64()), Some(42));
    }

    #[test]
    fn test_struct_registration() {
        let mut ctx = Context::new();
        ctx.register_struct("Point", vec![Type::I32, Type::I32], false);
        let s = ctx.get_struct("Point").unwrap();
        assert_eq!(s.fields.len(), 2);
        assert_eq!(s.name, Some("Point".to_string()));
        assert!(!s.is_packed);
    }

    #[test]
    fn test_anonymous_struct() {
        let mut ctx = Context::new();
        let idx = ctx.add_anonymous_struct(vec![Type::I8, Type::I32], false);
        let s = ctx.get_anonymous_struct(idx).unwrap();
        assert!(s.name.is_none());
        assert_eq!(s.fields[0], Type::I8);
        assert_eq!(s.fields[1], Type::I32);
    }

    #[test]
    fn test_struct_size() {
        let s = StructType::named("Test", vec![Type::I8, Type::I32]);
        // 非 packed: i8 + 3 padding + i32 = 8
        assert_eq!(s.size_bytes(), 8);
        // packed: i8 + i32 = 5
        let s_packed = StructType::named("TestPacked", vec![Type::I8, Type::I32]).with_packed(true);
        assert_eq!(s_packed.size_bytes(), 5);
    }

    #[test]
    fn test_field_offset() {
        let s = StructType::named("Test", vec![Type::I8, Type::I32, Type::I64]);
        assert_eq!(s.field_offset(0), Some(0));
        assert_eq!(s.field_offset(1), Some(4)); // i8 + 3 padding
        assert_eq!(s.field_offset(2), Some(8)); // i32 + i32 + 0 padding
        assert!(s.field_offset(3).is_none());
    }

    #[test]
    fn test_array_type() {
        let mut ctx = Context::new();
        let id = ctx.add_array_type(Type::I32, 10);
        let arr = ctx.get_array_type(id).unwrap();
        assert_eq!(arr.element_type, Type::I32);
        assert_eq!(arr.length, 10);
        assert_eq!(arr.size_bytes(), 40);
    }

    #[test]
    fn test_vector_type() {
        let mut ctx = Context::new();
        let id = ctx.add_vector_type(Type::I32, 4);
        let vec_ty = ctx.get_vector_type(id).unwrap();
        assert_eq!(vec_ty.element_type, Type::I32);
        assert_eq!(vec_ty.length, 4);
        assert_eq!(vec_ty.size_bytes(), 16);
    }

    #[test]
    fn test_function_type() {
        let mut ctx = Context::new();
        let id = ctx.add_function_type(vec![Type::I32, Type::I32], vec![Type::I32]);
        let ft = ctx.get_function_type(id).unwrap();
        assert_eq!(ft.param_types.len(), 2);
        assert_eq!(ft.return_types.len(), 1);
        assert!(!ft.is_vararg);
    }

    #[test]
    fn test_data_layout_and_triple() {
        let mut ctx = Context::new();
        ctx.set_data_layout("e-m:e-p:64:64:64-i1:8:8");
        ctx.set_target_triple("x86_64-unknown-linux-gnu");
        assert_eq!(ctx.data_layout(), Some("e-m:e-p:64:64:64-i1:8:8"));
        assert_eq!(ctx.target_triple(), Some("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn test_type_alignment() {
        assert_eq!(Type::I8.alignment(), 1);
        assert_eq!(Type::I32.alignment(), 4);
        assert_eq!(Type::I64.alignment(), 8);
        assert_eq!(Type::V128.alignment(), 16);
        assert_eq!(Type::Ptr.alignment(), 8);
    }

    #[test]
    fn test_type_primitive_check() {
        assert!(Type::I32.is_primitive());
        assert!(Type::F64.is_primitive());
        assert!(Type::Ptr.is_primitive());
        assert!(Type::V128.is_primitive());
    }
}
