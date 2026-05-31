//! Scan-result value types shared by the scanner layer.
//!
//! These are the intermediate representation the AST scanners (`rust_syn`)
//! produce and the orchestrator (`analyze`) consumes. They live in this leaf
//! module — depended on one-directionally by `analyze`, `scanner`, and
//! `rust_syn` — so those three modules don't form an import cycle.

use super::model::Field;

#[derive(Debug, Clone)]
pub struct LiveDependency {
    pub from_file: String,
    pub to_module: String,
}

/// A function/method call discovered in source code.
#[derive(Debug, Clone)]
pub struct CallInfo {
    /// Fully qualified caller (e.g. "Store::save_desired" or file-level "main")
    pub caller: String,
    /// Callee name (e.g. "save_state", "Vec::new")
    pub callee: String,
    /// Line number of the call site (1-based)
    pub line: usize,
}

/// A struct discovered in the source code with its fields.
#[derive(Debug, Clone)]
pub struct DiscoveredStruct {
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub fields: Vec<Field>,
    pub file_path: String,
    pub extends: Vec<String>,
    pub implements: Vec<String>,
}

/// A method discovered from an impl block.
#[derive(Debug, Clone)]
pub struct DiscoveredMethod {
    pub start_line: usize,
    pub end_line: usize,
    /// The type this impl block is for (e.g. "Store")
    pub owner: String,
    pub name: String,
    pub parameters: Vec<Field>,
    pub return_type: String,
    pub file_path: String,
    pub extends: Vec<String>,
    pub implements: Vec<String>,
}

/// An enum discovered in the source code with its variants.
#[derive(Debug, Clone)]
pub struct DiscoveredEnum {
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    /// Variants represented as Fields: name = variant ident, field_type = associated data.
    pub variants: Vec<Field>,
    pub file_path: String,
    pub extends: Vec<String>,
    pub implements: Vec<String>,
}

/// A trait discovered in the source code.
///
/// In idiomatic Rust, a `trait` is how a bounded context expresses a DDD *port*:
/// a repository, gateway, or domain-service contract that is depended upon
/// abstractly. The concrete `struct`s that `impl` the trait are its adapters,
/// linked back to it via `implements` AST edges.
#[derive(Debug, Clone)]
pub struct DiscoveredTrait {
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    /// Method signatures the trait declares (owner is the trait name).
    pub methods: Vec<DiscoveredMethod>,
    /// Supertraits / bounds, e.g. `trait Foo: Bar` → `["Bar"]`.
    pub supertraits: Vec<String>,
    pub file_path: String,
}

/// A free (module-level) function discovered in the AST.
///
/// Distinct from [`DiscoveredMethod`], which is always attached to an `impl` or
/// trait owner. Free functions are common in Rust for domain operations and
/// factories, so they are surfaced as `function` symbols in the graph.
#[derive(Debug, Clone)]
pub struct DiscoveredFunction {
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub file_path: String,
}

/// A module declaration discovered in the AST.
#[derive(Debug, Clone)]
pub struct DiscoveredModule {
    pub name: String,
    pub public: bool,
    pub file_path: String,
    pub extends: Vec<String>,
    pub implements: Vec<String>,
}

/// A compiler directive / attribute discovered on a source item or field.
///
/// Captured for items of **any** visibility (and for struct fields / enum
/// variants), independent of the public-API symbol scan, so directives like
/// `#[allow(dead_code)]` on private items are still indexed. `owner` is the
/// annotated symbol (`Owner::method`, `Owner.field`, or a bare name); `directive`
/// is the normalized text (`allow(dead_code)`, `cfg(feature = "x")`, `Debug`).
#[derive(Debug, Clone)]
pub struct DiscoveredDirective {
    pub owner: String,
    pub directive: String,
    pub line: usize,
}

/// Everything discovered in source files under a single bounded context's module path.
#[derive(Debug, Clone)]
pub struct ContextScan {
    pub context_name: String,
    pub module_path: String,
    pub structs: Vec<DiscoveredStruct>,
    pub enums: Vec<DiscoveredEnum>,
    pub methods: Vec<DiscoveredMethod>,
    pub modules: Vec<DiscoveredModule>,
}

/// Result type for scanning a source file: (structs, enums, methods, modules).
pub type ScanResult = (
    Vec<DiscoveredStruct>,
    Vec<DiscoveredEnum>,
    Vec<DiscoveredMethod>,
    Vec<DiscoveredModule>,
    Vec<DiscoveredTrait>,
);

/// Everything extracted from a single source file in **one** parse.
///
/// Produced by [`crate::domain::scanner::AstScanner::scan_source`]. Bundling
/// symbols, imports, and calls lets the caller walk each file once and parse it
/// once, instead of re-reading and re-parsing it for each extractor.
#[derive(Debug, Clone, Default)]
pub struct FileScan {
    pub structs: Vec<DiscoveredStruct>,
    pub enums: Vec<DiscoveredEnum>,
    pub methods: Vec<DiscoveredMethod>,
    pub modules: Vec<DiscoveredModule>,
    pub traits: Vec<DiscoveredTrait>,
    pub functions: Vec<DiscoveredFunction>,
    pub imports: Vec<LiveDependency>,
    pub calls: Vec<CallInfo>,
    /// Compiler directives on items/fields of any visibility (see
    /// [`DiscoveredDirective`]). Decoupled from the public-only symbol vectors.
    pub directives: Vec<DiscoveredDirective>,
}
