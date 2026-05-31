use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
#[cfg(test)]
use syn::spanned::Spanned;
use syn::visit::Visit;

use super::model::*;
use super::rust_syn::RustSynScanner;
use super::scanner::AstScanner;

// The scan-result value types (`CallInfo`, `FileScan`, `Discovered*`, …) live in
// the leaf `super::ast` module and are re-exported here for backward
// compatibility. Housing them in a leaf module breaks the import cycle that
// would otherwise form between `analyze`, `scanner`, and `rust_syn`.
pub use super::ast::*;

// ─── Live Import Extraction ────────────────────────────────────────────────


struct ImportVisitor {
    imports: Vec<String>,
}

impl<'ast> Visit<'ast> for ImportVisitor {
    fn visit_use_tree(&mut self, node: &'ast syn::UseTree) {
        fn extract_paths(tree: &syn::UseTree, prefix: &str) -> Vec<String> {
            fn join(prefix: &str, segment: &str) -> String {
                if prefix.is_empty() {
                    segment.to_string()
                } else {
                    format!("{prefix}::{segment}")
                }
            }

            match tree {
                syn::UseTree::Path(path) => {
                    extract_paths(&path.tree, &join(prefix, &path.ident.to_string()))
                }
                syn::UseTree::Name(name) => vec![join(prefix, &name.ident.to_string())],
                syn::UseTree::Rename(rename) => vec![join(prefix, &rename.ident.to_string())],
                syn::UseTree::Glob(_) => vec![join(prefix, "*")],
                syn::UseTree::Group(group) => {
                    let mut paths = vec![];
                    for item in &group.items {
                        paths.extend(extract_paths(item, prefix));
                    }
                    paths
                }
            }
        }
        self.imports.extend(extract_paths(node, ""));
    }
}

pub fn extract_live_dependencies(
    file_path: &Path,
    source_code: &str,
) -> Result<Vec<LiveDependency>> {
    let syntax_tree = syn::parse_file(source_code)
        .with_context(|| format!("Failed to parse rust file: {}", file_path.display()))?;

    let mut visitor = ImportVisitor { imports: vec![] };
    visitor.visit_file(&syntax_tree);

    let from_file = file_path.to_string_lossy().to_string();
    let deps = visitor
        .imports
        .into_iter()
        .map(|to_module| LiveDependency {
            from_file: from_file.clone(),
            to_module,
        })
        .collect();

    Ok(deps)
}

/// Return a scanner appropriate for the file's extension, or None if unsupported.
fn scanner_for_path(path: &Path) -> Option<Box<dyn AstScanner>> {
    match path.extension()?.to_str()? {
        "rs" => Some(Box::new(RustSynScanner)),
        _ => None,
    }
}

pub fn scan_workspace(workspace_root: &Path) -> Result<Vec<LiveDependency>> {
    let mut all_deps = Vec::new();

    for entry in ignore::WalkBuilder::new(workspace_root).build() {
        let entry = entry
            .with_context(|| format!("Failed to walk workspace: {}", workspace_root.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(scanner) = scanner_for_path(path) else {
            continue;
        };
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read source file: {}", path.display()))?;
        let deps = scanner
            .extract_live_dependencies(path, &content)
            .with_context(|| format!("Failed to extract imports from: {}", path.display()))?;
        all_deps.extend(deps);
    }

    Ok(all_deps)
}

// ─── Domain Structure Extraction ───────────────────────────────────────────


// ─── Inline Rust scanner (used by test suite only) ─────────────────────────

#[cfg(test)]
/// AST visitor that collects struct definitions, enums, modules, traits, and impl methods.
struct StructMethodVisitor {
    structs: Vec<DiscoveredStruct>,
    enums: Vec<DiscoveredEnum>,
    methods: Vec<DiscoveredMethod>,
    modules: Vec<DiscoveredModule>,
    traits: Vec<DiscoveredTrait>,
    file_path: String,
}

#[cfg(test)]
impl<'ast> Visit<'ast> for StructMethodVisitor {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        // Skip private, test, or #[cfg(test)] structs
        if !is_public(&node.vis) {
            return;
        }

        let name = node.ident.to_string();
        let fields = match &node.fields {
            syn::Fields::Named(named) => named
                .named
                .iter()
                .filter_map(|f| {
                    let field_name = f.ident.as_ref()?.to_string();
                    let field_type = type_to_string(&f.ty);
                    Some(Field {
                        name: field_name,
                        field_type,
                        required: !is_option_type(&f.ty),
                        description: String::new(),
                    })
                })
                .collect(),
            // Tuple structs / newtypes: keep positional inner types.
            syn::Fields::Unnamed(unnamed) => unnamed
                .unnamed
                .iter()
                .enumerate()
                .map(|(i, f)| Field {
                    name: i.to_string(),
                    field_type: type_to_string(&f.ty),
                    required: !is_option_type(&f.ty),
                    description: String::new(),
                })
                .collect(),
            syn::Fields::Unit => vec![],
        };

        self.structs.push(DiscoveredStruct {
            name,
            start_line: node.span().start().line,
            end_line: node.span().end().line,
            fields,
            file_path: self.file_path.clone(),
            extends: vec![],
            implements: vec![],
        });

        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if !is_public(&node.vis) || has_cfg_test(&node.attrs) {
            return;
        }

        let name = node.ident.to_string();
        let variants = node
            .variants
            .iter()
            .map(|v| {
                let variant_name = v.ident.to_string();
                let variant_type = match &v.fields {
                    syn::Fields::Unit => "()".to_string(),
                    syn::Fields::Unnamed(u) => {
                        let types: Vec<String> =
                            u.unnamed.iter().map(|f| type_to_string(&f.ty)).collect();
                        types.join(", ")
                    }
                    syn::Fields::Named(n) => {
                        let parts: Vec<String> = n
                            .named
                            .iter()
                            .filter_map(|f| {
                                let fname = f.ident.as_ref()?.to_string();
                                Some(format!("{}: {}", fname, type_to_string(&f.ty)))
                            })
                            .collect();
                        format!("{{ {} }}", parts.join(", "))
                    }
                };
                Field {
                    name: variant_name,
                    field_type: variant_type,
                    required: true,
                    description: String::new(),
                }
            })
            .collect();

        self.enums.push(DiscoveredEnum {
            name,
            start_line: node.span().start().line,
            end_line: node.span().end().line,
            variants,
            file_path: self.file_path.clone(),
            extends: vec![],
            implements: vec![],
        });

        syn::visit::visit_item_enum(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let name = node.ident.to_string();
        if name == "tests" || has_cfg_test(&node.attrs) {
            return;
        }
        self.modules.push(DiscoveredModule {
            name,
            public: is_public(&node.vis),
            file_path: self.file_path.clone(),
            extends: vec![],
            implements: vec![],
        });
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        if !is_public(&node.vis) || has_cfg_test(&node.attrs) {
            return;
        }
        let name = node.ident.to_string();
        let supertraits = node
            .supertraits
            .iter()
            .filter_map(|bound| match bound {
                syn::TypeParamBound::Trait(t) => Some(
                    t.path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::"),
                ),
                _ => None,
            })
            .collect();
        let methods = node
            .items
            .iter()
            .filter_map(|item| {
                let syn::TraitItem::Fn(method) = item else {
                    return None;
                };
                let return_type = match &method.sig.output {
                    syn::ReturnType::Default => String::new(),
                    syn::ReturnType::Type(_, ty) => type_to_string(ty),
                };
                Some(DiscoveredMethod {
                    owner: name.clone(),
                    name: method.sig.ident.to_string(),
                    start_line: method.sig.span().start().line,
                    end_line: method.sig.span().end().line,
                    parameters: vec![],
                    return_type,
                    file_path: self.file_path.clone(),
                    extends: vec![],
                    implements: vec![],
                })
            })
            .collect();
        self.traits.push(DiscoveredTrait {
            name,
            start_line: node.span().start().line,
            end_line: node.span().end().line,
            methods,
            supertraits,
            file_path: self.file_path.clone(),
        });
        syn::visit::visit_item_trait(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        // Only inherent impls, not trait impls
        if node.trait_.is_some() {
            return;
        }

        let owner = type_to_string(&node.self_ty);

        for item in &node.items {
            if let syn::ImplItem::Fn(method) = item {
                if !is_public(&method.vis) {
                    continue;
                }

                let name = method.sig.ident.to_string();
                let return_type = match &method.sig.output {
                    syn::ReturnType::Default => String::new(),
                    syn::ReturnType::Type(_, ty) => type_to_string(ty),
                };

                let parameters: Vec<Field> = method
                    .sig
                    .inputs
                    .iter()
                    .filter_map(|arg| match arg {
                        syn::FnArg::Typed(pat_type) => {
                            let param_name = match pat_type.pat.as_ref() {
                                syn::Pat::Ident(ident) => ident.ident.to_string(),
                                _ => return None,
                            };
                            let param_type = type_to_string(&pat_type.ty);
                            Some(Field {
                                name: param_name,
                                field_type: param_type,
                                required: true,
                                description: String::new(),
                            })
                        }
                        syn::FnArg::Receiver(_) => None, // skip &self
                    })
                    .collect();

                self.methods.push(DiscoveredMethod {
                    owner: owner.clone(),
                    name,
                    start_line: method.span().start().line,
                    end_line: method.span().end().line,
                    parameters,
                    return_type,
                    file_path: self.file_path.clone(),
                    extends: vec![],
                    implements: vec![],
                });
            }
        }

        syn::visit::visit_item_impl(self, node);
    }
}

/// Scan a single Rust source file and extract structs, enums, methods, and modules.
#[cfg(test)]
fn scan_file(file_path: &Path, source_code: &str) -> Result<ScanResult> {
    let syntax_tree = syn::parse_file(source_code)
        .with_context(|| format!("Failed to parse: {}", file_path.display()))?;

    let mut visitor = StructMethodVisitor {
        structs: vec![],
        enums: vec![],
        methods: vec![],
        modules: vec![],
        traits: vec![],
        file_path: file_path.to_string_lossy().to_string(),
    };
    visitor.visit_file(&syntax_tree);

    Ok((
        visitor.structs,
        visitor.enums,
        visitor.methods,
        visitor.modules,
        visitor.traits,
    ))
}

// ─── Crate Discovery ──────────────────────────────────────────────────────

/// A discovered crate root in the workspace.
#[derive(Debug, Clone)]
struct CrateSource {
    /// Name of the crate (from directory name)
    name: String,
    /// Absolute path to the crate's src/ directory
    src_dir: PathBuf,
}

/// Discover all crate source directories in the workspace.
///
/// Walks the workspace looking for `Cargo.toml` files with adjacent `src/`
/// directories. Respects `.gitignore` (skips `target/`, hidden dirs, etc.).
fn discover_crate_sources(workspace_root: &Path) -> Vec<CrateSource> {
    let mut sources = Vec::new();

    // Check the workspace root itself
    let root_cargo = workspace_root.join("Cargo.toml");
    let root_src = workspace_root.join("src");
    if root_cargo.exists() && root_src.is_dir() {
        let name = workspace_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "root".into());
        sources.push(CrateSource {
            name,
            src_dir: root_src.clone(),
        });
    }

    // Walk for workspace member crates (nested Cargo.toml files)
    for entry in ignore::WalkBuilder::new(workspace_root)
        .max_depth(Some(4))
        .build()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == "Cargo.toml") && path != root_cargo {
            let crate_dir = match path.parent() {
                Some(d) => d,
                None => continue,
            };
            let src = crate_dir.join("src");
            if src.is_dir() {
                let name = crate_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".into());
                sources.push(CrateSource { name, src_dir: src });
            }
        }
    }

    // Fallback: if no Cargo.toml was found but src/ exists, still scan it.
    // Covers non-Cargo Rust projects and test scenarios.
    if sources.is_empty() {
        let fallback_src = workspace_root.join("src");
        if fallback_src.is_dir() {
            let name = workspace_root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "root".into());
            sources.push(CrateSource {
                name,
                src_dir: fallback_src,
            });
        }
    }

    sources
}

