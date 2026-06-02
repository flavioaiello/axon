use anyhow::Result;
use serde::{Deserialize, Serialize};

// ─── AST Edge ──────────────────────────────────────────────────────────────

/// A structural dependency extracted from source AST (extends, implements, decorators).
///
/// For `decorators` edges (compiler directives, lints, derives) `to_node` is the
/// directive text (e.g. `allow(dead_code)`) and `from_node` is the annotated
/// symbol — `Owner::method`, `Owner.field`, or a bare type/fn/module name. The
/// `file_path`/`line` locate the declaration so a single query can list, say,
/// every `dead_code` flag with a jump target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ASTEdge {
    pub from_node: String,
    pub to_node: String,
    pub edge_type: String,
    /// Relative path of the file where the edge is declared.
    #[serde(default)]
    pub file_path: String,
    /// 1-based line of the declaration site (0 when unknown).
    #[serde(default)]
    pub line: usize,
}

/// A source file discovered in the workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFile {
    /// Relative path from workspace root
    pub path: String,
    /// Owning bounded context
    pub context: String,
    /// Programming language. The live scanner currently indexes Rust only.
    pub language: String,
}

/// A Rust symbol discovered in the source code.
///
/// Symbols are the model's Rust-aligned node vocabulary. The mapping from Rust
/// constructs to DDD concepts is:
///
/// - `struct` / `enum` → tactical building blocks (entity, value object,
///   aggregate member, event), refined by `classify_struct`/`classify_enum`.
/// - `trait` → a **port**: a repository, gateway, or domain-service interface.
///   The structs that `impl` it are its adapters, linked by `implements`
///   [`ASTEdge`]s. Capturing traits is what lets the graph represent the
///   ports-and-adapters boundary, not just concrete types.
/// - `method` → an operation on a struct (inherent impl) or a trait (port op),
///   named `Owner::method`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolDef {
    /// Canonical name of the symbol (e.g. "Store", "DomainModel")
    pub name: String,
    /// Kind: `struct`, `enum`, `trait`, `method`, `module`, `function`
    pub kind: String,
    /// Owning bounded context
    pub context: String,
    /// File where the symbol is defined (relative path)
    pub file_path: String,
    /// Start line in the file (1-based)
    pub start_line: usize,
    /// End line in the file (1-based)
    pub end_line: usize,
    /// Visibility: public, private, etc.
    pub visibility: String,
}

/// A file-level import edge: from_file imports to_module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportEdge {
    /// Source file (relative path)
    pub from_file: String,
    /// Target module or symbol path
    pub to_module: String,
    /// Owning bounded context of the source file
    pub context: String,
}

/// A symbol-level call edge: caller invokes callee.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallEdge {
    /// Fully qualified caller symbol (e.g. "Store::save_desired")
    pub caller: String,
    /// Fully qualified callee symbol (e.g. "Store::save_state")
    pub callee: String,
    /// File where the call occurs (relative path)
    pub file_path: String,
    /// Line number of the call site (1-based)
    pub line: usize,
    /// Owning bounded context
    pub context: String,
}

/// A source-level reference edge: from_file references to_path in code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceEdge {
    /// Source file (relative path)
    pub from_file: String,
    /// Referenced Rust path or symbol path
    pub to_path: String,
    /// Kind: `type`, `trait_bound`, `expr_path`, or `macro`
    pub reference_kind: String,
    /// Line number of the reference site (1-based)
    pub line: usize,
    /// Owning bounded context of the source file
    pub context: String,
}

// ─── Top-Level Domain Model ────────────────────────────────────────────────

/// The root of the domain model configuration.
/// Describes the entire system architecture that Copilot should adhere to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainModel {
    /// Human-readable project name
    pub name: String,
    /// Project description
    #[serde(default)]
    pub description: String,
    /// Bounded contexts (DDD)
    #[serde(default)]
    pub bounded_contexts: Vec<BoundedContext>,
    /// External systems that define integration boundaries for the project
    #[serde(default)]
    pub external_systems: Vec<ExternalSystem>,
    /// Architecture decision records and their rationale
    #[serde(default)]
    pub architectural_decisions: Vec<ArchitecturalDecision>,
    /// Project ownership and stewardship metadata
    #[serde(default)]
    pub ownership: Ownership,
    /// Cross-cutting architectural rules
    #[serde(default)]
    pub rules: Vec<ArchitecturalRule>,
    /// Technology stack constraints
    #[serde(default)]
    pub tech_stack: TechStack,
    /// Naming conventions
    #[serde(default)]
    pub conventions: Conventions,
    /// AST structural dependencies (extends, implements, decorators)
    #[serde(default)]
    pub ast_edges: Vec<ASTEdge>,
    /// Source files discovered in the workspace
    #[serde(default)]
    pub source_files: Vec<SourceFile>,
    /// Symbols (structs, enums, functions) discovered in the workspace
    #[serde(default)]
    pub symbols: Vec<SymbolDef>,
    /// File-level import edges
    #[serde(default)]
    pub import_edges: Vec<ImportEdge>,
    /// Symbol-level call edges (function/method calls)
    #[serde(default)]
    pub call_edges: Vec<CallEdge>,
    /// Source-level type/path/macro reference edges
    #[serde(default)]
    pub reference_edges: Vec<ReferenceEdge>,
}

