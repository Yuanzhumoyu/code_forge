//! Symbol infrastructure — linkage, visibility, comdat, TLS model.
//!
//! Provides the symbol-level attributes shared by functions and global
//! variables, modeled after LLVM's GlobalObject attributes.
//!
//! # Reference
//! - LLVM: `llvm::GlobalObject` with `LinkageTypes`, `VisibilityTypes`,
//!   `Comdat`, `ThreadLocalMode`, `UnnamedAddr`
//! - Cranelift: simpler model with just `Linkage::Local/Exported/Preemptible`

use super::imm_str::ImmStr;

// ============================================================
// Visibility
// ============================================================

/// Symbol visibility — controls whether a symbol is visible to other
/// dynamic shared objects (DSOs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Visibility {
    /// Default visibility — visible to all DSOs, may be preempted.
    #[default]
    Default,
    /// Hidden visibility — visible within the current DSO only.
    /// Enables more aggressive optimizations (inlining, constant propagation).
    Hidden,
    /// Protected visibility — visible to all DSOs but not preemptible.
    /// The symbol cannot be overridden by another DSO.
    Protected,
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Visibility::Default => write!(f, "default"),
            Visibility::Hidden => write!(f, "hidden"),
            Visibility::Protected => write!(f, "protected"),
        }
    }
}

// ============================================================
// Linkage
// ============================================================

/// Symbol linkage — controls how a symbol is resolved at link time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Linkage {
    /// External linkage — globally visible, may be referenced from other
    /// translation units. This is the default for normal functions/globals.
    #[default]
    External,
    /// Available externally — definition exists elsewhere, but this
    /// definition may be used for inlining/optimization. May be discarded.
    AvailableExternally,
    /// Link-once (Any) — merged with other definitions of the same name.
    /// The linker may choose any copy.
    LinkOnceAny,
    /// Link-once (ODR) — merged with other definitions under ODR guarantee.
    /// All definitions must be identical.
    LinkOnceODR,
    /// Weak (Any) — like LinkOnceAny but remains even if unreferenced.
    WeakAny,
    /// Weak (ODR) — like LinkOnceODR but remains even if unreferenced.
    WeakODR,
    /// Appending — appended to a section (e.g., `llvm.global_ctors`).
    Appending,
    /// Internal — module-local, like `static` in C. Not in symbol table.
    Internal,
    /// Private — like Internal but no name in symbol table at all.
    /// Used for stripped debug builds.
    Private,
    /// External weak — weak with external default (C `__attribute__((weak))`).
    ExternalWeak,
    /// Common — tentative definition (C `int x;` without initializer).
    Common,
}

impl Linkage {}

impl std::fmt::Display for Linkage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Linkage::External => "external",
            Linkage::AvailableExternally => "available_externally",
            Linkage::LinkOnceAny => "linkonce_any",
            Linkage::LinkOnceODR => "linkonce_odr",
            Linkage::WeakAny => "weak_any",
            Linkage::WeakODR => "weak_odr",
            Linkage::Appending => "appending",
            Linkage::Internal => "internal",
            Linkage::Private => "private",
            Linkage::ExternalWeak => "extern_weak",
            Linkage::Common => "common",
        };
        write!(f, "{}", s)
    }
}

// ============================================================
// DllStorageClass
// ============================================================

/// DLL storage class — controls import/export on Windows targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DllStorageClass {
    /// Not a DLL import/export.
    #[default]
    Default,
    /// Import from a DLL.
    DllImport,
    /// Export to a DLL.
    DllExport,
}

// ============================================================
// TlsModel
// ============================================================

/// Thread-local storage model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsModel {
    /// General dynamic — most general, works for all TLS access patterns.
    /// Slower, uses `__tls_get_addr`.
    GeneralDynamic,
    /// Local dynamic — optimized for access within the local DSO.
    LocalDynamic,
    /// Initial exec — the TLS block is allocated at load time.
    /// Faster, but the variable must be in the main executable.
    InitialExec,
    /// Local exec — the fastest model, for TLS in the main executable only.
    LocalExec,
}

// ============================================================
// ComdatKind
// ============================================================

/// Comdat selection kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComdatKind {
    /// Any — the linker may choose any copy.
    Any,
    /// Exact match — all copies must be identical (bitwise).
    ExactMatch,
    /// Largest — the linker chooses the largest copy.
    Largest,
    /// No duplicates — duplicates are an error.
    NoDuplicates,
    /// Same size — all copies must have the same size.
    SameSize,
}

// ============================================================
// ComdatId
// ============================================================

/// Handle to a comdat group in the Module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct ComdatId(pub u32);

// ============================================================
// Comdat
// ============================================================

/// A comdat group — allows the linker to deduplicate sections.
///
/// Used for C++ inline functions, template instantiations, and
/// other cases where multiple translation units may define the
/// same symbol.
#[derive(Clone, Debug)]
pub struct Comdat {
    /// Name of the comdat group.
    pub name: ImmStr,
    /// Selection kind.
    pub kind: ComdatKind,
}

// ============================================================
// SymbolInfo
// ============================================================

/// Symbol-level attributes shared by all global objects
/// (functions and global variables).
#[derive(Clone, Debug, Default)]
pub struct SymbolInfo {
    /// Linkage type.
    pub linkage: Linkage,
    /// Visibility.
    pub visibility: Visibility,
    /// DLL storage class (Windows only).
    pub dll_storage_class: DllStorageClass,
    /// Custom section name (if placed in a specific section).
    pub section: Option<ImmStr>,
    /// Comdat group reference.
    pub comdat: Option<ComdatId>,
    /// Thread-local storage model (if TLS).
    pub tls_model: Option<TlsModel>,
    /// The address of this symbol is not significant.
    /// Enables merging of identical symbols.
    pub unnamed_addr: bool,
    /// The symbol can be safely discarded if unused.
    pub can_discard: bool,
    /// `dso_local` — 地址不会跨 DSO 抢占（允许直接访问优化）。
    pub dso_local: bool,
}

impl SymbolInfo {
    /// Create a new default (external, default visibility) symbol.
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_info_default() {
        let info = SymbolInfo::default();
        assert_eq!(info.linkage, Linkage::External);
        assert_eq!(info.visibility, Visibility::Default);
        assert!(info.section.is_none());
        assert!(info.comdat.is_none());
        assert!(info.tls_model.is_none());
    }
}