// ─── Struct & Enum Classification ──────────────────────────────────────────


/// Classification of a discovered struct based on naming conventions and
/// structural heuristics. Used when no enrichment model is available or when a
/// struct is not declared in the enrichment model.
#[derive(Debug, Clone, PartialEq)]
enum StructKind {
    Entity,
    ValueObject,
    Service,
    Repository,
    Event,
}

/// Classify a struct via naming conventions first, then structural heuristics.
///
/// Priority order:
/// 1. Suffix-matched naming conventions (strongest signal)
/// 2. Structural shape (fields vs methods vs both)
fn classify_struct(name: &str, fields: &[Field], has_methods: bool) -> StructKind {
    let upper = name.to_uppercase();

    // ── Guard: ontology-definition structs are pure data models, not DDD roles ──
    // A struct literally named "Service", "Repository", or "DomainEvent" with
    // data fields (name, description, etc.) is a *model definition* for that
    // concept, not an instance of the DDD role itself.
    const ONTOLOGY_NAMES: &[&str] = &[
        "SERVICE",
        "REPOSITORY",
        "DOMAINEVENT",
        "EVENT",
        "ENTITY",
        "VALUEOBJECT",
        "AGGREGATE",
        "BOUNDEDCONTEXT",
    ];
    if ONTOLOGY_NAMES.contains(&upper.as_str()) {
        let has_name_field = fields.iter().any(|f| f.name == "name");
        if has_name_field {
            return StructKind::ValueObject;
        }
    }

    // ── Naming conventions (suffix-based) ──
    if upper.ends_with("REPOSITORY") || upper.ends_with("REPO") {
        return StructKind::Repository;
    }
    if upper.ends_with("SERVICE")
        || upper.ends_with("HANDLER")
        || upper.ends_with("USECASE")
        || upper.ends_with("INTERACTOR")
    {
        return StructKind::Service;
    }
    if upper.ends_with("EVENT")
        || upper.ends_with("CREATED")
        || upper.ends_with("UPDATED")
        || upper.ends_with("DELETED")
        || upper.ends_with("CHANGED")
        || upper.ends_with("OCCURRED")
    {
        return StructKind::Event;
    }

    // ── Structural heuristics ──
    // Fields that carry domain data (not framework wiring)
    let has_data_fields = fields.iter().any(|f| {
        !f.field_type.starts_with("Arc<")
            && !f.field_type.starts_with("Box<dyn")
            && !f.field_type.starts_with("Rc<")
            && !f.field_type.starts_with("&")
    });

    if !has_data_fields && has_methods {
        return StructKind::Service;
    }
    if has_data_fields && has_methods {
        return StructKind::Entity;
    }

    // Has fields, no public methods → pure data → ValueObject
    StructKind::ValueObject
}

/// Classify an enum via naming conventions first, then variant shape.
///
/// Enums are most commonly ValueObjects (status codes, type discriminators).
/// Event naming suffixes override to Event.
fn classify_enum(name: &str) -> StructKind {
    let upper = name.to_uppercase();

    if upper.ends_with("EVENT")
        || upper.ends_with("CREATED")
        || upper.ends_with("UPDATED")
        || upper.ends_with("DELETED")
        || upper.ends_with("CHANGED")
        || upper.ends_with("OCCURRED")
    {
        return StructKind::Event;
    }

    // Enums are natural value objects — closed set of named values
    StructKind::ValueObject
}

/// Infer an architectural layer for a bounded context from its module name,
/// using the same naming-convention approach as [`classify_struct`].
///
/// Clean / Hexagonal / Onion architectures all share a conventional vocabulary
/// for the rings (`domain`, `application`, `infrastructure`, presentation). When
/// a context directory follows that vocabulary we can assign its layer with no
/// hand-written policy. Names that match no convention return `None` so callers
/// leave them unassigned rather than guessing.
pub fn classify_layer(name: &str) -> Option<&'static str> {
    match name.to_lowercase().as_str() {
        "domain" | "core" | "model" | "models" | "entity" | "entities" | "domain_model" => {
            Some("domain")
        }
        "application" | "app" | "usecase" | "usecases" | "use_case" | "use_cases" | "service"
        | "services" | "handler" | "handlers" | "interactor" | "interactors" => Some("application"),
        "infrastructure" | "infra" | "adapter" | "adapters" | "persistence" | "repository"
        | "repositories" | "repo" | "repos" | "gateway" | "gateways" | "db" | "database"
        | "store" | "stores" => Some("infrastructure"),
        "api" | "web" | "http" | "rest" | "grpc" | "interface" | "interfaces" | "presentation"
        | "ui" | "controller" | "controllers" => Some("presentation"),
        _ => None,
    }
}