impl DomainModel {
    /// Create an empty model for a new workspace.
    pub fn empty(workspace_path: &str) -> Self {
        let name = std::path::Path::new(workspace_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unnamed".into());
        Self {
            name,
            description: String::new(),
            bounded_contexts: vec![],
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
            reference_edges: vec![],
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            anyhow::bail!("Domain model must have a name");
        }
        for bc in &self.bounded_contexts {
            if bc.name.is_empty() {
                anyhow::bail!("Bounded context must have a name");
            }
            for aggregate in &bc.aggregates {
                if aggregate.name.is_empty() {
                    anyhow::bail!(
                        "Aggregate in bounded context '{}' must have a name",
                        bc.name
                    );
                }
            }
            for policy in &bc.policies {
                if policy.name.is_empty() {
                    anyhow::bail!("Policy in bounded context '{}' must have a name", bc.name);
                }
            }
            for read_model in &bc.read_models {
                if read_model.name.is_empty() {
                    anyhow::bail!(
                        "Read model in bounded context '{}' must have a name",
                        bc.name
                    );
                }
            }
            for entity in &bc.entities {
                if entity.name.is_empty() {
                    anyhow::bail!("Entity in bounded context '{}' must have a name", bc.name);
                }
            }
        }
        for system in &self.external_systems {
            if system.name.is_empty() {
                anyhow::bail!("External system must have a name");
            }
        }
        for decision in &self.architectural_decisions {
            if decision.id.is_empty() {
                anyhow::bail!("Architectural decision must have an id");
            }
        }
        Ok(())
    }
}

// ─── Bounded Context ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundedContext {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Module path / namespace for this context
    #[serde(default)]
    pub module_path: String,
    #[serde(default)]
    pub ownership: Ownership,
    #[serde(default)]
    pub aggregates: Vec<Aggregate>,
    #[serde(default)]
    pub policies: Vec<Policy>,
    #[serde(default)]
    pub read_models: Vec<ReadModel>,
    #[serde(default)]
    pub entities: Vec<Entity>,
    #[serde(default)]
    pub value_objects: Vec<ValueObject>,
    #[serde(default)]
    pub services: Vec<Service>,
    #[serde(default)]
    pub api_endpoints: Vec<APIEndpoint>,
    #[serde(default)]
    pub repositories: Vec<Repository>,
    #[serde(default)]
    pub events: Vec<DomainEvent>,
    #[serde(default)]
    pub modules: Vec<Module>,
    /// Allowed dependencies to other bounded contexts
    #[serde(default)]
    pub dependencies: Vec<String>,
}

// ─── Module ────────────────────────────────────────────────────────────────

/// A discovered or declared module within a bounded context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    pub name: String,
    /// Fully-qualified module path (e.g., "domain::model")
    #[serde(default)]
    pub path: String,
    /// Whether the module is declared as `pub mod`
    #[serde(default)]
    pub public: bool,
    /// Source file where the module is declared
    #[serde(default)]
    pub file_path: String,
    #[serde(default)]
    pub description: String,
}

// ─── Explicit Aggregates ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Aggregate {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub root_entity: String,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub value_objects: Vec<String>,
    #[serde(default)]
    pub ownership: Ownership,
}

// ─── Policies / Process Managers ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: PolicyKind,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub ownership: Ownership,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    #[default]
    Domain,
    ProcessManager,
    Integration,
}

// ─── Read Models ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadModel {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub ownership: Ownership,
}

// ─── Entity ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Whether this is an aggregate root
    #[serde(default)]
    pub aggregate_root: bool,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub methods: Vec<Method>,
    #[serde(default)]
    pub invariants: Vec<String>,

    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
}

// ─── Value Object ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueObject {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub validation_rules: Vec<String>,

    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
}

// ─── Service ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: ServiceKind,
    #[serde(default)]
    pub methods: Vec<Method>,
    #[serde(default)]
    pub dependencies: Vec<String>,

    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    #[default]
    Domain,
    Application,
    Infrastructure,
}

// ─── Repository ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub name: String,
    /// The aggregate root this repository manages
    pub aggregate: String,
    #[serde(default)]
    pub methods: Vec<Method>,

    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
}

// ─── Domain Event ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainEvent {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub fields: Vec<Field>,
    /// Which entity/aggregate emits this event
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
}

// ─── External Boundaries ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct APIEndpoint {
    pub id: String,
    pub service_id: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub route_pattern: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSystem {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub consumed_by_contexts: Vec<String>,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub ownership: Ownership,
}

// ─── Architectural Decisions ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitecturalDecision {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: DecisionStatus,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub consequences: Vec<String>,
    #[serde(default)]
    pub contexts: Vec<String>,
    #[serde(default)]
    pub ownership: Ownership,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    #[default]
    Proposed,
    Accepted,
    Superseded,
    Deprecated,
}

// ─── Ownership ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ownership {
    #[serde(default)]
    pub team: String,
    #[serde(default)]
    pub owners: Vec<String>,
    #[serde(default)]
    pub rationale: String,
}

// ─── Shared Building Blocks ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Method {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Vec<Field>,
    #[serde(default)]
    pub return_type: String,

    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
}

// ─── Architectural Rules ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitecturalRule {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub severity: Severity,
    /// The pattern/layer this rule applies to
    #[serde(default)]
    pub scope: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    #[default]
    Error,
    Warning,
    Info,
}

// ─── Tech Stack ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TechStack {
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub framework: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub messaging: String,
    #[serde(default)]
    pub additional: Vec<String>,
}

// ─── Conventions ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Conventions {
    #[serde(default)]
    pub naming: NamingConventions,
    #[serde(default)]
    pub file_structure: FileStructure,
    #[serde(default)]
    pub error_handling: String,
    #[serde(default)]
    pub testing: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NamingConventions {
    #[serde(default)]
    pub entities: String,
    #[serde(default)]
    pub value_objects: String,
    #[serde(default)]
    pub services: String,
    #[serde(default)]
    pub repositories: String,
    #[serde(default)]
    pub events: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileStructure {
    /// e.g. "src/{context}/{layer}/{type}.rs"
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub layers: Vec<String>,
}