/// The typed DDD role a `trait` (port) plays, inferred from its name.
///
/// A trait *is* the Rust expression of a port; this maps it onto the existing
/// typed building blocks so a trait-defined repository/service shows up in the
/// typed model (and thus in `reconstruct_model`/export), not just as a `trait`
/// symbol. `None` keeps generic traits as ports in the symbol graph only.
enum PortRole {
    Repository,
    Service(ServiceKind),
    None,
}

fn classify_trait_port(name: &str) -> PortRole {
    let upper = name.to_uppercase();
    if upper.ends_with("REPOSITORY") || upper.ends_with("REPO") || upper.ends_with("STORE") {
        PortRole::Repository
    } else if upper.ends_with("GATEWAY") || upper.ends_with("CLIENT") {
        // Gateways/clients are infrastructure-facing ports.
        PortRole::Service(ServiceKind::Infrastructure)
    } else if upper.ends_with("SERVICE")
        || upper.ends_with("HANDLER")
        || upper.ends_with("USECASE")
        || upper.ends_with("INTERACTOR")
        || upper.ends_with("PORT")
    {
        PortRole::Service(ServiceKind::Domain)
    } else {
        PortRole::None
    }
}

/// Strip a repository-suffix to recover the aggregate a port manages
/// (`OrderRepository` → `Order`). Falls back to the full name.
fn aggregate_from_repo_name(name: &str) -> String {
    for suffix in ["Repository", "Repo", "Store"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            if !stripped.is_empty() {
                return stripped.to_string();
            }
        }
    }
    name.to_string()
}

// ─── Full Workspace Scan ───────────────────────────────────────────────────

/// Scan the entire workspace bottom-up, discovering ALL public structs and
/// methods across every crate's `src/` directory.
///
/// Bounded contexts are derived from the top-level module directories under
/// each `src/`. For multi-crate workspaces every crate is scanned.
///
/// When an enrichment model is provided it overlays explicit domain knowledge:
///   - Structs matching an enriched element inherit its classification, metadata
///     (description, invariants, aggregate_root, etc.).
///   - Structs NOT in the enrichment model are still discovered and classified
///     via naming conventions and structural heuristics.
///   - Model-only metadata (aggregates, policies, read_models, external
///     systems, architectural decisions, ownership, rules, etc.) is carried
///     forward into the actual model.
pub fn scan_actual_model(
    workspace_root: &Path,
    desired: Option<&DomainModel>,
) -> Result<DomainModel> {
    let project_name = desired.map(|d| d.name.clone()).unwrap_or_else(|| {
        workspace_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unnamed".into())
    });

    let mut actual = DomainModel {
        name: project_name,
        description: desired.map_or(String::new(), |d| d.description.clone()),
        bounded_contexts: vec![],
        external_systems: desired.map_or(vec![], |d| d.external_systems.clone()),
        architectural_decisions: desired.map_or(vec![], |d| d.architectural_decisions.clone()),
        ownership: desired.map_or(Ownership::default(), |d| d.ownership.clone()),
        rules: desired.map_or(vec![], |d| d.rules.clone()),
        tech_stack: desired.map_or(TechStack::default(), |d| d.tech_stack.clone()),
        conventions: desired.map_or(Conventions::default(), |d| d.conventions.clone()),
        ast_edges: vec![],
        source_files: vec![],
        symbols: vec![],
        import_edges: vec![],
        call_edges: vec![],
    };

    // 1. Discover all crate source directories
    let crate_sources = discover_crate_sources(workspace_root);
    let multi_crate = crate_sources.len() > 1;

    for crate_src in &crate_sources {
        // 2. Discover bounded contexts from top-level module directories
        let module_dirs: Vec<(String, PathBuf, String)> = {
            let mut contexts = Vec::new();

            let entries = std::fs::read_dir(&crate_src.src_dir).with_context(|| {
                format!(
                    "Failed to read crate source directory: {}",
                    crate_src.src_dir.display()
                )
            })?;
            for entry in entries {
                let entry = entry.with_context(|| {
                    format!(
                        "Failed to read entry from crate source directory: {}",
                        crate_src.src_dir.display()
                    )
                })?;
                let path = entry.path();
                if path.is_dir() {
                    let dir_name = match path.file_name() {
                        Some(n) => n.to_string_lossy().to_string(),
                        None => continue,
                    };
                    if dir_name.starts_with('.') {
                        continue;
                    }
                    let module_path = path
                        .strip_prefix(workspace_root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    let ctx_name = if multi_crate {
                        format!("{}::{}", crate_src.name, dir_name)
                    } else {
                        dir_name
                    };
                    contexts.push((ctx_name, path, module_path));
                }
            }

            // If src/ has no subdirectories, treat it as a single context
            if contexts.is_empty() {
                let module_path = crate_src
                    .src_dir
                    .strip_prefix(workspace_root)
                    .unwrap_or(&crate_src.src_dir)
                    .to_string_lossy()
                    .to_string();
                contexts.push((
                    crate_src.name.clone(),
                    crate_src.src_dir.clone(),
                    module_path,
                ));
            }

            contexts
        };

        // 3. For each discovered context, scan and classify
        // Collect per-context imports for dependency inference
        let mut context_imports: Vec<(String, Vec<LiveDependency>)> = Vec::new();

        for (ctx_name, scan_dir, module_path) in &module_dirs {
            // Single pass over the context's files: each file is read once and
            // parsed once, yielding symbols, imports, and calls together.
            let mut structs = Vec::new();
            let mut enums = Vec::new();
            let mut methods = Vec::new();
            let mut discovered_modules = Vec::new();
            let mut traits = Vec::new();
            let mut functions = Vec::new();
            let mut ctx_deps = Vec::new();

            if scan_dir.exists() {
                for entry in ignore::WalkBuilder::new(scan_dir).build() {
                    let entry = entry.with_context(|| {
                        format!("Failed to walk context directory: {}", scan_dir.display())
                    })?;
                    let path = entry.path();
                    if !path.is_file() {
                        continue;
                    }
                    let Some(scanner) = scanner_for_path(path) else {
                        continue;
                    };
                    let content = std::fs::read_to_string(path).with_context(|| {
                        format!("Failed to read source file: {}", path.display())
                    })?;

                    let rel_path = path
                        .strip_prefix(workspace_root)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .to_string();

                    let scan = scanner.scan_source(path, &content).with_context(|| {
                        format!("Failed to scan source file: {}", path.display())
                    })?;

                    actual.source_files.push(SourceFile {
                        path: rel_path.clone(),
                        context: ctx_name.clone(),
                        language: "rust".to_string(),
                    });

                    // Import edges
                    for dep in &scan.imports {
                        actual.import_edges.push(ImportEdge {
                            from_file: rel_path.clone(),
                            to_module: dep.to_module.clone(),
                            context: ctx_name.clone(),
                        });
                    }
                    ctx_deps.extend(scan.imports);

                    // Call edges
                    for ci in scan.calls {
                        actual.call_edges.push(CallEdge {
                            caller: ci.caller,
                            callee: ci.callee,
                            file_path: rel_path.clone(),
                            line: ci.line,
                            context: ctx_name.clone(),
                        });
                    }

                    // Directive edges (lints, cfg, derives) on items/fields of any
                    // visibility — the sole source of `decorators` edges, carrying
                    // the declaration location so a `to="dead_code"` query is
                    // actionable.
                    for d in scan.directives {
                        actual.ast_edges.push(crate::domain::model::ASTEdge {
                            from_node: d.owner,
                            to_node: d.directive,
                            edge_type: "decorators".into(),
                            file_path: rel_path.clone(),
                            line: d.line,
                        });
                    }

                    // Symbols
                    structs.extend(scan.structs);
                    enums.extend(scan.enums);
                    methods.extend(scan.methods);
                    discovered_modules.extend(scan.modules);
                    traits.extend(scan.traits);
                    functions.extend(scan.functions);
                }
            }
            context_imports.push((ctx_name.clone(), ctx_deps));

            // Resolve matching desired bounded context (by name or module_path)
            let desired_bc = desired.and_then(|d| {
                d.bounded_contexts.iter().find(|bc| {
                    bc.name.eq_ignore_ascii_case(ctx_name) || bc.module_path == *module_path
                })
            });

            let mut bc = BoundedContext {
                name: ctx_name.to_string(),
                description: desired_bc.map_or(String::new(), |b| b.description.clone()),
                module_path: module_path.clone(),
                ownership: desired_bc.map_or(Ownership::default(), |b| b.ownership.clone()),
                aggregates: desired_bc.map_or(vec![], |b| b.aggregates.clone()),
                policies: desired_bc.map_or(vec![], |b| b.policies.clone()),
                read_models: desired_bc.map_or(vec![], |b| b.read_models.clone()),
                entities: vec![],
                value_objects: vec![],
                services: vec![],
                repositories: vec![],
                events: vec![],
                modules: discovered_modules
                    .iter()
                    .map(|dm| {
                        let mod_path = format!("{}::{}", ctx_name, dm.name);
                        let desired_mod = desired_bc.and_then(|dbc| {
                            dbc.modules
                                .iter()
                                .find(|m| m.name.eq_ignore_ascii_case(&dm.name))
                        });
                        Module {
                            name: dm.name.clone(),
                            path: mod_path,
                            public: dm.public,
                            file_path: dm.file_path.clone(),
                            description: desired_mod
                                .map_or(String::new(), |m| m.description.clone()),
                        }
                    })
                    .collect(),
                dependencies: desired_bc.map_or(vec![], |b| b.dependencies.clone()),
                api_endpoints: desired_bc.map_or(vec![], |b| b.api_endpoints.clone()),
            };

            // Index methods by their owning type once, so collecting a struct's
            // methods is a hash lookup instead of a full scan per struct
            // (was O(structs × methods); now O(structs + methods)).
            let mut methods_by_owner: std::collections::HashMap<&str, Vec<&DiscoveredMethod>> =
                std::collections::HashMap::new();
            for m in &methods {
                methods_by_owner
                    .entry(m.owner.as_str())
                    .or_default()
                    .push(m);
            }

            for discovered in &structs {
                let name = &discovered.name;

                // Collect public methods for this struct from the owner index.
                let owned_methods: &[&DiscoveredMethod] = methods_by_owner
                    .get(name.as_str())
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let struct_methods: Vec<Method> = owned_methods
                    .iter()
                    .map(|m| Method {
                        name: m.name.clone(),
                        description: String::new(),
                        parameters: m.parameters.clone(),
                        return_type: m.return_type.clone(),
                        file_path: Some(m.file_path.clone()),
                        start_line: Some(m.start_line),
                        end_line: Some(m.end_line),
                    })
                    .collect();

                // Check if the enrichment model provides an explicit classification.
                let desired_kind = desired_bc.and_then(|dbc| {
                    if dbc
                        .entities
                        .iter()
                        .any(|e| e.name.eq_ignore_ascii_case(name))
                    {
                        Some(StructKind::Entity)
                    } else if dbc
                        .value_objects
                        .iter()
                        .any(|v| v.name.eq_ignore_ascii_case(name))
                    {
                        Some(StructKind::ValueObject)
                    } else if dbc
                        .services
                        .iter()
                        .any(|s| s.name.eq_ignore_ascii_case(name))
                    {
                        Some(StructKind::Service)
                    } else if dbc
                        .repositories
                        .iter()
                        .any(|r| r.name.eq_ignore_ascii_case(name))
                    {
                        Some(StructKind::Repository)
                    } else if dbc.events.iter().any(|e| e.name.eq_ignore_ascii_case(name)) {
                        Some(StructKind::Event)
                    } else {
                        None
                    }
                });

                // Use desired classification when available, otherwise heuristic
                let kind = desired_kind.unwrap_or_else(|| {
                    classify_struct(name, &discovered.fields, !owned_methods.is_empty())
                });

                match kind {
                    StructKind::Entity => {
                        let desired_ent = desired_bc.and_then(|dbc| {
                            dbc.entities
                                .iter()
                                .find(|e| e.name.eq_ignore_ascii_case(name))
                        });
                        bc.entities.push(Entity {
                            name: name.clone(),
                            description: desired_ent
                                .map_or(String::new(), |e| e.description.clone()),
                            aggregate_root: desired_ent.is_some_and(|e| e.aggregate_root),
                            fields: discovered.fields.clone(),
                            methods: struct_methods,
                            invariants: desired_ent.map_or(vec![], |e| e.invariants.clone()),
                            file_path: Some(discovered.file_path.clone()),
                            start_line: Some(discovered.start_line),
                            end_line: Some(discovered.end_line),
                        });
                    }
                    StructKind::ValueObject => {
                        let desired_vo = desired_bc.and_then(|dbc| {
                            dbc.value_objects
                                .iter()
                                .find(|v| v.name.eq_ignore_ascii_case(name))
                        });
                        bc.value_objects.push(ValueObject {
                            name: name.clone(),
                            description: desired_vo
                                .map_or(String::new(), |v| v.description.clone()),
                            fields: discovered.fields.clone(),
                            validation_rules: desired_vo
                                .map_or(vec![], |v| v.validation_rules.clone()),
                            file_path: Some(discovered.file_path.clone()),
                            start_line: Some(discovered.start_line),
                            end_line: Some(discovered.end_line),
                        });
                    }
                    StructKind::Service => {
                        let desired_svc = desired_bc.and_then(|dbc| {
                            dbc.services
                                .iter()
                                .find(|s| s.name.eq_ignore_ascii_case(name))
                        });
                        bc.services.push(Service {
                            name: name.clone(),
                            description: desired_svc
                                .map_or(String::new(), |s| s.description.clone()),
                            kind: desired_svc.map_or(ServiceKind::Domain, |s| s.kind.clone()),
                            methods: struct_methods,
                            dependencies: desired_svc.map_or(vec![], |s| s.dependencies.clone()),
                            file_path: Some(discovered.file_path.clone()),
                            start_line: Some(discovered.start_line),
                            end_line: Some(discovered.end_line),
                        });
                    }
                    StructKind::Repository => {
                        let desired_repo = desired_bc.and_then(|dbc| {
                            dbc.repositories
                                .iter()
                                .find(|r| r.name.eq_ignore_ascii_case(name))
                        });
                        bc.repositories.push(Repository {
                            name: name.clone(),
                            aggregate: desired_repo.map_or(String::new(), |r| r.aggregate.clone()),
                            methods: struct_methods,
                            file_path: Some(discovered.file_path.clone()),
                            start_line: Some(discovered.start_line),
                            end_line: Some(discovered.end_line),
                        });
                    }
                    StructKind::Event => {
                        let desired_evt = desired_bc.and_then(|dbc| {
                            dbc.events
                                .iter()
                                .find(|e| e.name.eq_ignore_ascii_case(name))
                        });
                        bc.events.push(DomainEvent {
                            name: name.clone(),
                            description: desired_evt
                                .map_or(String::new(), |e| e.description.clone()),
                            fields: discovered.fields.clone(),
                            source: desired_evt.map_or(String::new(), |e| e.source.clone()),
                            file_path: Some(discovered.file_path.clone()),
                            start_line: Some(discovered.start_line),
                            end_line: Some(discovered.end_line),
                        });
                    }
                }
            }

            // ── Classify discovered enums ──
            for discovered in &enums {
                let name = &discovered.name;

                // Collect public methods for this enum (from impl blocks)
                let enum_methods: Vec<Method> = methods
                    .iter()
                    .filter(|m| m.owner == *name)
                    .map(|m| Method {
                        name: m.name.clone(),
                        description: String::new(),
                        parameters: m.parameters.clone(),
                        return_type: m.return_type.clone(),
                        file_path: Some(m.file_path.clone()),
                        start_line: Some(m.start_line),
                        end_line: Some(m.end_line),
                    })
                    .collect();

                // Check the enrichment model for explicit classification.
                let desired_kind = desired_bc.and_then(|dbc| {
                    if dbc
                        .entities
                        .iter()
                        .any(|e| e.name.eq_ignore_ascii_case(name))
                    {
                        Some(StructKind::Entity)
                    } else if dbc
                        .value_objects
                        .iter()
                        .any(|v| v.name.eq_ignore_ascii_case(name))
                    {
                        Some(StructKind::ValueObject)
                    } else if dbc
                        .services
                        .iter()
                        .any(|s| s.name.eq_ignore_ascii_case(name))
                    {
                        Some(StructKind::Service)
                    } else if dbc.events.iter().any(|e| e.name.eq_ignore_ascii_case(name)) {
                        Some(StructKind::Event)
                    } else {
                        None
                    }
                });

                let kind = desired_kind.unwrap_or_else(|| classify_enum(name));

                match kind {
                    StructKind::Entity => {
                        let desired_ent = desired_bc.and_then(|dbc| {
                            dbc.entities
                                .iter()
                                .find(|e| e.name.eq_ignore_ascii_case(name))
                        });
                        bc.entities.push(Entity {
                            name: name.clone(),
                            description: desired_ent
                                .map_or(String::new(), |e| e.description.clone()),
                            aggregate_root: desired_ent.is_some_and(|e| e.aggregate_root),
                            fields: discovered.variants.clone(),
                            methods: enum_methods,
                            invariants: desired_ent.map_or(vec![], |e| e.invariants.clone()),
                            file_path: Some(discovered.file_path.clone()),
                            start_line: Some(discovered.start_line),
                            end_line: Some(discovered.end_line),
                        });
                    }
                    StructKind::ValueObject => {
                        let desired_vo = desired_bc.and_then(|dbc| {
                            dbc.value_objects
                                .iter()
                                .find(|v| v.name.eq_ignore_ascii_case(name))
                        });
                        bc.value_objects.push(ValueObject {
                            name: name.clone(),
                            description: desired_vo
                                .map_or(String::new(), |v| v.description.clone()),
                            fields: discovered.variants.clone(),
                            validation_rules: desired_vo
                                .map_or(vec![], |v| v.validation_rules.clone()),
                            file_path: Some(discovered.file_path.clone()),
                            start_line: Some(discovered.start_line),
                            end_line: Some(discovered.end_line),
                        });
                    }
                    StructKind::Service => {
                        let desired_svc = desired_bc.and_then(|dbc| {
                            dbc.services
                                .iter()
                                .find(|s| s.name.eq_ignore_ascii_case(name))
                        });
                        bc.services.push(Service {
                            name: name.clone(),
                            description: desired_svc
                                .map_or(String::new(), |s| s.description.clone()),
                            kind: desired_svc.map_or(ServiceKind::Domain, |s| s.kind.clone()),
                            methods: enum_methods,
                            dependencies: desired_svc.map_or(vec![], |s| s.dependencies.clone()),
                            file_path: Some(discovered.file_path.clone()),
                            start_line: Some(discovered.start_line),
                            end_line: Some(discovered.end_line),
                        });
                    }
                    StructKind::Repository => {
                        let desired_repo = desired_bc.and_then(|dbc| {
                            dbc.repositories
                                .iter()
                                .find(|r| r.name.eq_ignore_ascii_case(name))
                        });
                        bc.repositories.push(Repository {
                            name: name.clone(),
                            aggregate: desired_repo.map_or(String::new(), |r| r.aggregate.clone()),
                            methods: enum_methods,
                            file_path: Some(discovered.file_path.clone()),
                            start_line: Some(discovered.start_line),
                            end_line: Some(discovered.end_line),
                        });
                    }
                    StructKind::Event => {
                        let desired_evt = desired_bc.and_then(|dbc| {
                            dbc.events
                                .iter()
                                .find(|e| e.name.eq_ignore_ascii_case(name))
                        });
                        bc.events.push(DomainEvent {
                            name: name.clone(),
                            description: desired_evt
                                .map_or(String::new(), |e| e.description.clone()),
                            fields: discovered.variants.clone(),
                            source: desired_evt.map_or(String::new(), |e| e.source.clone()),
                            file_path: Some(discovered.file_path.clone()),
                            start_line: Some(discovered.start_line),
                            end_line: Some(discovered.end_line),
                        });
                    }
                }
            }

            // Collect symbols from discovered structs, enums, and methods
            for s in &structs {
                let rel_path = std::path::Path::new(&s.file_path)
                    .strip_prefix(workspace_root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| s.file_path.clone());
                actual.symbols.push(SymbolDef {
                    name: s.name.clone(),
                    kind: "struct".to_string(),
                    context: ctx_name.clone(),
                    file_path: rel_path,
                    start_line: s.start_line,
                    end_line: s.end_line,
                    visibility: "public".to_string(),
                });
            }
            for e in &enums {
                let rel_path = std::path::Path::new(&e.file_path)
                    .strip_prefix(workspace_root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| e.file_path.clone());
                actual.symbols.push(SymbolDef {
                    name: e.name.clone(),
                    kind: "enum".to_string(),
                    context: ctx_name.clone(),
                    file_path: rel_path,
                    start_line: e.start_line,
                    end_line: e.end_line,
                    visibility: "public".to_string(),
                });
            }
            for m in &methods {
                let rel_path = std::path::Path::new(&m.file_path)
                    .strip_prefix(workspace_root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| m.file_path.clone());
                actual.symbols.push(SymbolDef {
                    name: format!("{}::{}", m.owner, m.name),
                    kind: "method".to_string(),
                    context: ctx_name.clone(),
                    file_path: rel_path,
                    start_line: m.start_line,
                    end_line: m.end_line,
                    visibility: "public".to_string(),
                });
            }
            // Traits are Rust's expression of DDD ports (repository / gateway /
            // domain-service interfaces). Surface each trait as a first-class
            // `trait` symbol and its declared operations as `method` symbols, so
            // the graph carries the abstraction boundary, not just the adapters.
            for t in &traits {
                let rel_path = std::path::Path::new(&t.file_path)
                    .strip_prefix(workspace_root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| t.file_path.clone());
                actual.symbols.push(SymbolDef {
                    name: t.name.clone(),
                    kind: "trait".to_string(),
                    context: ctx_name.clone(),
                    file_path: rel_path.clone(),
                    start_line: t.start_line,
                    end_line: t.end_line,
                    visibility: "public".to_string(),
                });
                for m in &t.methods {
                    actual.symbols.push(SymbolDef {
                        name: format!("{}::{}", m.owner, m.name),
                        kind: "method".to_string(),
                        context: ctx_name.clone(),
                        file_path: rel_path.clone(),
                        start_line: m.start_line,
                        end_line: m.end_line,
                        visibility: "public".to_string(),
                    });
                }
                // Supertrait bounds become `implements` edges (port → supertrait),
                // mirroring how adapter structs link to the traits they implement.
                for sup in &t.supertraits {
                    actual.ast_edges.push(crate::domain::model::ASTEdge {
                        from_node: t.name.clone(),
                        to_node: sup.clone(),
                        edge_type: "implements".to_string(),
                        file_path: rel_path.clone(),
                        line: t.start_line,
                    });
                }

                // Fold trait-defined ports into the typed building blocks so the
                // reconstructed model sees them as repositories/services, reusing
                // those vectors' existing persistence. Dedup by name so a port
                // and a like-named struct adapter aren't double-listed.
                let port_methods = || -> Vec<Method> {
                    t.methods
                        .iter()
                        .map(|m| Method {
                            name: m.name.clone(),
                            description: String::new(),
                            parameters: m.parameters.clone(),
                            return_type: m.return_type.clone(),
                            file_path: Some(rel_path.clone()),
                            start_line: Some(m.start_line),
                            end_line: Some(m.end_line),
                        })
                        .collect()
                };
                match classify_trait_port(&t.name) {
                    PortRole::Repository => {
                        if !bc
                            .repositories
                            .iter()
                            .any(|r| r.name.eq_ignore_ascii_case(&t.name))
                        {
                            bc.repositories.push(Repository {
                                name: t.name.clone(),
                                aggregate: aggregate_from_repo_name(&t.name),
                                methods: port_methods(),
                                file_path: Some(rel_path.clone()),
                                start_line: Some(t.start_line),
                                end_line: Some(t.end_line),
                            });
                        }
                    }
                    PortRole::Service(kind) => {
                        if !bc
                            .services
                            .iter()
                            .any(|s| s.name.eq_ignore_ascii_case(&t.name))
                        {
                            bc.services.push(Service {
                                name: t.name.clone(),
                                description: String::new(),
                                kind,
                                methods: port_methods(),
                                dependencies: vec![],
                                file_path: Some(rel_path.clone()),
                                start_line: Some(t.start_line),
                                end_line: Some(t.end_line),
                            });
                        }
                    }
                    PortRole::None => {}
                }
            }

            // Free functions are surfaced as `function` symbols (domain
            // operations / factories that live outside any impl or trait).
            for f in &functions {
                let rel_path = std::path::Path::new(&f.file_path)
                    .strip_prefix(workspace_root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| f.file_path.clone());
                actual.symbols.push(SymbolDef {
                    name: f.name.clone(),
                    kind: "function".to_string(),
                    context: ctx_name.clone(),
                    file_path: rel_path,
                    start_line: f.start_line,
                    end_line: f.end_line,
                    visibility: "public".to_string(),
                });
            }

            actual.bounded_contexts.push(bc);

            // Harvest structural AST edges (extends / implements) reported by the
            // Rust scanner. Directive (`decorators`) edges are emitted per-file
            // above from `scan.directives`, so they carry a location and cover
            // items of any visibility — they are not re-derived here.
            for s in &structs {
                let rel_path = std::path::Path::new(&s.file_path)
                    .strip_prefix(workspace_root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| s.file_path.clone());
                for ext in &s.extends {
                    actual.ast_edges.push(crate::domain::model::ASTEdge {
                        from_node: s.name.clone(),
                        to_node: ext.clone(),
                        edge_type: "extends".into(),
                        file_path: rel_path.clone(),
                        line: s.start_line,
                    });
                }
                for imp in &s.implements {
                    actual.ast_edges.push(crate::domain::model::ASTEdge {
                        from_node: s.name.clone(),
                        to_node: imp.clone(),
                        edge_type: "implements".into(),
                        file_path: rel_path.clone(),
                        line: s.start_line,
                    });
                }
            }
            for e in &enums {
                let rel_path = std::path::Path::new(&e.file_path)
                    .strip_prefix(workspace_root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| e.file_path.clone());
                for ext in &e.extends {
                    actual.ast_edges.push(crate::domain::model::ASTEdge {
                        from_node: e.name.clone(),
                        to_node: ext.clone(),
                        edge_type: "extends".into(),
                        file_path: rel_path.clone(),
                        line: e.start_line,
                    });
                }
                for imp in &e.implements {
                    actual.ast_edges.push(crate::domain::model::ASTEdge {
                        from_node: e.name.clone(),
                        to_node: imp.clone(),
                        edge_type: "implements".into(),
                        file_path: rel_path.clone(),
                        line: e.start_line,
                    });
                }
            }
        }

        // ── Infer context dependencies from collected imports ──────────────
        // Lowercased context name → canonical name, so resolving an import's
        // first segment is an O(1) lookup instead of two linear scans per import
        // (was O(imports × contexts); now O(imports)).
        let ctx_by_lower: std::collections::HashMap<String, String> = actual
            .bounded_contexts
            .iter()
            .map(|bc| (bc.name.to_ascii_lowercase(), bc.name.clone()))
            .collect();

        for (ctx_name, imports) in &context_imports {
            let ctx_lower = ctx_name.to_ascii_lowercase();
            let mut inferred_deps: Vec<String> = Vec::new();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for dep in imports {
                // Extract the first meaningful module segment from the Rust import path.
                let raw = &dep.to_module;

                // Rust: strip crate::/super:: prefix, split on ::
                let first_segment = if let Some(stripped) = raw
                    .strip_prefix("crate::")
                    .or_else(|| raw.strip_prefix("super::"))
                {
                    stripped.split("::").next().unwrap_or("")
                } else {
                    raw.split("::").next().unwrap_or(raw)
                };

                if first_segment.is_empty() {
                    continue;
                }
                let seg_lower = first_segment.to_ascii_lowercase();
                if seg_lower == ctx_lower {
                    continue; // skip self-references
                }
                // Map to a known context name, deduping via `seen`.
                let Some(canonical) = ctx_by_lower.get(&seg_lower) else {
                    continue;
                };
                if seen.insert(seg_lower) {
                    inferred_deps.push(canonical.clone());
                }
            }

            if !inferred_deps.is_empty() {
                if let Some(bc) = actual
                    .bounded_contexts
                    .iter_mut()
                    .find(|bc| bc.name == *ctx_name)
                {
                    // Merge: keep desired deps, add inferred ones that aren't already present
                    for dep in inferred_deps {
                        if !bc.dependencies.iter().any(|d| d.eq_ignore_ascii_case(&dep)) {
                            bc.dependencies.push(dep);
                        }
                    }
                }
            }
        }

        // ── Infer event sources from entities in the same context ─────────
        for bc in &mut actual.bounded_contexts {
            let entity_names: Vec<String> = bc.entities.iter().map(|e| e.name.clone()).collect();
            for event in &mut bc.events {
                if event.source.is_empty() {
                    // Try to match by naming convention: "UserCreatedEvent" → "User"
                    let event_upper = event.name.to_uppercase();
                    if let Some(entity) = entity_names.iter().find(|e| {
                        let prefix = e.to_uppercase();
                        event_upper.starts_with(&prefix) && event_upper.len() > prefix.len()
                    }) {
                        event.source = entity.clone();
                    } else if entity_names.len() == 1 {
                        // Single entity in context → likely the source
                        event.source = entity_names[0].clone();
                    } else if let Some(root) = bc.entities.iter().find(|e| e.aggregate_root) {
                        // Fall back to aggregate root
                        event.source = root.name.clone();
                    }
                }
            }
        }
    }

    Ok(actual)
}

// ─── Helpers (test-only: used by inline StructMethodVisitor) ───────────────

#[cfg(test)]
fn is_public(vis: &syn::Visibility) -> bool {
    matches!(vis, syn::Visibility::Public(_))
}

#[cfg(test)]
fn has_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        attr.parse_args::<syn::Ident>()
            .map(|ident| ident == "test")
            .unwrap_or(false)
    })
}

/// Convert a syn::Type to a readable string.
#[cfg(test)]
fn type_to_string(ty: &syn::Type) -> String {
    // Manual conversion without depending on `quote` crate
    match ty {
        syn::Type::Path(type_path) => {
            let segments: Vec<String> = type_path
                .path
                .segments
                .iter()
                .map(|seg| {
                    let ident = seg.ident.to_string();
                    match &seg.arguments {
                        syn::PathArguments::None => ident,
                        syn::PathArguments::AngleBracketed(args) => {
                            let inner: Vec<String> = args
                                .args
                                .iter()
                                .filter_map(|a| match a {
                                    syn::GenericArgument::Type(t) => Some(type_to_string(t)),
                                    _ => None,
                                })
                                .collect();
                            if inner.is_empty() {
                                ident
                            } else {
                                format!("{}<{}>", ident, inner.join(","))
                            }
                        }
                        syn::PathArguments::Parenthesized(args) => {
                            let inputs: Vec<String> =
                                args.inputs.iter().map(type_to_string).collect();
                            format!("{}({})", ident, inputs.join(","))
                        }
                    }
                })
                .collect();
            segments.join("::")
        }
        syn::Type::Reference(r) => {
            let mutability = if r.mutability.is_some() { "&mut " } else { "&" };
            format!("{}{}", mutability, type_to_string(&r.elem))
        }
        syn::Type::Slice(s) => format!("[{}]", type_to_string(&s.elem)),
        syn::Type::Array(a) => format!("[{}; _]", type_to_string(&a.elem)),
        syn::Type::Tuple(t) => {
            let elems: Vec<String> = t.elems.iter().map(type_to_string).collect();
            format!("({})", elems.join(","))
        }
        _ => "?".to_string(),
    }
}

/// Check whether a type is Option<T>.
#[cfg(test)]
fn is_option_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(path) = ty
        && let Some(segment) = path.path.segments.last()
    {
        return segment.ident == "Option";
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_live_deps() {
        let code = r#"
            use std::path::Path;
            use crate::domain::model::DomainModel;
        "#;
        let deps = extract_live_dependencies(Path::new("test.rs"), code).unwrap();
        let modules: Vec<&str> = deps.iter().map(|dep| dep.to_module.as_str()).collect();
        assert_eq!(
            modules,
            vec!["std::path::Path", "crate::domain::model::DomainModel"]
        );
    }

    #[test]
    fn test_scan_file_struct_fields() {
        let code = r#"
            pub struct User {
                pub name: String,
                pub email: Option<String>,
                pub age: u32,
            }
        "#;
        let (structs, _, _, _, _) = scan_file(Path::new("test.rs"), code).unwrap();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "User");
        assert_eq!(structs[0].fields.len(), 3);
        assert!(structs[0].fields[0].required); // name: String
        assert!(!structs[0].fields[1].required); // email: Option<String>
        assert!(structs[0].fields[2].required); // age: u32
    }

    #[test]
    fn test_scan_file_impl_methods() {
        let code = r#"
            pub struct Store {}
            impl Store {
                pub fn open(path: &Path) -> Result<Self> { todo!() }
                pub fn save(&self, name: &str, data: &[u8]) -> Result<()> { todo!() }
                fn private_helper(&self) {} // should be ignored
            }
        "#;
        let (structs, _, methods, _, _) = scan_file(Path::new("test.rs"), code).unwrap();
        assert_eq!(structs.len(), 1);
        assert_eq!(methods.len(), 2); // only public methods
        assert_eq!(methods[0].owner, "Store");
        assert_eq!(methods[0].name, "open");
        assert_eq!(methods[0].parameters.len(), 1); // &self excluded
        assert_eq!(methods[1].name, "save");
        assert_eq!(methods[1].parameters.len(), 2);
    }

    #[test]
    fn test_scan_file_skips_private_structs() {
        let code = r#"
            struct PrivateStruct { x: i32 }
            pub struct PublicStruct { pub y: i32 }
        "#;
        let (structs, _, _, _, _) = scan_file(Path::new("test.rs"), code).unwrap();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "PublicStruct");
    }

    #[test]
    fn test_scan_file_skips_trait_impls() {
        let code = r#"
            pub struct Foo {}
            impl Foo {
                pub fn bar(&self) {}
            }
            impl std::fmt::Display for Foo {
                fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    write!(f, "foo")
                }
            }
        "#;
        let (_, _, methods, _, _) = scan_file(Path::new("test.rs"), code).unwrap();
        assert_eq!(methods.len(), 1); // only inherent impl
        assert_eq!(methods[0].name, "bar");
    }

    #[test]
    fn test_classify_struct_naming_conventions() {
        // Repository suffix
        assert_eq!(
            classify_struct("UserRepository", &[], false),
            StructKind::Repository,
        );
        assert_eq!(
            classify_struct("OrderRepo", &[], false),
            StructKind::Repository,
        );

        // Service suffix
        assert_eq!(
            classify_struct("PaymentService", &[], false),
            StructKind::Service,
        );
        assert_eq!(
            classify_struct("AuthHandler", &[], false),
            StructKind::Service,
        );

        // Event suffix
        assert_eq!(
            classify_struct("OrderCreated", &[], false),
            StructKind::Event,
        );
        assert_eq!(
            classify_struct("UserDeletedEvent", &[], false),
            StructKind::Event,
        );
    }

    #[test]
    fn test_classify_struct_structural_heuristics() {
        let data_fields = vec![Field {
            name: "name".into(),
            field_type: "String".into(),
            required: true,
            description: String::new(),
        }];
        let dep_fields = vec![Field {
            name: "store".into(),
            field_type: "Arc<Store>".into(),
            required: true,
            description: String::new(),
        }];
        let methods = vec![DiscoveredMethod {
            owner: "Foo".into(),
            name: "do_thing".into(),
            parameters: vec![],
            return_type: String::new(),
            file_path: String::new(),
            start_line: 0,
            end_line: 0,
            extends: vec![],
            implements: vec![],
        }];

        // Data fields + methods → Entity
        assert_eq!(
            classify_struct("Foo", &data_fields, true),
            StructKind::Entity,
        );

        // Data fields, no methods → ValueObject
        assert_eq!(
            classify_struct("Foo", &data_fields, false),
            StructKind::ValueObject,
        );

        // Only dependency fields + methods → Service
        assert_eq!(
            classify_struct("Foo", &dep_fields, true),
            StructKind::Service,
        );
    }

    #[test]
    fn test_classify_layer_naming_conventions() {
        assert_eq!(classify_layer("domain"), Some("domain"));
        assert_eq!(classify_layer("Core"), Some("domain"));
        assert_eq!(classify_layer("application"), Some("application"));
        assert_eq!(classify_layer("services"), Some("application"));
        assert_eq!(classify_layer("infrastructure"), Some("infrastructure"));
        assert_eq!(classify_layer("adapters"), Some("infrastructure"));
        assert_eq!(classify_layer("store"), Some("infrastructure"));
        assert_eq!(classify_layer("api"), Some("presentation"));
        assert_eq!(classify_layer("controllers"), Some("presentation"));
        // Unconventional names stay unassigned rather than guessing.
        assert_eq!(classify_layer("billing"), None);
        assert_eq!(classify_layer("reasoning"), None);
    }

    #[test]
    fn test_scan_actual_model_discovers_without_desired() {
        use std::env::temp_dir;
        use std::fs;

        let tmp = temp_dir().join(format!("axon_nodesc_test_{}", std::process::id()));
        let src = tmp.join("src").join("billing");
        fs::create_dir_all(&src).unwrap();

        fs::write(
            src.join("types.rs"),
            r#"
pub struct Invoice {
    pub id: u64,
    pub total: f64,
}

pub struct Currency {
    pub code: String,
}

pub struct InvoiceRepository {}

impl Invoice {
    pub fn apply_discount(&self, pct: f64) -> f64 { todo!() }
}

impl InvoiceRepository {
    pub fn find_by_id(&self, id: u64) -> Option<Invoice> { todo!() }
}
"#,
        )
        .unwrap();

        // No enrichment model at all: pure heuristic discovery.
        let actual = scan_actual_model(&tmp, None).unwrap();

        assert_eq!(actual.bounded_contexts.len(), 1);
        let bc = &actual.bounded_contexts[0];
        assert_eq!(bc.name, "billing");

        // Invoice: has data fields + methods → Entity (heuristic)
        assert!(bc.entities.iter().any(|e| e.name == "Invoice"));
        let invoice = bc.entities.iter().find(|e| e.name == "Invoice").unwrap();
        assert_eq!(invoice.fields.len(), 2);
        assert_eq!(invoice.methods.len(), 1);

        // Currency: has data fields, no methods → ValueObject (heuristic)
        assert!(bc.value_objects.iter().any(|v| v.name == "Currency"));

        // InvoiceRepository: naming convention → Repository
        assert!(
            bc.repositories
                .iter()
                .any(|r| r.name == "InvoiceRepository")
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_actual_model_captures_traits_as_ports_and_newtypes() {
        use std::env::temp_dir;
        use std::fs;

        let tmp = temp_dir().join(format!("axon_traits_test_{}", std::process::id()));
        let src = tmp.join("src").join("billing");
        fs::create_dir_all(&src).unwrap();

        fs::write(
            src.join("ports.rs"),
            r#"
pub struct InvoiceId(pub u64);

pub trait InvoiceStore: Send + Sync {
    fn find(&self, id: InvoiceId) -> Option<String>;
    fn save(&self, invoice: String);
}

pub struct PgInvoiceAdapter {
    pub url: String,
}

impl InvoiceStore for PgInvoiceAdapter {
    fn find(&self, id: InvoiceId) -> Option<String> { todo!() }
    fn save(&self, invoice: String) { todo!() }
}
"#,
        )
        .unwrap();

        let actual = scan_actual_model(&tmp, None).unwrap();

        // The trait is a first-class `trait` symbol — the port.
        assert!(
            actual
                .symbols
                .iter()
                .any(|s| s.kind == "trait" && s.name == "InvoiceStore"),
            "trait port should be a `trait` symbol, got {:?}",
            actual
                .symbols
                .iter()
                .map(|s| (s.kind.clone(), s.name.clone()))
                .collect::<Vec<_>>()
        );
        // Its declared operations are `method` symbols (port operations).
        assert!(
            actual
                .symbols
                .iter()
                .any(|s| s.kind == "method" && s.name == "InvoiceStore::find")
        );

        // The adapter struct links to the port via an `implements` edge.
        assert!(
            actual.ast_edges.iter().any(|e| e.edge_type == "implements"
                && e.from_node == "PgInvoiceAdapter"
                && e.to_node == "InvoiceStore"),
            "adapter should implement the port, edges: {:?}",
            actual.ast_edges
        );

        // Supertrait bounds are captured as `implements` edges from the port.
        assert!(
            actual.ast_edges.iter().any(|e| e.edge_type == "implements"
                && e.from_node == "InvoiceStore"
                && e.to_node == "Send"),
            "supertrait bound should be an implements edge, edges: {:?}",
            actual.ast_edges
        );

        // Newtype keeps its inner type instead of flattening to zero fields.
        let id_vo = actual.bounded_contexts[0]
            .value_objects
            .iter()
            .find(|v| v.name == "InvoiceId")
            .expect("InvoiceId newtype should be a value object with its inner field");
        assert_eq!(id_vo.fields.len(), 1);
        assert_eq!(id_vo.fields[0].name, "0");
        assert!(id_vo.fields[0].field_type.contains("u64"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_actual_model_folds_ports_and_free_functions() {
        use std::env::temp_dir;
        use std::fs;

        let tmp = temp_dir().join(format!("axon_ports_fold_test_{}", std::process::id()));
        let src = tmp.join("src").join("billing");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("lib.rs"),
            r#"
pub trait OrderRepository {
    fn find(&self, id: u64) -> Option<String>;
}

pub trait PaymentService {
    fn charge(&self, amount: u64);
}

pub trait StripeGateway {
    fn call(&self);
}

pub fn compute_total(items: &[u64]) -> u64 { todo!() }
"#,
        )
        .unwrap();

        let actual = scan_actual_model(&tmp, None).unwrap();
        let bc = &actual.bounded_contexts[0];

        // Repository-named trait → typed Repository (port), aggregate inferred.
        let repo = bc
            .repositories
            .iter()
            .find(|r| r.name == "OrderRepository")
            .expect("trait repository should fold into typed repositories");
        assert_eq!(repo.aggregate, "Order");
        assert_eq!(repo.methods.len(), 1);

        // Service-named trait → typed domain Service.
        assert!(bc.services.iter().any(|s| s.name == "PaymentService"));
        // Gateway → infrastructure-facing service.
        assert!(bc.services.iter().any(|s| {
            s.name == "StripeGateway" && matches!(s.kind, ServiceKind::Infrastructure)
        }));

        // Free function → `function` symbol.
        assert!(
            actual
                .symbols
                .iter()
                .any(|s| s.kind == "function" && s.name == "compute_total"),
            "free function should be a `function` symbol, got {:?}",
            actual
                .symbols
                .iter()
                .filter(|s| s.kind == "function")
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
        );

        // Ports remain `trait` symbols in the graph as well.
        assert!(
            actual
                .symbols
                .iter()
                .any(|s| s.kind == "trait" && s.name == "OrderRepository")
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_actual_model_classifies_by_desired() {
        use std::env::temp_dir;
        use std::fs;

        let tmp = temp_dir().join(format!("axon_scan_test_{}", std::process::id()));
        let src = tmp.join("src").join("domain");
        fs::create_dir_all(&src).unwrap();

        // Write a Rust file with structs matching the enrichment model.
        fs::write(
            src.join("model.rs"),
            r#"
pub struct User {
    pub name: String,
    pub email: Option<String>,
}

pub struct Email {
    pub value: String,
}

impl User {
    pub fn change_email(&self, email: Email) -> Result<()> { todo!() }
}
"#,
        )
        .unwrap();

        let desired = DomainModel {
            name: "Test".into(),
            description: "".into(),
            bounded_contexts: vec![BoundedContext {
                name: "domain".into(),
                description: "".into(),
                module_path: "src/domain".into(),
                ownership: Ownership::default(),
                aggregates: vec![],
                policies: vec![],
                read_models: vec![],
                entities: vec![Entity {
                    name: "User".into(),
                    description: "".into(),
                    aggregate_root: true,
                    fields: vec![],
                    methods: vec![],
                    invariants: vec!["Email must be unique".into()],
                    file_path: None,
                    start_line: None,
                    end_line: None,
                }],
                value_objects: vec![ValueObject {
                    name: "Email".into(),
                    description: "".into(),
                    fields: vec![],
                    validation_rules: vec![],
                    file_path: None,
                    start_line: None,
                    end_line: None,
                }],
                services: vec![],
                api_endpoints: vec![],
                repositories: vec![],
                events: vec![],
                modules: vec![],
                dependencies: vec![],
            }],
            external_systems: vec![],
            architectural_decisions: vec![],
            ownership: Ownership::default(),
            rules: vec![],
            tech_stack: TechStack::default(),
            conventions: Conventions::default(),
            ast_edges: vec![],
            source_files: vec![],
            symbols: vec![],
            import_edges: vec![],
            call_edges: vec![],
        };

        let actual = scan_actual_model(&tmp, Some(&desired)).unwrap();
        assert_eq!(actual.bounded_contexts.len(), 1);
        let bc = &actual.bounded_contexts[0];

        // User classified as entity (from desired)
        assert_eq!(bc.entities.len(), 1);
        assert_eq!(bc.entities[0].name, "User");
        assert!(bc.entities[0].aggregate_root); // inherited from desired
        assert_eq!(bc.entities[0].fields.len(), 2); // name, email from AST
        assert_eq!(bc.entities[0].methods.len(), 1); // change_email
        // Invariants carried from enrichment metadata.
        assert_eq!(bc.entities[0].invariants.len(), 1);
        assert_eq!(bc.entities[0].invariants[0], "Email must be unique");

        // Email classified as value_object (from desired)
        assert_eq!(bc.value_objects.len(), 1);
        assert_eq!(bc.value_objects[0].name, "Email");
        assert_eq!(bc.value_objects[0].fields.len(), 1);

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_scan_actual_model_ignores_non_rust_sources() {
        use std::env::temp_dir;
        use std::fs;

        let tmp = temp_dir().join(format!("axon_non_rust_test_{}", std::process::id()));
        let src = tmp.join("src").join("orders");
        fs::create_dir_all(&src).unwrap();

        fs::write(
            src.join("Order.java"),
            r#"
package com.example.orders;

import java.util.List;

public class Order {
    private String orderId;
    private double total;

    public void addItem(String item) {
        // business logic
    }

    public double getTotal() {
        return total;
    }
}
"#,
        )
        .unwrap();

        fs::write(
            src.join("OrderRepository.java"),
            r#"
package com.example.orders;

public interface OrderRepository {
    Order findById(String id);
    void save(Order order);
}
"#,
        )
        .unwrap();

        fs::write(
            src.join("OrderStatus.java"),
            r#"
package com.example.orders;

public enum OrderStatus {
    PENDING, CONFIRMED, SHIPPED, CANCELLED;
}
"#,
        )
        .unwrap();

        let actual = scan_actual_model(&tmp, None).unwrap();
        assert!(
            actual.source_files.is_empty(),
            "Rust-only scanner must ignore non-Rust source files"
        );
        assert!(
            actual
                .bounded_contexts
                .iter()
                .all(|bc| bc.entities.is_empty()
                    && bc.value_objects.is_empty()
                    && bc.repositories.is_empty()
                    && bc.events.is_empty()
                    && bc.services.is_empty()),
            "Non-Rust sources must not populate the domain model"
        );

        let _ = fs::remove_dir_all(&tmp);
    }
}
