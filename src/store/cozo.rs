use anyhow::{Context, Result};
use cozo::{DbInstance, ScriptMutability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::domain::model::*;

/// CozoDB-backed cerebral store for domain models.
///
/// Architecture:
/// - Every domain element is stored as a **first-class relational tuple**.
/// - Sub-structures (fields, methods, parameters, invariants, validation rules)
///   are their own relations — not JSON blobs. Datalog can reason about them directly.
/// - Domain/source relations use Cozo `Validity` for point-in-time actual-state history.
/// - Diffs are temporal comparisons over the implemented graph, not desired-vs-actual slices.
/// - `DomainModel` structs are reconstructed on-demand from relations.
pub struct Store {
    db: DbInstance,
}

impl Store {
    /// Open an in-memory store.
    ///
    /// The path parameter is retained for callers that still derive a crate-local
    /// store location, but CozoDB data now lives only for the process lifetime.
    pub fn open(_path: &Path) -> Result<Self> {
        let db = DbInstance::new("mem", "", Default::default())
            .map_err(|e| anyhow::anyhow!("Failed to open in-memory CozoDB: {:?}", e))?;

        Self::init_schema(&db)?;
        Ok(Self { db })
    }

    // ── Schema ─────────────────────────────────────────────────────────────

    fn init_schema(db: &DbInstance) -> Result<()> {
        // Migration v0: old schema used 'workspace_path' key on project
        let has_v0 = db
            .run_script(
                "?[x] := *project{workspace_path: x}",
                Default::default(),
                ScriptMutability::Immutable,
            )
            .is_ok();

        if has_v0 {
            let old_tables = [
                "project",
                "context",
                "context_dep",
                "entity",
                "entity_field",
                "entity_method",
                "method_param",
                "invariant",
                "service",
                "service_dep",
                "service_method",
                "event",
                "event_field",
                "value_object",
                "repository",
                "arch_rule",
                "live_import",
            ];
            for t in old_tables {
                let _ = db.run_script(
                    &format!("::remove {t}"),
                    Default::default(),
                    ScriptMutability::Mutable,
                );
            }
        }

        // Migration v1: schema had *_json blob columns on entity/service/event/etc.
        let has_v1 = db
            .run_script(
                "?[x] := *entity{fields_json: x}",
                Default::default(),
                ScriptMutability::Immutable,
            )
            .is_ok();

        if has_v1 {
            for t in ["entity", "service", "event", "value_object", "repository"] {
                let _ = db.run_script(
                    &format!("::remove {t}"),
                    Default::default(),
                    ScriptMutability::Mutable,
                );
            }
        }

        // Migration v2: tables lacked file_path/start_line/end_line columns
        let needs_v2 = db
            .run_script(
                "?[x] := *service{file_path: x}",
                Default::default(),
                ScriptMutability::Immutable,
            )
            .is_err()
            && db
                .run_script(
                    "?[x] := *service{name: x}",
                    Default::default(),
                    ScriptMutability::Immutable,
                )
                .is_ok();

        if needs_v2 {
            for t in [
                "entity",
                "service",
                "event",
                "value_object",
                "repository",
                "module",
            ] {
                let _ = db.run_script(
                    &format!("::remove {t}"),
                    Default::default(),
                    ScriptMutability::Mutable,
                );
            }
        }

        // Migration v3: schema lacked Validity columns for time-travel
        let needs_v3 = db
            .run_script(
                "?[x] := *context{workspace: x}",
                Default::default(),
                ScriptMutability::Immutable,
            )
            .is_ok()
            && db
                .run_script(
                    "?[x] := *context{workspace: x @ 'NOW'}",
                    Default::default(),
                    ScriptMutability::Immutable,
                )
                .is_err();

        if needs_v3 {
            let temporal_tables = [
                "context",
                "context_dep",
                "owner_meta",
                "aggregate",
                "aggregate_member",
                "entity",
                "policy",
                "policy_link",
                "read_model",
                "service",
                "service_dep",
                "event",
                "value_object",
                "repository",
                "module",
                "external_system",
                "external_system_context",
                "api_endpoint",
                "invokes_endpoint",
                "calls_external_system",
                "architectural_decision",
                "decision_context",
                "decision_consequence",
                "invariant",
                "field",
                "method",
                "method_param",
                "vo_rule",
                "ast_edge",
                "source_file",
                "symbol",
                "import_edge",
            ];
            for t in temporal_tables {
                let _ = db.run_script(
                    &format!("::remove {t}"),
                    Default::default(),
                    ScriptMutability::Mutable,
                );
            }
        }

        let schemas = vec![
            // Project metadata (rules/tech/conventions as JSON — config, not domain topology)
            ":create project { workspace: String => name: String, description: String default '', updated_at: String, rules_json: String default '[]', tech_stack_json: String default '{}', conventions_json: String default '{}' }",
            // ── Domain element headers (all with Validity for actual-state time-travel) ──
            ":create context { workspace: String, name: String, vld: Validity default 'ASSERT' => description: String default '', module_path: String default '' }",
            ":create context_dep { workspace: String, from_ctx: String, to_ctx: String, vld: Validity default 'ASSERT' }",
            ":create owner_meta { workspace: String, context: String, owner_kind: String, owner: String, vld: Validity default 'ASSERT' => team: String default '', owners_json: String default '[]', rationale: String default '' }",
            ":create aggregate { workspace: String, context: String, name: String, vld: Validity default 'ASSERT' => description: String default '', root_entity: String default '' }",
            ":create aggregate_member { workspace: String, context: String, aggregate: String, member_kind: String, member: String, vld: Validity default 'ASSERT' }",
            ":create entity { workspace: String, context: String, name: String, vld: Validity default 'ASSERT' => description: String default '', aggregate_root: Bool default false, file_path: String default '', start_line: Int default 0, end_line: Int default 0 }",
            ":create policy { workspace: String, context: String, name: String, vld: Validity default 'ASSERT' => description: String default '', kind: String default 'domain' }",
            ":create policy_link { workspace: String, context: String, policy: String, link_kind: String, link: String, idx: Int, vld: Validity default 'ASSERT' }",
            ":create read_model { workspace: String, context: String, name: String, vld: Validity default 'ASSERT' => description: String default '', source: String default '' }",
            ":create service { workspace: String, context: String, name: String, vld: Validity default 'ASSERT' => description: String default '', kind: String default 'domain', file_path: String default '', start_line: Int default 0, end_line: Int default 0 }",
            ":create service_dep { workspace: String, context: String, service: String, dep: String, vld: Validity default 'ASSERT' }",
            ":create event { workspace: String, context: String, name: String, vld: Validity default 'ASSERT' => description: String default '', source: String default '', file_path: String default '', start_line: Int default 0, end_line: Int default 0 }",
            ":create value_object { workspace: String, context: String, name: String, vld: Validity default 'ASSERT' => description: String default '', file_path: String default '', start_line: Int default 0, end_line: Int default 0 }",
            ":create repository { workspace: String, context: String, name: String, vld: Validity default 'ASSERT' => aggregate: String default '', file_path: String default '', start_line: Int default 0, end_line: Int default 0 }",
            ":create module { workspace: String, context: String, name: String, vld: Validity default 'ASSERT' => path: String default '', public: Bool default false, file_path: String default '', description: String default '' }",
            ":create external_system { workspace: String, name: String, vld: Validity default 'ASSERT' => description: String default '', kind: String default '', rationale: String default '' }",
            ":create external_system_context { workspace: String, system: String, context: String, idx: Int, vld: Validity default 'ASSERT' }",
            ":create api_endpoint { workspace: String, context: String, id: String, vld: Validity default 'ASSERT' => service_id: String default '', method: String default '', route_pattern: String default '', description: String default '' }",
            ":create invokes_endpoint { workspace: String, caller_context: String, caller_method: String, endpoint_id: String, vld: Validity default 'ASSERT' }",
            ":create calls_external_system { workspace: String, caller_context: String, caller_method: String, ext_id: String, vld: Validity default 'ASSERT' }",
            ":create architectural_decision { workspace: String, id: String, vld: Validity default 'ASSERT' => title: String default '', status: String default 'proposed', scope: String default '', date: String default '', rationale: String default '' }",
            ":create decision_context { workspace: String, decision_id: String, context: String, idx: Int, vld: Validity default 'ASSERT' }",
            ":create decision_consequence { workspace: String, decision_id: String, idx: Int, vld: Validity default 'ASSERT' => text: String default '' }",
            // ── First-class sub-structures ──
            ":create invariant { workspace: String, context: String, entity: String, idx: Int, vld: Validity default 'ASSERT' => text: String }",
            ":create field { workspace: String, context: String, owner_kind: String, owner: String, name: String, vld: Validity default 'ASSERT' => field_type: String default '', required: Bool default false, description: String default '', idx: Int default 0 }",
            ":create method { workspace: String, context: String, owner_kind: String, owner: String, name: String, vld: Validity default 'ASSERT' => description: String default '', return_type: String default '', idx: Int default 0 }",
            ":create method_param { workspace: String, context: String, owner_kind: String, owner: String, method: String, name: String, vld: Validity default 'ASSERT' => param_type: String default '', required: Bool default false, description: String default '', idx: Int default 0 }",
            ":create vo_rule { workspace: String, context: String, value_object: String, idx: Int, vld: Validity default 'ASSERT' => text: String }",
            // ── Architecture policy relations (no state, no Validity) ──
            ":create layer_assignment { workspace: String, context: String => layer: String }",
            ":create dependency_constraint { workspace: String, constraint_kind: String, source: String, target: String => rule: String default 'forbidden' }",
            // Ephemeral — no state column
            ":create live_import { workspace: String, from_file: String, to_module: String }",
            // AST structural edges (extends, implements, decorators)
            ":create ast_edge { workspace: String, from_node: String, to_node: String, edge_type: String, vld: Validity default 'ASSERT' }",
            // ── Source-level relations ──
            ":create source_file { workspace: String, path: String, vld: Validity default 'ASSERT' => context: String default '', language: String default '' }",
            ":create symbol { workspace: String, name: String, vld: Validity default 'ASSERT' => kind: String default '', context: String default '', file_path: String default '', start_line: Int default 0, end_line: Int default 0, visibility: String default 'public' }",
            ":create import_edge { workspace: String, from_file: String, to_module: String, vld: Validity default 'ASSERT' => context: String default '' }",
            // ── Symbol-level call graph ──
            ":create calls_symbol { workspace: String, caller: String, callee: String, vld: Validity default 'ASSERT' => file_path: String default '', line: Int default 0, context: String default '' }",
            // ── Drift model ──
            ":create drift { workspace: String, category: String, context: String, name: String, change_type: String, vld: Validity default 'ASSERT' => detail: String default '' }",
            ":create drift_meta { workspace: String => computed_at_us: Int default 0 }",
            // ── Reasoning kernel relations (non-temporal, current cache only) ──
            ":create reasoning_claim { workspace: String, claim_id: String => claim_kind: String default '', subject: String default '', status: String default '', summary: String default '', payload_json: String default '{}', provenance_source: String default '', provenance_state: String default '', stale: Bool default true, computed_at_us: Int default 0 }",
            ":create reasoning_derivation { workspace: String, claim_id: String, idx: Int => rule: String default '', derived_from_json: String default '[]', witness_count: Int default 0 }",
            ":create reasoning_assumption { workspace: String, claim_id: String, idx: Int => assumption_kind: String default 'assumption', text: String default '' }",
            ":create reasoning_support { workspace: String, claim_id: String, idx: Int => support_kind: String default '', summary: String default '', detail_json: String default '{}' }",
            ":create reasoning_dependency { workspace: String, claim_id: String, idx: Int => dependency_kind: String default '', dependency_state: String default '', basis_timestamp_us: Int default 0 }",
            ":create reasoning_justification { workspace: String, claim_id: String, idx: Int => fact_kind: String default '', fact_key: String default '', fact_state: String default '', basis_timestamp_us: Int default 0 }",
            // ── Snapshot log (explicit timestamp tracking for list_snapshots) ──
            ":create snapshot_log { workspace: String, timestamp_us: Int => label: String default '' }",
        ];

        for schema in schemas {
            db.run_script(schema, Default::default(), ScriptMutability::Mutable)
                .map_err(|e| {
                    anyhow::anyhow!("Failed to create Cozo schema relation from `{schema}`: {e:?}")
                })?;
        }

        // ── Secondary indices ──
        // CozoDB indices are reordered stored relations, queryable directly.
        // They avoid full scans for reverse lookups and non-primary-key filters.
        let indices = [
            // Reverse context dependency: "who depends on me?"
            "::index create context_dep:reverse {to_ctx}",
            // Reverse service dependency: "who uses this service?"
            "::index create service_dep:reverse {dep}",
            // Find events by their source entity
            "::index create event:by_source {source}",
            // Find aggregate members by member name
            "::index create aggregate_member:by_member {member_kind, member}",
            // Find fields/methods by owner kind + owner
            "::index create field:by_owner {owner_kind, owner}",
            "::index create method:by_owner {owner_kind, owner}",
            // Reverse AST edges: "what points to this node?"
            "::index create ast_edge:reverse {to_node, edge_type}",
            // Context by module_path for live dependency matching
            "::index create context:by_module_path {module_path}",
            // Owners by owner_kind + owner
            "::index create owner_meta:by_owner {owner_kind, owner}",
            // External system contexts by context
            "::index create external_system_context:by_context {context}",
            // Calls/invocations by target
            "::index create invokes_endpoint:by_endpoint {endpoint_id}",
            "::index create calls_external_system:by_ext {ext_id}",
            // Source file by context
            "::index create source_file:by_context {context}",
            // Symbol by context + kind
            "::index create symbol:by_context {context, kind}",
            // Symbol by file_path (find all symbols in a file)
            "::index create symbol:by_file {file_path}",
            // Import edge by target module (reverse lookup)
            "::index create import_edge:by_target {to_module}",
            // Import edge by context
            "::index create import_edge:by_context {context}",
            // Call graph: reverse lookup (who calls this symbol?)
            "::index create calls_symbol:by_callee {callee}",
            // Call graph: by context
            "::index create calls_symbol:by_context {context}",
        ];
        for idx in indices {
            db.run_script(idx, Default::default(), ScriptMutability::Mutable)
                .map_err(|e| {
                    anyhow::anyhow!("Failed to create Cozo secondary index from `{idx}`: {e:?}")
                })?;
        }

        // ── Full-text search indices ──
        // CozoDB FTS enables keyword search across description and text fields.
        let fts_indices = [
            "::fts create context:fts {
                extractor: description,
                extract_filter: description != '',
                tokenizer: Simple,
                filters: [Lowercase]
            }",
            "::fts create entity:fts {
                extractor: description,
                extract_filter: description != '',
                tokenizer: Simple,
                filters: [Lowercase]
            }",
            "::fts create service:fts {
                extractor: description,
                extract_filter: description != '',
                tokenizer: Simple,
                filters: [Lowercase]
            }",
            "::fts create event:fts {
                extractor: description,
                extract_filter: description != '',
                tokenizer: Simple,
                filters: [Lowercase]
            }",
            "::fts create architectural_decision:title_fts {
                extractor: title,
                extract_filter: title != '',
                tokenizer: Simple,
                filters: [Lowercase]
            }",
            "::fts create architectural_decision:rationale_fts {
                extractor: rationale,
                extract_filter: rationale != '',
                tokenizer: Simple,
                filters: [Lowercase]
            }",
            "::fts create invariant:text_fts {
                extractor: text,
                tokenizer: Simple,
                filters: [Lowercase]
            }",
        ];
        for idx in fts_indices {
            db.run_script(idx, Default::default(), ScriptMutability::Mutable)
                .map_err(|e| {
                    anyhow::anyhow!("Failed to create Cozo full-text index from `{idx}`: {e:?}")
                })?;
        }

        Ok(())
    }

    // ── Core State Operations ──────────────────────────────────────────────

    /// Compatibility alias for saving the current implemented model.
    pub fn save_desired(&self, workspace_path: &str, model: &DomainModel) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.save_state(&ws, model, "actual")
    }

    /// Compatibility alias for loading the current implemented model.
    pub fn load_desired(&self, workspace_path: &str) -> Result<Option<DomainModel>> {
        self.reconstruct_model(workspace_path, "actual")
    }

    /// Load the actual domain model (reconstructed from relations).
    pub fn load_actual(&self, workspace_path: &str) -> Result<Option<DomainModel>> {
        self.reconstruct_model(workspace_path, "actual")
    }

    /// Save a scanned model as the actual state (from AST extraction).
    pub fn save_actual(&self, workspace_path: &str, model: &DomainModel) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.save_state(&ws, model, "actual")
    }

    /// Record a temporal checkpoint for the current implemented graph.
    pub fn record_actual_snapshot(&self, workspace_path: &str) -> Result<i64> {
        let ws = canonicalize_path(workspace_path);
        self.record_snapshot(&ws, "actual")
    }

    fn run_mutation_script(
        &self,
        script: &str,
        params: BTreeMap<String, cozo::DataValue>,
        context: impl Into<String>,
    ) -> Result<()> {
        let context = context.into();
        self.run_script(script, params, ScriptMutability::Mutable)
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("{context}: {:?}", e))
    }

    fn run_script(
        &self,
        script: &str,
        params: BTreeMap<String, cozo::DataValue>,
        mutability: ScriptMutability,
    ) -> std::result::Result<cozo::NamedRows, cozo::Error> {
        let script = strip_state_dimension_from_script(script);
        self.db.run_script(&script, params, mutability)
    }

    fn save_project_metadata(&self, workspace: &str, model: &DomainModel) -> Result<()> {
        let now = chrono_now();
        let rules_json = serde_json::to_string(&model.rules).unwrap_or_else(|_| "[]".into());
        let tech_json = serde_json::to_string(&model.tech_stack).unwrap_or_else(|_| "{}".into());
        let conv_json = serde_json::to_string(&model.conventions).unwrap_or_else(|_| "{}".into());
        let params = params_map(&[
            ("ws", workspace),
            ("name", &model.name),
            ("desc", &model.description),
            ("now", &now),
            ("rules", &rules_json),
            ("tech", &tech_json),
            ("conv", &conv_json),
        ]);
        self.run_mutation_script(
            "?[workspace, name, description, updated_at, rules_json, tech_stack_json, conventions_json] <- \
                [[$ws, $name, $desc, $now, $rules, $tech, $conv]] \
             :put project { workspace => name, description, updated_at, rules_json, tech_stack_json, conventions_json }",
            params,
            format!("save project metadata '{}'", model.name),
        )
    }

    /// Compatibility no-op: actual-first storage has no desired graph to promote.
    pub fn accept(&self, workspace_path: &str) -> Result<()> {
        self.invalidate_reasoning_claims_for_dependency(workspace_path, "actual")?;
        Ok(())
    }

    /// Compatibility no-op: actual-first storage returns the current implemented model.
    pub fn reset(&self, workspace_path: &str) -> Result<Option<DomainModel>> {
        self.invalidate_reasoning_claims_for_dependency(workspace_path, "actual")?;
        self.load_actual(workspace_path)
    }

    // ── Private: Sub-structure Helpers ──────────────────────────────────────

    /// Save a slice of fields into the `field` relation.
    fn save_fields(
        &self,
        ws: &str,
        ctx: &str,
        owner_kind: &str,
        owner: &str,
        fields: &[Field],
        state: &str,
    ) -> Result<()> {
        for (i, f) in fields.iter().enumerate() {
            let mut params = params_map(&[
                ("ws", ws),
                ("ctx", ctx),
                ("ok", owner_kind),
                ("ow", owner),
                ("name", &f.name),
                ("st", state),
                ("ft", &f.field_type),
                ("desc", &f.description),
            ]);
            params.insert("req".into(), cozo::DataValue::Bool(f.required));
            params.insert("idx".into(), int_dv(i as i64));
            self
                .run_script(
                    "?[workspace, context, owner_kind, owner, name, state, field_type, required, description, idx] <- \
                        [[$ws, $ctx, $ok, $ow, $name, $st, $ft, $req, $desc, $idx]] \
                     :put field { workspace, context, owner_kind, owner, name, state => field_type, required, description, idx }",
                    params,
                    ScriptMutability::Mutable,
                )
                .map_err(|e| anyhow::anyhow!("save field '{}'.{}: {:?}", owner, f.name, e))?;
        }
        Ok(())
    }

    /// Save a slice of methods (+ their params) into the `method` and `method_param` relations.
    fn save_methods(
        &self,
        ws: &str,
        ctx: &str,
        owner_kind: &str,
        owner: &str,
        methods: &[Method],
        state: &str,
    ) -> Result<()> {
        for (i, m) in methods.iter().enumerate() {
            let mut params = params_map(&[
                ("ws", ws),
                ("ctx", ctx),
                ("ok", owner_kind),
                ("ow", owner),
                ("name", &m.name),
                ("st", state),
                ("desc", &m.description),
                ("rt", &m.return_type),
            ]);
            params.insert("idx".into(), int_dv(i as i64));
            self
                .run_script(
                    "?[workspace, context, owner_kind, owner, name, state, description, return_type, idx] <- \
                        [[$ws, $ctx, $ok, $ow, $name, $st, $desc, $rt, $idx]] \
                     :put method { workspace, context, owner_kind, owner, name, state => description, return_type, idx }",
                    params,
                    ScriptMutability::Mutable,
                )
                .map_err(|e| anyhow::anyhow!("save method '{}'.{}: {:?}", owner, m.name, e))?;

            // Method parameters
            for (j, p) in m.parameters.iter().enumerate() {
                let mut pp = params_map(&[
                    ("ws", ws),
                    ("ctx", ctx),
                    ("ok", owner_kind),
                    ("ow", owner),
                    ("method", &m.name),
                    ("name", &p.name),
                    ("st", state),
                    ("pt", &p.field_type),
                    ("desc", &p.description),
                ]);
                pp.insert("req".into(), cozo::DataValue::Bool(p.required));
                pp.insert("idx".into(), int_dv(j as i64));
                self
                    .run_script(
                        "?[workspace, context, owner_kind, owner, method, name, state, param_type, required, description, idx] <- \
                            [[$ws, $ctx, $ok, $ow, $method, $name, $st, $pt, $req, $desc, $idx]] \
                         :put method_param { workspace, context, owner_kind, owner, method, name, state => param_type, required, description, idx }",
                        pp,
                        ScriptMutability::Mutable,
                    )
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "save method_param '{}'.{}.{}: {:?}",
                            owner,
                            m.name,
                            p.name,
                            e
                        )
                    })?;
            }
        }
        Ok(())
    }

    fn save_owner_meta(
        &self,
        ws: &str,
        ctx: &str,
        owner_kind: &str,
        owner: &str,
        ownership: &Ownership,
        state: &str,
    ) -> Result<()> {
        let owners_json = serde_json::to_string(&ownership.owners).unwrap_or_else(|_| "[]".into());
        self
            .run_script(
                "?[workspace, context, owner_kind, owner, state, team, owners_json, rationale] <- [[$ws, $ctx, $ok, $owner, $st, $team, $owners, $rationale]] :put owner_meta { workspace, context, owner_kind, owner, state => team, owners_json, rationale }",
                params_map(&[
                    ("ws", ws),
                    ("ctx", ctx),
                    ("ok", owner_kind),
                    ("owner", owner),
                    ("st", state),
                    ("team", &ownership.team),
                    ("owners", &owners_json),
                    ("rationale", &ownership.rationale),
                ]),
                ScriptMutability::Mutable,
            )
            .map_err(|e| anyhow::anyhow!("save owner_meta '{}':'{}': {:?}", owner_kind, owner, e))?;
        Ok(())
    }

    fn remove_owner_meta(&self, ws: &str, ctx: &str, owner_kind: &str, owner: &str) -> Result<()> {
        self.run_mutation_script(
            "?[workspace, context, owner_kind, owner, state, vld] := *owner_meta{workspace, context, owner_kind, owner, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = $ok, owner = $owner, vld = 'RETRACT' :put owner_meta { workspace, context, owner_kind, owner, state, vld }",
            params_map(&[("ws", ws), ("ctx", ctx), ("ok", owner_kind), ("owner", owner)]),
            format!("remove owner_meta {owner_kind}:{owner}"),
        )
    }

    fn replace_owner_fields(
        &self,
        ws: &str,
        ctx: &str,
        owner_kind: &str,
        owner: &str,
        fields: &[Field],
    ) -> Result<()> {
        self.run_mutation_script(
            "?[workspace, context, owner_kind, owner, name, state, vld] := *field{workspace, context, owner_kind, owner, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = $ok, owner = $owner, state = 'desired', vld = 'RETRACT' :put field { workspace, context, owner_kind, owner, name, state, vld }",
            params_map(&[("ws", ws), ("ctx", ctx), ("ok", owner_kind), ("owner", owner)]),
            format!("replace fields for {owner_kind}:{owner}"),
        )?;
        self.save_fields(ws, ctx, owner_kind, owner, fields, "desired")
    }

    fn replace_owner_methods(
        &self,
        ws: &str,
        ctx: &str,
        owner_kind: &str,
        owner: &str,
        methods: &[Method],
    ) -> Result<()> {
        self.run_mutation_script(
            "?[workspace, context, owner_kind, owner, name, state, vld] := *method{workspace, context, owner_kind, owner, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = $ok, owner = $owner, state = 'desired', vld = 'RETRACT' :put method { workspace, context, owner_kind, owner, name, state, vld }",
            params_map(&[("ws", ws), ("ctx", ctx), ("ok", owner_kind), ("owner", owner)]),
            format!("replace methods for {owner_kind}:{owner}"),
        )?;
        self.run_mutation_script(
            "?[workspace, context, owner_kind, owner, method, name, state, vld] := *method_param{workspace, context, owner_kind, owner, method, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = $ok, owner = $owner, state = 'desired', vld = 'RETRACT' :put method_param { workspace, context, owner_kind, owner, method, name, state, vld }",
            params_map(&[("ws", ws), ("ctx", ctx), ("ok", owner_kind), ("owner", owner)]),
            format!("replace method params for {owner_kind}:{owner}"),
        )?;
        self.save_methods(ws, ctx, owner_kind, owner, methods, "desired")
    }

    fn replace_invariants(
        &self,
        ws: &str,
        ctx: &str,
        entity: &str,
        invariants: &[String],
    ) -> Result<()> {
        self.run_mutation_script(
                "?[workspace, context, entity, idx, state, text, vld] := *invariant{workspace, context, entity, idx, state, text @ 'NOW'}, workspace = $ws, context = $ctx, entity = $entity, state = 'desired', vld = 'RETRACT' :put invariant { workspace, context, entity, idx, state, vld => text }",
            params_map(&[("ws", ws), ("ctx", ctx), ("entity", entity)]),
            format!("replace invariants for entity:{entity}"),
        )?;
        for (idx, invariant) in invariants.iter().enumerate() {
            let mut params = params_map(&[
                ("ws", ws),
                ("ctx", ctx),
                ("entity", entity),
                ("text", invariant),
            ]);
            params.insert("idx".into(), int_dv(idx as i64));
            self.run_script(
                "?[workspace, context, entity, idx, state, text] <- [[$ws, $ctx, $entity, $idx, 'desired', $text]] :put invariant { workspace, context, entity, idx, state => text }",
                params,
                ScriptMutability::Mutable,
            ).map_err(|e| anyhow::anyhow!("replace_invariants '{}': {:?}", entity, e))?;
        }
        Ok(())
    }

    fn replace_vo_rules(
        &self,
        ws: &str,
        ctx: &str,
        value_object: &str,
        rules: &[String],
    ) -> Result<()> {
        self.run_mutation_script(
                "?[workspace, context, value_object, idx, state, text, vld] := *vo_rule{workspace, context, value_object, idx, state, text @ 'NOW'}, workspace = $ws, context = $ctx, value_object = $vo, state = 'desired', vld = 'RETRACT' :put vo_rule { workspace, context, value_object, idx, state, vld => text }",
            params_map(&[("ws", ws), ("ctx", ctx), ("vo", value_object)]),
            format!("replace value object rules for {value_object}"),
        )?;
        for (idx, rule) in rules.iter().enumerate() {
            let mut params = params_map(&[
                ("ws", ws),
                ("ctx", ctx),
                ("vo", value_object),
                ("text", rule),
            ]);
            params.insert("idx".into(), int_dv(idx as i64));
            self.run_script(
                "?[workspace, context, value_object, idx, state, text] <- [[$ws, $ctx, $vo, $idx, 'desired', $text]] :put vo_rule { workspace, context, value_object, idx, state => text }",
                params,
                ScriptMutability::Mutable,
            ).map_err(|e| anyhow::anyhow!("replace_vo_rules '{}': {:?}", value_object, e))?;
        }
        Ok(())
    }

    fn replace_service_deps(
        &self,
        ws: &str,
        ctx: &str,
        service: &str,
        dependencies: &[String],
    ) -> Result<()> {
        self.run_mutation_script(
            "?[workspace, context, service, dep, state, vld] := *service_dep{workspace, context, service, dep, state @ 'NOW'}, workspace = $ws, context = $ctx, service = $service, state = 'desired', vld = 'RETRACT' :put service_dep { workspace, context, service, dep, state, vld }",
            params_map(&[("ws", ws), ("ctx", ctx), ("service", service)]),
            format!("replace service dependencies for {service}"),
        )?;
        for dep in dependencies {
            self.run_script(
                "?[workspace, context, service, dep, state] <- [[$ws, $ctx, $service, $dep, 'desired']] :put service_dep { workspace, context, service, dep, state }",
                params_map(&[("ws", ws), ("ctx", ctx), ("service", service), ("dep", dep)]),
                ScriptMutability::Mutable,
            ).map_err(|e| anyhow::anyhow!("replace_service_deps '{}': {:?}", service, e))?;
        }
        Ok(())
    }

    fn ensure_project(&self, workspace_path: &str) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        let has_project = self
            .run_script(
                "?[name] := *project{workspace: $ws, name}",
                params_map(&[("ws", &ws)]),
                ScriptMutability::Immutable,
            )
            .map(|r| !r.rows.is_empty())
            .unwrap_or(false);
        if has_project {
            return Ok(());
        }

        let empty = DomainModel::empty(workspace_path);
        self.save_project_metadata(&ws, &empty)
            .map_err(|e| anyhow::anyhow!("ensure_project: {e}"))?;
        self.save_owner_meta(&ws, "", "project", &empty.name, &empty.ownership, "desired")?;
        Ok(())
    }

    /// Query fields for a specific owner from the `field` relation, ordered by idx.
    fn query_fields(
        &self,
        ws: &str,
        ctx: &str,
        owner_kind: &str,
        owner: &str,
        state: &str,
    ) -> Vec<Field> {
        let params = params_map(&[
            ("ws", ws),
            ("ctx", ctx),
            ("ok", owner_kind),
            ("ow", owner),
            ("st", state),
        ]);
        let rows = self
            .run_script(
                "?[name, field_type, required, description, idx] := \
                    *field{workspace: $ws, context: $ctx, owner_kind: $ok, owner: $ow, \
                           name, state: $st, field_type, required, description, idx @ 'NOW'}",
                params,
                ScriptMutability::Immutable,
            )
            .map(|r| r.rows)
            .unwrap_or_default();

        let mut indexed: Vec<(i64, Field)> = rows
            .iter()
            .map(|r| {
                (
                    dv_i64(&r[4]),
                    Field {
                        name: dv_str(&r[0]),
                        field_type: dv_str(&r[1]),
                        required: matches!(&r[2], cozo::DataValue::Bool(true)),
                        description: dv_str(&r[3]),
                    },
                )
            })
            .collect();
        indexed.sort_by_key(|(i, _)| *i);
        indexed.into_iter().map(|(_, f)| f).collect()
    }

    /// Query methods (+ their params) for a specific owner, ordered by idx.
    fn query_methods(
        &self,
        ws: &str,
        ctx: &str,
        owner_kind: &str,
        owner: &str,
        state: &str,
    ) -> Vec<Method> {
        let params = params_map(&[
            ("ws", ws),
            ("ctx", ctx),
            ("ok", owner_kind),
            ("ow", owner),
            ("st", state),
        ]);
        let rows = self
            .run_script(
                "?[name, description, return_type, idx] := \
                    *method{workspace: $ws, context: $ctx, owner_kind: $ok, owner: $ow, \
                            name, state: $st, description, return_type, idx @ 'NOW'}",
                params,
                ScriptMutability::Immutable,
            )
            .map(|r| r.rows)
            .unwrap_or_default();

        let mut indexed: Vec<(i64, Method)> = rows
            .iter()
            .map(|r| {
                let mname = dv_str(&r[0]);
                let mp = params_map(&[
                    ("ws", ws),
                    ("ctx", ctx),
                    ("ok", owner_kind),
                    ("ow", owner),
                    ("method", &mname),
                    ("st", state),
                ]);
                let param_rows = self
                    .run_script(
                        "?[name, param_type, required, description, idx] := \
                            *method_param{workspace: $ws, context: $ctx, owner_kind: $ok, \
                                          owner: $ow, method: $method, name, state: $st, \
                                          param_type, required, description, idx @ 'NOW'}",
                        mp,
                        ScriptMutability::Immutable,
                    )
                    .map(|r| r.rows)
                    .unwrap_or_default();

                let mut parms: Vec<(i64, Field)> = param_rows
                    .iter()
                    .map(|p| {
                        (
                            dv_i64(&p[4]),
                            Field {
                                name: dv_str(&p[0]),
                                field_type: dv_str(&p[1]),
                                required: matches!(&p[2], cozo::DataValue::Bool(true)),
                                description: dv_str(&p[3]),
                            },
                        )
                    })
                    .collect();
                parms.sort_by_key(|(i, _)| *i);

                (
                    dv_i64(&r[3]),
                    Method {
                        name: mname,
                        description: dv_str(&r[1]),
                        parameters: parms.into_iter().map(|(_, p)| p).collect(),
                        return_type: dv_str(&r[2]),
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    },
                )
            })
            .collect();
        indexed.sort_by_key(|(i, _)| *i);
        indexed.into_iter().map(|(_, m)| m).collect()
    }

    fn query_ownership(
        &self,
        ws: &str,
        ctx: &str,
        owner_kind: &str,
        owner: &str,
        state: &str,
    ) -> Ownership {
        let rows = self
            .run_script(
                "?[team, owners_json, rationale] := *owner_meta{workspace: $ws, context: $ctx, owner_kind: $ok, owner: $owner, state: $st, team, owners_json, rationale @ 'NOW'}",
                params_map(&[("ws", ws), ("ctx", ctx), ("ok", owner_kind), ("owner", owner), ("st", state)]),
                ScriptMutability::Immutable,
            )
            .map(|r| r.rows)
            .unwrap_or_default();

        if let Some(row) = rows.first() {
            let owners = serde_json::from_str::<Vec<String>>(&dv_str(&row[1])).unwrap_or_default();
            Ownership {
                team: dv_str(&row[0]),
                owners,
                rationale: dv_str(&row[2]),
            }
        } else {
            Ownership::default()
        }
    }

    fn query_indexed_strings(
        &self,
        query: &str,
        params: BTreeMap<String, cozo::DataValue>,
    ) -> Vec<String> {
        let rows = self
            .run_script(query, params, ScriptMutability::Immutable)
            .map(|r| r.rows)
            .unwrap_or_default();

        let mut indexed: Vec<(i64, String)> = rows
            .iter()
            .map(|row| (dv_i64(&row[0]), dv_str(&row[1])))
            .collect();
        indexed.sort_by_key(|(idx, _)| *idx);
        indexed.into_iter().map(|(_, value)| value).collect()
    }

    fn policy_kind_key(kind: &PolicyKind) -> &'static str {
        match kind {
            PolicyKind::Domain => "domain",
            PolicyKind::ProcessManager => "process_manager",
            PolicyKind::Integration => "integration",
        }
    }

    /// Query invariants for an entity, ordered by idx.
    fn query_invariants(&self, ws: &str, ctx: &str, entity: &str, state: &str) -> Vec<String> {
        let params = params_map(&[("ws", ws), ("ctx", ctx), ("ent", entity), ("st", state)]);
        let rows = self
            .run_script(
                "?[idx, text] := \
                    *invariant{workspace: $ws, context: $ctx, entity: $ent, \
                               idx, state: $st, text @ 'NOW'}",
                params,
                ScriptMutability::Immutable,
            )
            .map(|r| r.rows)
            .unwrap_or_default();

        let mut indexed: Vec<(i64, String)> = rows
            .iter()
            .map(|r| (dv_i64(&r[0]), dv_str(&r[1])))
            .collect();
        indexed.sort_by_key(|(i, _)| *i);
        indexed.into_iter().map(|(_, t)| t).collect()
    }

    /// Query validation rules for a value object, ordered by idx.
    fn query_vo_rules(&self, ws: &str, ctx: &str, vo: &str, state: &str) -> Vec<String> {
        let params = params_map(&[("ws", ws), ("ctx", ctx), ("vo", vo), ("st", state)]);
        let rows = self
            .run_script(
                "?[idx, text] := \
                    *vo_rule{workspace: $ws, context: $ctx, value_object: $vo, \
                             idx, state: $st, text @ 'NOW'}",
                params,
                ScriptMutability::Immutable,
            )
            .map(|r| r.rows)
            .unwrap_or_default();

        let mut indexed: Vec<(i64, String)> = rows
            .iter()
            .map(|r| (dv_i64(&r[0]), dv_str(&r[1])))
            .collect();
        indexed.sort_by_key(|(i, _)| *i);
        indexed.into_iter().map(|(_, t)| t).collect()
    }

    // ── Private: State Decomposition ───────────────────────────────────────

    /// Decompose a DomainModel into relational rows tagged with `state`.
    fn save_state(&self, workspace: &str, model: &DomainModel, state: &str) -> Result<()> {
        self.save_project_metadata(workspace, model)?;
        self.clear_state(workspace, state)?;
        self.save_owner_meta(
            workspace,
            "",
            "project",
            &model.name,
            &model.ownership,
            state,
        )?;

        for bc in &model.bounded_contexts {
            let params = params_map(&[
                ("ws", workspace),
                ("name", &bc.name),
                ("st", state),
                ("desc", &bc.description),
                ("mp", &bc.module_path),
            ]);
            self.run_script(
                "?[workspace, name, state, description, module_path] <- [[$ws, $name, $st, $desc, $mp]] :put context { workspace, name, state => description, module_path }",
                params,
                ScriptMutability::Mutable,
            ).map_err(|e| anyhow::anyhow!("save context '{}': {:?}", bc.name, e))?;

            self.save_owner_meta(
                workspace,
                &bc.name,
                "context",
                &bc.name,
                &bc.ownership,
                state,
            )?;

            for dep in &bc.dependencies {
                self.run_script(
                    "?[workspace, from_ctx, to_ctx, state] <- [[$ws, $from, $to, $st]] :put context_dep { workspace, from_ctx, to_ctx, state }",
                    params_map(&[("ws", workspace), ("from", &bc.name), ("to", dep), ("st", state)]),
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save context_dep: {:?}", e))?;
            }

            for aggregate in &bc.aggregates {
                self.run_script(
                    "?[workspace, context, name, state, description, root_entity] <- [[$ws, $ctx, $name, $st, $desc, $root]] :put aggregate { workspace, context, name, state => description, root_entity }",
                    params_map(&[("ws", workspace), ("ctx", &bc.name), ("name", &aggregate.name), ("st", state), ("desc", &aggregate.description), ("root", &aggregate.root_entity)]),
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save aggregate '{}': {:?}", aggregate.name, e))?;
                self.save_owner_meta(
                    workspace,
                    &bc.name,
                    "aggregate",
                    &aggregate.name,
                    &aggregate.ownership,
                    state,
                )?;
                for entity in &aggregate.entities {
                    self.run_script(
                        "?[workspace, context, aggregate, member_kind, member, state] <- [[$ws, $ctx, $agg, 'entity', $member, $st]] :put aggregate_member { workspace, context, aggregate, member_kind, member, state }",
                        params_map(&[("ws", workspace), ("ctx", &bc.name), ("agg", &aggregate.name), ("member", entity), ("st", state)]),
                        ScriptMutability::Mutable,
                    ).map_err(|e| anyhow::anyhow!("save aggregate entity member: {:?}", e))?;
                }
                for value_object in &aggregate.value_objects {
                    self.run_script(
                        "?[workspace, context, aggregate, member_kind, member, state] <- [[$ws, $ctx, $agg, 'value_object', $member, $st]] :put aggregate_member { workspace, context, aggregate, member_kind, member, state }",
                        params_map(&[("ws", workspace), ("ctx", &bc.name), ("agg", &aggregate.name), ("member", value_object), ("st", state)]),
                        ScriptMutability::Mutable,
                    ).map_err(|e| anyhow::anyhow!("save aggregate value_object member: {:?}", e))?;
                }
            }

            for entity in &bc.entities {
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("ctx", &bc.name),
                    ("name", &entity.name),
                    ("st", state),
                    ("desc", &entity.description),
                ]);
                params.insert("agg".into(), cozo::DataValue::Bool(entity.aggregate_root));
                params.insert(
                    "file".into(),
                    cozo::DataValue::Str(entity.file_path.as_deref().unwrap_or("").into()),
                );
                params.insert("sl".into(), int_dv(entity.start_line.unwrap_or(0) as i64));
                params.insert("el".into(), int_dv(entity.end_line.unwrap_or(0) as i64));
                self.run_script(
                    "?[workspace, context, name, state, description, aggregate_root, file_path, start_line, end_line] <- [[$ws, $ctx, $name, $st, $desc, $agg, $file, $sl, $el]] :put entity { workspace, context, name, state => description, aggregate_root, file_path, start_line, end_line }",
                    params,
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save entity '{}': {:?}", entity.name, e))?;
                self.save_fields(
                    workspace,
                    &bc.name,
                    "entity",
                    &entity.name,
                    &entity.fields,
                    state,
                )?;
                self.save_methods(
                    workspace,
                    &bc.name,
                    "entity",
                    &entity.name,
                    &entity.methods,
                    state,
                )?;
                for (idx, inv) in entity.invariants.iter().enumerate() {
                    let mut params = params_map(&[
                        ("ws", workspace),
                        ("ctx", &bc.name),
                        ("ent", &entity.name),
                        ("st", state),
                        ("text", inv),
                    ]);
                    params.insert("idx".into(), int_dv(idx as i64));
                    self.run_script(
                        "?[workspace, context, entity, idx, state, text] <- [[$ws, $ctx, $ent, $idx, $st, $text]] :put invariant { workspace, context, entity, idx, state => text }",
                        params,
                        ScriptMutability::Mutable,
                    ).map_err(|e| anyhow::anyhow!("save invariant: {:?}", e))?;
                }
            }

            for policy in &bc.policies {
                let kind_str = Self::policy_kind_key(&policy.kind).to_string();
                self.run_script(
                    "?[workspace, context, name, state, description, kind] <- [[$ws, $ctx, $name, $st, $desc, $kind]] :put policy { workspace, context, name, state => description, kind }",
                    params_map(&[("ws", workspace), ("ctx", &bc.name), ("name", &policy.name), ("st", state), ("desc", &policy.description), ("kind", &kind_str)]),
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save policy '{}': {:?}", policy.name, e))?;
                self.save_owner_meta(
                    workspace,
                    &bc.name,
                    "policy",
                    &policy.name,
                    &policy.ownership,
                    state,
                )?;
                for (idx, trigger) in policy.triggers.iter().enumerate() {
                    let mut params = params_map(&[
                        ("ws", workspace),
                        ("ctx", &bc.name),
                        ("policy", &policy.name),
                        ("link", trigger),
                        ("st", state),
                    ]);
                    params.insert("idx".into(), int_dv(idx as i64));
                    self.run_script(
                        "?[workspace, context, policy, link_kind, link, idx, state] <- [[$ws, $ctx, $policy, 'trigger', $link, $idx, $st]] :put policy_link { workspace, context, policy, link_kind, link, idx, state }",
                        params,
                        ScriptMutability::Mutable,
                    ).map_err(|e| anyhow::anyhow!("save policy trigger: {:?}", e))?;
                }
                for (idx, command) in policy.commands.iter().enumerate() {
                    let mut params = params_map(&[
                        ("ws", workspace),
                        ("ctx", &bc.name),
                        ("policy", &policy.name),
                        ("link", command),
                        ("st", state),
                    ]);
                    params.insert("idx".into(), int_dv(idx as i64));
                    self.run_script(
                        "?[workspace, context, policy, link_kind, link, idx, state] <- [[$ws, $ctx, $policy, 'command', $link, $idx, $st]] :put policy_link { workspace, context, policy, link_kind, link, idx, state }",
                        params,
                        ScriptMutability::Mutable,
                    ).map_err(|e| anyhow::anyhow!("save policy command: {:?}", e))?;
                }
            }

            for read_model in &bc.read_models {
                self.run_script(
                    "?[workspace, context, name, state, description, source] <- [[$ws, $ctx, $name, $st, $desc, $src]] :put read_model { workspace, context, name, state => description, source }",
                    params_map(&[("ws", workspace), ("ctx", &bc.name), ("name", &read_model.name), ("st", state), ("desc", &read_model.description), ("src", &read_model.source)]),
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save read_model '{}': {:?}", read_model.name, e))?;
                self.save_owner_meta(
                    workspace,
                    &bc.name,
                    "read_model",
                    &read_model.name,
                    &read_model.ownership,
                    state,
                )?;
                self.save_fields(
                    workspace,
                    &bc.name,
                    "read_model",
                    &read_model.name,
                    &read_model.fields,
                    state,
                )?;
            }

            for ep in &bc.api_endpoints {
                let params = params_map(&[
                    ("ws", workspace),
                    ("ctx", &bc.name),
                    ("id", &ep.id),
                    ("st", state),
                    ("svc", &ep.service_id),
                    ("met", &ep.method),
                    ("path", &ep.route_pattern),
                    ("desc", &ep.description),
                ]);
                self.run_script(
                    "?[workspace, context, id, state, service_id, method, route_pattern, description] <- \
                     [[$ws, $ctx, $id, $st, $svc, $met, $path, $desc]] \
                     :put api_endpoint { workspace, context, id, state => service_id, method, route_pattern, description }",
                    params,
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save api_endpoint: {:?}", e))?;
            }
            for svc in &bc.services {
                let kind_str = format!("{:?}", svc.kind).to_lowercase();
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("ctx", &bc.name),
                    ("name", &svc.name),
                    ("st", state),
                    ("desc", &svc.description),
                    ("kind", &kind_str),
                ]);
                params.insert(
                    "file".into(),
                    cozo::DataValue::Str(svc.file_path.as_deref().unwrap_or("").into()),
                );
                params.insert("sl".into(), int_dv(svc.start_line.unwrap_or(0) as i64));
                params.insert("el".into(), int_dv(svc.end_line.unwrap_or(0) as i64));
                self.run_script(
                    "?[workspace, context, name, state, description, kind, file_path, start_line, end_line] <- [[$ws, $ctx, $name, $st, $desc, $kind, $file, $sl, $el]] :put service { workspace, context, name, state => description, kind, file_path, start_line, end_line }",
                    params,
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save service '{}': {:?}", svc.name, e))?;
                self.save_methods(
                    workspace,
                    &bc.name,
                    "service",
                    &svc.name,
                    &svc.methods,
                    state,
                )?;
                for dep in &svc.dependencies {
                    self.run_script(
                        "?[workspace, context, service, dep, state] <- [[$ws, $ctx, $svc, $dep, $st]] :put service_dep { workspace, context, service, dep, state }",
                        params_map(&[("ws", workspace), ("ctx", &bc.name), ("svc", &svc.name), ("dep", dep), ("st", state)]),
                        ScriptMutability::Mutable,
                    ).map_err(|e| anyhow::anyhow!("save service_dep: {:?}", e))?;
                }
            }

            for evt in &bc.events {
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("ctx", &bc.name),
                    ("name", &evt.name),
                    ("st", state),
                    ("desc", &evt.description),
                    ("src", &evt.source),
                ]);
                params.insert(
                    "file".into(),
                    cozo::DataValue::Str(evt.file_path.as_deref().unwrap_or("").into()),
                );
                params.insert("sl".into(), int_dv(evt.start_line.unwrap_or(0) as i64));
                params.insert("el".into(), int_dv(evt.end_line.unwrap_or(0) as i64));
                self.run_script(
                    "?[workspace, context, name, state, description, source, file_path, start_line, end_line] <- [[$ws, $ctx, $name, $st, $desc, $src, $file, $sl, $el]] :put event { workspace, context, name, state => description, source, file_path, start_line, end_line }",
                    params,
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save event '{}': {:?}", evt.name, e))?;
                self.save_fields(workspace, &bc.name, "event", &evt.name, &evt.fields, state)?;
            }

            for vo in &bc.value_objects {
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("ctx", &bc.name),
                    ("name", &vo.name),
                    ("st", state),
                    ("desc", &vo.description),
                ]);
                params.insert(
                    "file".into(),
                    cozo::DataValue::Str(vo.file_path.as_deref().unwrap_or("").into()),
                );
                params.insert("sl".into(), int_dv(vo.start_line.unwrap_or(0) as i64));
                params.insert("el".into(), int_dv(vo.end_line.unwrap_or(0) as i64));
                self.run_script(
                    "?[workspace, context, name, state, description, file_path, start_line, end_line] <- [[$ws, $ctx, $name, $st, $desc, $file, $sl, $el]] :put value_object { workspace, context, name, state => description, file_path, start_line, end_line }",
                    params,
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save value_object '{}': {:?}", vo.name, e))?;
                self.save_fields(
                    workspace,
                    &bc.name,
                    "value_object",
                    &vo.name,
                    &vo.fields,
                    state,
                )?;
                for (idx, rule) in vo.validation_rules.iter().enumerate() {
                    let mut p = params_map(&[
                        ("ws", workspace),
                        ("ctx", &bc.name),
                        ("vo", &vo.name),
                        ("st", state),
                        ("text", rule),
                    ]);
                    p.insert("idx".into(), int_dv(idx as i64));
                    self.run_script(
                        "?[workspace, context, value_object, idx, state, text] <- [[$ws, $ctx, $vo, $idx, $st, $text]] :put vo_rule { workspace, context, value_object, idx, state => text }",
                        p,
                        ScriptMutability::Mutable,
                    ).map_err(|e| anyhow::anyhow!("save vo_rule: {:?}", e))?;
                }
            }

            for repo in &bc.repositories {
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("ctx", &bc.name),
                    ("name", &repo.name),
                    ("st", state),
                    ("agg", &repo.aggregate),
                ]);
                params.insert(
                    "file".into(),
                    cozo::DataValue::Str(repo.file_path.as_deref().unwrap_or("").into()),
                );
                params.insert("sl".into(), int_dv(repo.start_line.unwrap_or(0) as i64));
                params.insert("el".into(), int_dv(repo.end_line.unwrap_or(0) as i64));
                self.run_script(
                    "?[workspace, context, name, state, aggregate, file_path, start_line, end_line] <- [[$ws, $ctx, $name, $st, $agg, $file, $sl, $el]] :put repository { workspace, context, name, state => aggregate, file_path, start_line, end_line }",
                    params,
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save repository '{}': {:?}", repo.name, e))?;
                self.save_methods(
                    workspace,
                    &bc.name,
                    "repository",
                    &repo.name,
                    &repo.methods,
                    state,
                )?;
            }

            for module in &bc.modules {
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("ctx", &bc.name),
                    ("name", &module.name),
                    ("st", state),
                    ("path", &module.path),
                    ("fp", &module.file_path),
                    ("desc", &module.description),
                ]);
                params.insert("public".into(), cozo::DataValue::Bool(module.public));
                self.run_script(
                    "?[workspace, context, name, state, path, public, file_path, description] <- [[$ws, $ctx, $name, $st, $path, $public, $fp, $desc]] :put module { workspace, context, name, state => path, public, file_path, description }",
                    params,
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save module '{}': {:?}", module.name, e))?;
            }
        }

        for system in &model.external_systems {
            self.run_script(
                "?[workspace, name, state, description, kind, rationale] <- [[$ws, $name, $st, $desc, $kind, $rationale]] :put external_system { workspace, name, state => description, kind, rationale }",
                params_map(&[("ws", workspace), ("name", &system.name), ("st", state), ("desc", &system.description), ("kind", &system.kind), ("rationale", &system.rationale)]),
                ScriptMutability::Mutable,
            ).map_err(|e| anyhow::anyhow!("save external_system '{}': {:?}", system.name, e))?;
            self.save_owner_meta(
                workspace,
                "",
                "external_system",
                &system.name,
                &system.ownership,
                state,
            )?;
            for (idx, ctx) in system.consumed_by_contexts.iter().enumerate() {
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("name", &system.name),
                    ("ctx", ctx),
                    ("st", state),
                ]);
                params.insert("idx".into(), int_dv(idx as i64));
                self.run_script(
                    "?[workspace, system, context, idx, state] <- [[$ws, $name, $ctx, $idx, $st]] :put external_system_context { workspace, system, context, idx, state }",
                    params,
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save external_system_context: {:?}", e))?;
            }
        }

        for decision in &model.architectural_decisions {
            let status = format!("{:?}", decision.status).to_lowercase();
            self.run_script(
                "?[workspace, id, state, title, status, scope, date, rationale] <- [[$ws, $id, $st, $title, $status, $scope, $date, $rationale]] :put architectural_decision { workspace, id, state => title, status, scope, date, rationale }",
                params_map(&[("ws", workspace), ("id", &decision.id), ("st", state), ("title", &decision.title), ("status", &status), ("scope", &decision.scope), ("date", &decision.date), ("rationale", &decision.rationale)]),
                ScriptMutability::Mutable,
            ).map_err(|e| anyhow::anyhow!("save architectural_decision '{}': {:?}", decision.id, e))?;
            self.save_owner_meta(
                workspace,
                "",
                "architectural_decision",
                &decision.id,
                &decision.ownership,
                state,
            )?;
            for (idx, ctx) in decision.contexts.iter().enumerate() {
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("id", &decision.id),
                    ("ctx", ctx),
                    ("st", state),
                ]);
                params.insert("idx".into(), int_dv(idx as i64));
                self.run_script(
                    "?[workspace, decision_id, context, idx, state] <- [[$ws, $id, $ctx, $idx, $st]] :put decision_context { workspace, decision_id, context, idx, state }",
                    params,
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save decision_context: {:?}", e))?;
            }
            for (idx, consequence) in decision.consequences.iter().enumerate() {
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("id", &decision.id),
                    ("text", consequence),
                    ("st", state),
                ]);
                params.insert("idx".into(), int_dv(idx as i64));
                self.run_script(
                    "?[workspace, decision_id, idx, state, text] <- [[$ws, $id, $idx, $st, $text]] :put decision_consequence { workspace, decision_id, idx, state => text }",
                    params,
                    ScriptMutability::Mutable,
                ).map_err(|e| anyhow::anyhow!("save decision_consequence: {:?}", e))?;
            }
        }

        // Save AST edges
        for edge in &model.ast_edges {
            self.run_script(
                "?[workspace, state, from_node, to_node, edge_type] <- [[$ws, $st, $from, $to, $kind]] :put ast_edge { workspace, state, from_node, to_node, edge_type }",
                params_map(&[
                    ("ws", workspace),
                    ("st", state),
                    ("from", &edge.from_node),
                    ("to", &edge.to_node),
                    ("kind", &edge.edge_type),
                ]),
                ScriptMutability::Mutable,
            ).map_err(|e| anyhow::anyhow!("save ast_edge: {:?}", e))?;
        }

        // Save source files
        for sf in &model.source_files {
            self.run_script(
                "?[workspace, path, state, context, language] <- [[$ws, $path, $st, $ctx, $lang]] \
                 :put source_file { workspace, path, state => context, language }",
                params_map(&[
                    ("ws", workspace),
                    ("path", &sf.path),
                    ("st", state),
                    ("ctx", &sf.context),
                    ("lang", &sf.language),
                ]),
                ScriptMutability::Mutable,
            )
            .map_err(|e| anyhow::anyhow!("save source_file '{}': {:?}", sf.path, e))?;
        }

        // Save symbols
        for sym in &model.symbols {
            let mut params = params_map(&[
                ("ws", workspace),
                ("name", &sym.name),
                ("st", state),
                ("kind", &sym.kind),
                ("ctx", &sym.context),
                ("fp", &sym.file_path),
                ("vis", &sym.visibility),
            ]);
            params.insert("sl".into(), int_dv(sym.start_line as i64));
            params.insert("el".into(), int_dv(sym.end_line as i64));
            self.run_script(
                "?[workspace, name, state, kind, context, file_path, start_line, end_line, visibility] <- \
                 [[$ws, $name, $st, $kind, $ctx, $fp, $sl, $el, $vis]] \
                 :put symbol { workspace, name, state => kind, context, file_path, start_line, end_line, visibility }",
                params,
                ScriptMutability::Mutable,
            ).map_err(|e| anyhow::anyhow!("save symbol '{}': {:?}", sym.name, e))?;
        }

        // Save import edges
        for ie in &model.import_edges {
            self.run_script(
                "?[workspace, from_file, to_module, state, context] <- [[$ws, $ff, $tm, $st, $ctx]] \
                 :put import_edge { workspace, from_file, to_module, state => context }",
                params_map(&[
                    ("ws", workspace),
                    ("ff", &ie.from_file),
                    ("tm", &ie.to_module),
                    ("st", state),
                    ("ctx", &ie.context),
                ]),
                ScriptMutability::Mutable,
            ).map_err(|e| anyhow::anyhow!("save import_edge: {:?}", e))?;
        }

        // Save call edges
        for ce in &model.call_edges {
            let mut params = params_map(&[
                ("ws", workspace),
                ("caller", &ce.caller),
                ("callee", &ce.callee),
                ("st", state),
                ("fp", &ce.file_path),
                ("ctx", &ce.context),
            ]);
            params.insert("line".into(), int_dv(ce.line as i64));
            self.run_script(
                "?[workspace, caller, callee, state, file_path, line, context] <- [[$ws, $caller, $callee, $st, $fp, $line, $ctx]] \
                 :put calls_symbol { workspace, caller, callee, state => file_path, line, context }",
                params,
                ScriptMutability::Mutable,
            ).map_err(|e| anyhow::anyhow!("save calls_symbol: {:?}", e))?;
        }

        self.record_snapshot(workspace, state)?;
        self.invalidate_reasoning_claims_for_dependency(workspace, state)?;

        Ok(())
    }

    fn record_snapshot(&self, workspace: &str, state: &str) -> Result<i64> {
        let now_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as i64;
        let latest_ts = self
            .list_snapshots(workspace, state)?
            .into_iter()
            .next()
            .unwrap_or(0);
        let ts_us = now_us.max(latest_ts.saturating_add(1));
        let mut snap_params = params_map(&[("ws", workspace), ("st", state)]);
        snap_params.insert("ts".into(), int_dv(ts_us));
        self.run_mutation_script(
            "?[workspace, state, timestamp_us] <- [[$ws, $st, $ts]] \
             :put snapshot_log { workspace, state, timestamp_us }",
            snap_params,
            format!("save snapshot_log for '{workspace}' state '{state}'"),
        )?;
        Ok(ts_us)
    }

    /// Retract all current rows for a workspace+state (preserves temporal history).
    ///
    /// Instead of `:rm` (which destroys history), this creates RETRACT entries
    /// so that point-in-time queries at earlier timestamps still return old data.
    fn clear_state(&self, workspace: &str, state: &str) -> Result<()> {
        let params = params_map(&[("ws", workspace), ("st", state)]);
        // Each table: query current rows via @ 'NOW', then :put with vld='RETRACT'
        // Value columns use defaults (irrelevant for retraction semantics).
        let tables = [
            ("owner_meta", "workspace, context, owner_kind, owner, state"),
            ("context", "workspace, name, state"),
            ("context_dep", "workspace, from_ctx, to_ctx, state"),
            ("aggregate", "workspace, context, name, state"),
            (
                "aggregate_member",
                "workspace, context, aggregate, member_kind, member, state",
            ),
            ("entity", "workspace, context, name, state"),
            ("policy", "workspace, context, name, state"),
            (
                "policy_link",
                "workspace, context, policy, link_kind, link, idx, state",
            ),
            ("read_model", "workspace, context, name, state"),
            ("service", "workspace, context, name, state"),
            ("service_dep", "workspace, context, service, dep, state"),
            ("event", "workspace, context, name, state"),
            ("value_object", "workspace, context, name, state"),
            ("repository", "workspace, context, name, state"),
            ("module", "workspace, context, name, state"),
            ("api_endpoint", "workspace, context, id, state"),
            (
                "invokes_endpoint",
                "workspace, caller_context, caller_method, endpoint_id, state",
            ),
            (
                "calls_external_system",
                "workspace, caller_context, caller_method, ext_id, state",
            ),
            ("external_system", "workspace, name, state"),
            (
                "external_system_context",
                "workspace, system, context, idx, state",
            ),
            ("architectural_decision", "workspace, id, state"),
            (
                "decision_context",
                "workspace, decision_id, context, idx, state",
            ),
            ("decision_consequence", "workspace, decision_id, idx, state"),
            (
                "field",
                "workspace, context, owner_kind, owner, name, state",
            ),
            (
                "method",
                "workspace, context, owner_kind, owner, name, state",
            ),
            (
                "method_param",
                "workspace, context, owner_kind, owner, method, name, state",
            ),
            (
                "ast_edge",
                "workspace, state, from_node, to_node, edge_type",
            ),
            ("source_file", "workspace, path, state"),
            ("symbol", "workspace, name, state"),
            ("import_edge", "workspace, from_file, to_module, state"),
            ("calls_symbol", "workspace, caller, callee, state"),
        ];
        for (rel, keys) in tables {
            let script = format!(
                "?[{keys}, vld] := *{rel}{{{keys} @ 'NOW'}}, workspace = $ws, state = $st, vld = 'RETRACT' \
                 :put {rel} {{{keys}, vld}}"
            );
            self.run_mutation_script(
                &script,
                params.clone(),
                format!("clear_state retract {rel} for '{state}'"),
            )?;
        }
        self.run_mutation_script(
            "?[workspace, context, entity, idx, state, text, vld] := *invariant{workspace, context, entity, idx, state, text @ 'NOW'}, workspace = $ws, state = $st, vld = 'RETRACT' :put invariant { workspace, context, entity, idx, state, vld => text }",
            params.clone(),
            format!("clear_state retract invariant for '{state}'"),
        )?;
        self.run_mutation_script(
            "?[workspace, context, value_object, idx, state, text, vld] := *vo_rule{workspace, context, value_object, idx, state, text @ 'NOW'}, workspace = $ws, state = $st, vld = 'RETRACT' :put vo_rule { workspace, context, value_object, idx, state, vld => text }",
            params,
            format!("clear_state retract vo_rule for '{state}'"),
        )?;
        Ok(())
    }

    /// Reconstruct a DomainModel from relational rows for a given state.
    fn reconstruct_model(&self, workspace_path: &str, state: &str) -> Result<Option<DomainModel>> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("st", state)]);

        // Project metadata
        let proj = self
            .run_script(
                "?[name, description, rules_json, tech_stack_json, conventions_json] := \
                    *project{workspace: $ws, name, description, rules_json, tech_stack_json, conventions_json}",
                params_map(&[("ws", &ws)]),
                ScriptMutability::Immutable,
            )
            .ok();

        // Contexts for this state
        let ctxs = self
            .run_script(
                "?[name, description, module_path] := \
                    *context{workspace: $ws, name, state: $st, description, module_path @ 'NOW'}",
                p.clone(),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("reconstruct contexts: {:?}", e))?;

        let project_row = proj.as_ref().and_then(|rows| rows.rows.first());
        let has_project = project_row.is_some();

        if ctxs.rows.is_empty() && !has_project {
            return Ok(None);
        }

        // Extract project-level metadata
        let (project_name, description, rules, tech_stack, conventions) = if let Some(r) =
            project_row
        {
            (
                dv_str(&r[0]),
                dv_str(&r[1]),
                serde_json::from_str::<Vec<ArchitecturalRule>>(&dv_str(&r[2])).unwrap_or_default(),
                serde_json::from_str::<TechStack>(&dv_str(&r[3])).unwrap_or_default(),
                serde_json::from_str::<Conventions>(&dv_str(&r[4])).unwrap_or_default(),
            )
        } else {
            let name = std::path::Path::new(workspace_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Unnamed".into());
            (
                name,
                String::new(),
                vec![],
                TechStack::default(),
                Conventions::default(),
            )
        };

        let project_ownership = self.query_ownership(&ws, "", "project", &project_name, state);

        // Reconstruct each bounded context
        let mut bounded_contexts = Vec::new();
        for row in &ctxs.rows {
            let ctx_name = dv_str(&row[0]);

            // Dependencies
            let deps = self
                .run_script(
                    "?[to_ctx] := *context_dep{workspace: $ws, from_ctx: $ctx, to_ctx, state: $st @ 'NOW'}",
                    params_map(&[("ws", &ws), ("ctx", &ctx_name), ("st", state)]),
                    ScriptMutability::Immutable,
                )
                .map(|r| r.rows)
                .unwrap_or_default();
            let dependencies: Vec<String> = deps.iter().map(|r| dv_str(&r[0])).collect();

            let ownership = self.query_ownership(&ws, &ctx_name, "context", &ctx_name, state);

            let aggs = self
                .run_script(
                    "?[name, description, root_entity] := *aggregate{workspace: $ws, context: $ctx, name, state: $st, description, root_entity @ 'NOW'}",
                    params_map(&[("ws", &ws), ("ctx", &ctx_name), ("st", state)]),
                    ScriptMutability::Immutable,
                )
                .map(|r| r.rows)
                .unwrap_or_default();
            let aggregates: Vec<Aggregate> = aggs
                .iter()
                .map(|r| {
                    let aggregate_name = dv_str(&r[0]);
                    let members = self
                        .run_script(
                            "?[member_kind, member] := *aggregate_member{workspace: $ws, context: $ctx, aggregate: $agg, member_kind, member, state: $st @ 'NOW'}",
                            params_map(&[("ws", &ws), ("ctx", &ctx_name), ("agg", &aggregate_name), ("st", state)]),
                            ScriptMutability::Immutable,
                        )
                        .map(|r| r.rows)
                        .unwrap_or_default();
                    Aggregate {
                        name: aggregate_name.clone(),
                        description: dv_str(&r[1]),
                        root_entity: dv_str(&r[2]),
                        entities: members.iter().filter(|m| dv_str(&m[0]) == "entity").map(|m| dv_str(&m[1])).collect(),
                        value_objects: members.iter().filter(|m| dv_str(&m[0]) == "value_object").map(|m| dv_str(&m[1])).collect(),
                        ownership: self.query_ownership(&ws, &ctx_name, "aggregate", &aggregate_name, state),
                    }
                })
                .collect();

            // Entities
            let ents = self
                .run_script(
                    "?[name, description, aggregate_root, file_path, start_line, end_line] := \
                        *entity{workspace: $ws, context: $ctx, name, state: $st, \
                                description, aggregate_root, file_path, start_line, end_line @ 'NOW'}",
                    params_map(&[("ws", &ws), ("ctx", &ctx_name), ("st", state)]),
                    ScriptMutability::Immutable,
                )
                .map(|r| r.rows)
                .unwrap_or_default();
            let entities: Vec<Entity> = ents
                .iter()
                .map(|r| {
                    let ename = dv_str(&r[0]);
                    Entity {
                        name: ename.clone(),
                        description: dv_str(&r[1]),
                        aggregate_root: matches!(&r[2], cozo::DataValue::Bool(true)),
                        fields: self.query_fields(&ws, &ctx_name, "entity", &ename, state),
                        methods: self.query_methods(&ws, &ctx_name, "entity", &ename, state),
                        invariants: self.query_invariants(&ws, &ctx_name, &ename, state),
                        file_path: dv_opt_string(&r[3]),
                        start_line: dv_opt_usize(&r[4]),
                        end_line: dv_opt_usize(&r[5]),
                    }
                })
                .collect();

            let policy_rows = self
                .run_script(
                    "?[name, description, kind] := *policy{workspace: $ws, context: $ctx, name, state: $st, description, kind @ 'NOW'}",
                    params_map(&[("ws", &ws), ("ctx", &ctx_name), ("st", state)]),
                    ScriptMutability::Immutable,
                )
                .map(|r| r.rows)
                .unwrap_or_default();
            let policies: Vec<Policy> = policy_rows
                .iter()
                .map(|r| {
                    let policy_name = dv_str(&r[0]);
                    let links = self
                        .run_script(
                            "?[idx, link_kind, link] := *policy_link{workspace: $ws, context: $ctx, policy: $policy, idx, state: $st, link_kind, link @ 'NOW'}",
                            params_map(&[("ws", &ws), ("ctx", &ctx_name), ("policy", &policy_name), ("st", state)]),
                            ScriptMutability::Immutable,
                        )
                        .map(|r| r.rows)
                        .unwrap_or_default();
                    let mut indexed = links.iter().map(|row| (dv_i64(&row[0]), dv_str(&row[1]), dv_str(&row[2]))).collect::<Vec<_>>();
                    indexed.sort_by_key(|(idx, _, _)| *idx);
                    Policy {
                        name: policy_name.clone(),
                        description: dv_str(&r[1]),
                        kind: match dv_str(&r[2]).as_str() {
                            "process_manager" => PolicyKind::ProcessManager,
                            "integration" => PolicyKind::Integration,
                            _ => PolicyKind::Domain,
                        },
                        triggers: indexed.iter().filter(|(_, kind, _)| kind == "trigger").map(|(_, _, link)| link.clone()).collect(),
                        commands: indexed.iter().filter(|(_, kind, _)| kind == "command").map(|(_, _, link)| link.clone()).collect(),
                        ownership: self.query_ownership(&ws, &ctx_name, "policy", &policy_name, state),
                    }
                })
                .collect();

            let read_model_rows = self
                .run_script(
                    "?[name, description, source] := *read_model{workspace: $ws, context: $ctx, name, state: $st, description, source @ 'NOW'}",
                    params_map(&[("ws", &ws), ("ctx", &ctx_name), ("st", state)]),
                    ScriptMutability::Immutable,
                )
                .map(|r| r.rows)
                .unwrap_or_default();
            let read_models: Vec<ReadModel> = read_model_rows
                .iter()
                .map(|r| {
                    let read_name = dv_str(&r[0]);
                    ReadModel {
                        name: read_name.clone(),
                        description: dv_str(&r[1]),
                        source: dv_str(&r[2]),
                        fields: self.query_fields(&ws, &ctx_name, "read_model", &read_name, state),
                        ownership: self.query_ownership(
                            &ws,
                            &ctx_name,
                            "read_model",
                            &read_name,
                            state,
                        ),
                    }
                })
                .collect();

            // Services
            let svcs = self
                .run_script(
                    "?[name, description, kind, file_path, start_line, end_line] := \
                        *service{workspace: $ws, context: $ctx, name, state: $st, \
                                 description, kind, file_path, start_line, end_line @ 'NOW'}",
                    params_map(&[("ws", &ws), ("ctx", &ctx_name), ("st", state)]),
                    ScriptMutability::Immutable,
                )
                .map(|r| r.rows)
                .unwrap_or_default();
            let services: Vec<Service> = svcs
                .iter()
                .map(|r| {
                    let svc_name = dv_str(&r[0]);
                    let svc_deps = self
                        .run_script(
                            "?[dep] := *service_dep{workspace: $ws, context: $ctx, service: $svc, dep, state: $st @ 'NOW'}",
                            params_map(&[
                                ("ws", &ws),
                                ("ctx", &ctx_name),
                                ("svc", &svc_name),
                                ("st", state),
                            ]),
                            ScriptMutability::Immutable,
                        )
                        .map(|r| r.rows)
                        .unwrap_or_default();
                    Service {
                        name: svc_name.clone(),
                        description: dv_str(&r[1]),
                        kind: match dv_str(&r[2]).as_str() {
                            "application" => ServiceKind::Application,
                            "infrastructure" => ServiceKind::Infrastructure,
                            _ => ServiceKind::Domain,
                        },
                        methods: self.query_methods(&ws, &ctx_name, "service", &svc_name, state),
                        dependencies: svc_deps.iter().map(|r| dv_str(&r[0])).collect(),
                        file_path: dv_opt_string(&r[3]),
                        start_line: dv_opt_usize(&r[4]),
                        end_line: dv_opt_usize(&r[5]),
                    }
                })
                .collect();

            // Events
            let evts = self
                .run_script(
                    "?[name, description, source, file_path, start_line, end_line] := \
                        *event{workspace: $ws, context: $ctx, name, state: $st, \
                               description, source, file_path, start_line, end_line @ 'NOW'}",
                    params_map(&[("ws", &ws), ("ctx", &ctx_name), ("st", state)]),
                    ScriptMutability::Immutable,
                )
                .map(|r| r.rows)
                .unwrap_or_default();
            let api_endpoints_rows = self.run_script(
                "?[id, service_id, method, route_pattern, description] := *api_endpoint{workspace: $ws, context: $ctx, id, state: $st, service_id, method, route_pattern, description @ 'NOW'}",
                params_map(&[("ws", &ws), ("ctx", &ctx_name), ("st", state)]),
                ScriptMutability::Immutable,
            ).map(|r| r.rows).unwrap_or_default();
            let api_endpoints: Vec<APIEndpoint> = api_endpoints_rows
                .iter()
                .map(|r| APIEndpoint {
                    id: dv_str(&r[0]),
                    service_id: dv_str(&r[1]),
                    method: dv_str(&r[2]),
                    route_pattern: dv_str(&r[3]),
                    description: dv_str(&r[4]),
                })
                .collect();

            let events: Vec<DomainEvent> = evts
                .iter()
                .map(|r| {
                    let ename = dv_str(&r[0]);
                    DomainEvent {
                        name: ename.clone(),
                        description: dv_str(&r[1]),
                        source: dv_str(&r[2]),
                        fields: self.query_fields(&ws, &ctx_name, "event", &ename, state),
                        file_path: dv_opt_string(&r[3]),
                        start_line: dv_opt_usize(&r[4]),
                        end_line: dv_opt_usize(&r[5]),
                    }
                })
                .collect();

            // Value objects
            let vos = self
                .run_script(
                    "?[name, description, file_path, start_line, end_line] := \
                        *value_object{workspace: $ws, context: $ctx, name, state: $st, description, file_path, start_line, end_line @ 'NOW'}",
                    params_map(&[("ws", &ws), ("ctx", &ctx_name), ("st", state)]),
                    ScriptMutability::Immutable,
                )
                .map(|r| r.rows)
                .unwrap_or_default();
            let value_objects: Vec<ValueObject> = vos
                .iter()
                .map(|r| {
                    let voname = dv_str(&r[0]);
                    ValueObject {
                        name: voname.clone(),
                        description: dv_str(&r[1]),
                        fields: self.query_fields(&ws, &ctx_name, "value_object", &voname, state),
                        validation_rules: self.query_vo_rules(&ws, &ctx_name, &voname, state),
                        file_path: dv_opt_string(&r[2]),
                        start_line: dv_opt_usize(&r[3]),
                        end_line: dv_opt_usize(&r[4]),
                    }
                })
                .collect();

            // Repositories
            let repos = self
                .run_script(
                    "?[name, aggregate, file_path, start_line, end_line] := \
                        *repository{workspace: $ws, context: $ctx, name, state: $st, aggregate, file_path, start_line, end_line @ 'NOW'}",
                    params_map(&[("ws", &ws), ("ctx", &ctx_name), ("st", state)]),
                    ScriptMutability::Immutable,
                )
                .map(|r| r.rows)
                .unwrap_or_default();
            let repositories: Vec<Repository> = repos
                .iter()
                .map(|r| {
                    let rname = dv_str(&r[0]);
                    Repository {
                        name: rname.clone(),
                        aggregate: dv_str(&r[1]),
                        methods: self.query_methods(&ws, &ctx_name, "repository", &rname, state),
                        file_path: dv_opt_string(&r[2]),
                        start_line: dv_opt_usize(&r[3]),
                        end_line: dv_opt_usize(&r[4]),
                    }
                })
                .collect();

            // Modules
            let mods = self
                .run_script(
                    "?[name, path, public, file_path, description] := \
                        *module{workspace: $ws, context: $ctx, name, state: $st, path, public, file_path, description @ 'NOW'}",
                    params_map(&[("ws", &ws), ("ctx", &ctx_name), ("st", state)]),
                    ScriptMutability::Immutable,
                )
                .map(|r| r.rows)
                .unwrap_or_default();
            let modules: Vec<Module> = mods
                .iter()
                .map(|r| Module {
                    name: dv_str(&r[0]),
                    path: dv_str(&r[1]),
                    public: r[2].get_bool().unwrap_or(false),
                    file_path: dv_str(&r[3]),
                    description: dv_str(&r[4]),
                })
                .collect();

            bounded_contexts.push(BoundedContext {
                name: ctx_name,
                description: dv_str(&row[1]),
                module_path: dv_str(&row[2]),
                ownership,
                aggregates,
                policies,
                read_models,
                entities,
                value_objects,
                services,
                api_endpoints,
                repositories,
                events,
                modules,
                dependencies,
            });
        }

        let external_system_rows = self
            .run_script(
                "?[name, description, kind, rationale] := *external_system{workspace: $ws, name, state: $st, description, kind, rationale @ 'NOW'}",
                params_map(&[("ws", &ws), ("st", state)]),
                ScriptMutability::Immutable,
            )
            .map(|r| r.rows)
            .unwrap_or_default();
        let external_systems: Vec<ExternalSystem> = external_system_rows
            .iter()
            .map(|r| {
                let system_name = dv_str(&r[0]);
                ExternalSystem {
                    name: system_name.clone(),
                    description: dv_str(&r[1]),
                    kind: dv_str(&r[2]),
                    consumed_by_contexts: self.query_indexed_strings(
                        "?[idx, context] := *external_system_context{workspace: $ws, system: $name, idx, state: $st, context @ 'NOW'}",
                        params_map(&[("ws", &ws), ("name", &system_name), ("st", state)]),
                    ),
                    rationale: dv_str(&r[3]),
                    ownership: self.query_ownership(&ws, "", "external_system", &system_name, state),
                }
            })
            .collect();

        let decision_rows = self
            .run_script(
                "?[id, title, status, scope, date, rationale] := *architectural_decision{workspace: $ws, id, state: $st, title, status, scope, date, rationale @ 'NOW'}",
                params_map(&[("ws", &ws), ("st", state)]),
                ScriptMutability::Immutable,
            )
            .map(|r| r.rows)
            .unwrap_or_default();
        let architectural_decisions: Vec<ArchitecturalDecision> = decision_rows
            .iter()
            .map(|r| {
                let decision_id = dv_str(&r[0]);
                ArchitecturalDecision {
                    id: decision_id.clone(),
                    title: dv_str(&r[1]),
                    status: match dv_str(&r[2]).as_str() {
                        "accepted" => DecisionStatus::Accepted,
                        "superseded" => DecisionStatus::Superseded,
                        "deprecated" => DecisionStatus::Deprecated,
                        _ => DecisionStatus::Proposed,
                    },
                    scope: dv_str(&r[3]),
                    date: dv_str(&r[4]),
                    rationale: dv_str(&r[5]),
                    consequences: self.query_indexed_strings(
                        "?[idx, text] := *decision_consequence{workspace: $ws, decision_id: $id, idx, state: $st, text @ 'NOW'}",
                        params_map(&[("ws", &ws), ("id", &decision_id), ("st", state)]),
                    ),
                    contexts: self.query_indexed_strings(
                        "?[idx, context] := *decision_context{workspace: $ws, decision_id: $id, idx, state: $st, context @ 'NOW'}",
                        params_map(&[("ws", &ws), ("id", &decision_id), ("st", state)]),
                    ),
                    ownership: self.query_ownership(&ws, "", "architectural_decision", &decision_id, state),
                }
            })
            .collect();

        Ok(Some(DomainModel {
            name: project_name,
            description,
            bounded_contexts,
            external_systems,
            architectural_decisions,
            ownership: project_ownership,
            rules,
            tech_stack,
            conventions,
            ast_edges: {
                let rows = self.run_script(
                    "?[from_node, to_node, edge_type] := *ast_edge{workspace: $ws, state: $st, from_node, to_node, edge_type @ 'NOW'}",
                    params_map(&[("ws", &ws), ("st", state)]),
                    ScriptMutability::Immutable,
                ).map(|r| r.rows).unwrap_or_default();
                rows.iter()
                    .map(|r| crate::domain::model::ASTEdge {
                        from_node: dv_str(&r[0]),
                        to_node: dv_str(&r[1]),
                        edge_type: dv_str(&r[2]),
                    })
                    .collect()
            },
            source_files: {
                let rows = self.run_script(
                    "?[path, context, language] := *source_file{workspace: $ws, path, state: $st, context, language @ 'NOW'}",
                    params_map(&[("ws", &ws), ("st", state)]),
                    ScriptMutability::Immutable,
                ).map(|r| r.rows).unwrap_or_default();
                rows.iter()
                    .map(|r| SourceFile {
                        path: dv_str(&r[0]),
                        context: dv_str(&r[1]),
                        language: dv_str(&r[2]),
                    })
                    .collect()
            },
            symbols: {
                let rows = self.run_script(
                    "?[name, kind, context, file_path, start_line, end_line, visibility] := \
                     *symbol{workspace: $ws, name, state: $st, kind, context, file_path, start_line, end_line, visibility @ 'NOW'}",
                    params_map(&[("ws", &ws), ("st", state)]),
                    ScriptMutability::Immutable,
                ).map(|r| r.rows).unwrap_or_default();
                rows.iter()
                    .map(|r| SymbolDef {
                        name: dv_str(&r[0]),
                        kind: dv_str(&r[1]),
                        context: dv_str(&r[2]),
                        file_path: dv_str(&r[3]),
                        start_line: dv_i64(&r[4]) as usize,
                        end_line: dv_i64(&r[5]) as usize,
                        visibility: dv_str(&r[6]),
                    })
                    .collect()
            },
            import_edges: {
                let rows = self.run_script(
                    "?[from_file, to_module, context] := *import_edge{workspace: $ws, from_file, to_module, state: $st, context @ 'NOW'}",
                    params_map(&[("ws", &ws), ("st", state)]),
                    ScriptMutability::Immutable,
                ).map(|r| r.rows).unwrap_or_default();
                rows.iter()
                    .map(|r| ImportEdge {
                        from_file: dv_str(&r[0]),
                        to_module: dv_str(&r[1]),
                        context: dv_str(&r[2]),
                    })
                    .collect()
            },
            call_edges: {
                let rows = self.run_script(
                    "?[caller, callee, file_path, line, context] := *calls_symbol{workspace: $ws, caller, callee, state: $st, file_path, line, context @ 'NOW'}",
                    params_map(&[("ws", &ws), ("st", state)]),
                    ScriptMutability::Immutable,
                ).map(|r| r.rows).unwrap_or_default();
                rows.iter()
                    .map(|r| CallEdge {
                        caller: dv_str(&r[0]),
                        callee: dv_str(&r[1]),
                        file_path: dv_str(&r[2]),
                        line: dv_i64(&r[3]) as usize,
                        context: dv_str(&r[4]),
                    })
                    .collect()
            },
        }))
    }

    // ── Graph-native Query & Mutation Helpers ─────────────────────────────

    pub fn query_entity(&self, ws: &str, ctx: &str, name: &str) -> Option<Entity> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[description, aggregate_root, file_path, start_line, end_line] := *entity{workspace: $ws, context: $ctx, name: $name, state: 'desired', description, aggregate_root, file_path, start_line, end_line @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).ok()?.rows;
        let row = rows.first()?;
        Some(Entity {
            name: name.to_string(),
            description: dv_str(&row[0]),
            aggregate_root: matches!(&row[1], cozo::DataValue::Bool(true)),
            fields: self.query_fields(&ws, ctx, "entity", name, "desired"),
            methods: self.query_methods(&ws, ctx, "entity", name, "desired"),
            invariants: self.query_invariants(&ws, ctx, name, "desired"),
            file_path: dv_opt_string(&row[2]),
            start_line: dv_opt_usize(&row[3]),
            end_line: dv_opt_usize(&row[4]),
        })
    }

    pub fn query_service(&self, ws: &str, ctx: &str, name: &str) -> Option<Service> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[description, kind, file_path, start_line, end_line] := *service{workspace: $ws, context: $ctx, name: $name, state: 'desired', description, kind, file_path, start_line, end_line @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).ok()?.rows;
        let row = rows.first()?;
        let dep_rows = self.run_script(
            "?[dep] := *service_dep{workspace: $ws, context: $ctx, service: $name, dep, state: 'desired' @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).map(|r| r.rows).unwrap_or_default();
        Some(Service {
            name: name.to_string(),
            description: dv_str(&row[0]),
            kind: match dv_str(&row[1]).as_str() {
                "application" => ServiceKind::Application,
                "infrastructure" => ServiceKind::Infrastructure,
                _ => ServiceKind::Domain,
            },
            methods: self.query_methods(&ws, ctx, "service", name, "desired"),
            dependencies: dep_rows.iter().map(|r| dv_str(&r[0])).collect(),
            file_path: dv_opt_string(&row[2]),
            start_line: dv_opt_usize(&row[3]),
            end_line: dv_opt_usize(&row[4]),
        })
    }

    pub fn query_event(&self, ws: &str, ctx: &str, name: &str) -> Option<DomainEvent> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[description, source, file_path, start_line, end_line] := *event{workspace: $ws, context: $ctx, name: $name, state: 'desired', description, source, file_path, start_line, end_line @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).ok()?.rows;
        let row = rows.first()?;
        Some(DomainEvent {
            name: name.to_string(),
            description: dv_str(&row[0]),
            fields: self.query_fields(&ws, ctx, "event", name, "desired"),
            source: dv_str(&row[1]),
            file_path: dv_opt_string(&row[2]),
            start_line: dv_opt_usize(&row[3]),
            end_line: dv_opt_usize(&row[4]),
        })
    }

    pub fn query_value_object(&self, ws: &str, ctx: &str, name: &str) -> Option<ValueObject> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[description, file_path, start_line, end_line] := *value_object{workspace: $ws, context: $ctx, name: $name, state: 'desired', description, file_path, start_line, end_line @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).ok()?.rows;
        let row = rows.first()?;
        Some(ValueObject {
            name: name.to_string(),
            description: dv_str(&row[0]),
            fields: self.query_fields(&ws, ctx, "value_object", name, "desired"),
            validation_rules: self.query_vo_rules(&ws, ctx, name, "desired"),
            file_path: dv_opt_string(&row[1]),
            start_line: dv_opt_usize(&row[2]),
            end_line: dv_opt_usize(&row[3]),
        })
    }

    pub fn query_repository(&self, ws: &str, ctx: &str, name: &str) -> Option<Repository> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[aggregate, file_path, start_line, end_line] := *repository{workspace: $ws, context: $ctx, name: $name, state: 'desired', aggregate, file_path, start_line, end_line @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).ok()?.rows;
        let row = rows.first()?;
        Some(Repository {
            name: name.to_string(),
            aggregate: dv_str(&row[0]),
            methods: self.query_methods(&ws, ctx, "repository", name, "desired"),
            file_path: dv_opt_string(&row[1]),
            start_line: dv_opt_usize(&row[2]),
            end_line: dv_opt_usize(&row[3]),
        })
    }

    pub fn query_aggregate(&self, ws: &str, ctx: &str, name: &str) -> Option<Aggregate> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[description, root_entity] := *aggregate{workspace: $ws, context: $ctx, name: $name, state: 'desired', description, root_entity @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).ok()?.rows;
        let row = rows.first()?;
        let members = self.run_script(
            "?[member_kind, member] := *aggregate_member{workspace: $ws, context: $ctx, aggregate: $name, member_kind, member, state: 'desired' @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).map(|r| r.rows).unwrap_or_default();
        Some(Aggregate {
            name: name.to_string(),
            description: dv_str(&row[0]),
            root_entity: dv_str(&row[1]),
            entities: members
                .iter()
                .filter(|r| dv_str(&r[0]) == "entity")
                .map(|r| dv_str(&r[1]))
                .collect(),
            value_objects: members
                .iter()
                .filter(|r| dv_str(&r[0]) == "value_object")
                .map(|r| dv_str(&r[1]))
                .collect(),
            ownership: self.query_ownership(&ws, ctx, "aggregate", name, "desired"),
        })
    }

    pub fn query_policy(&self, ws: &str, ctx: &str, name: &str) -> Option<Policy> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[description, kind] := *policy{workspace: $ws, context: $ctx, name: $name, state: 'desired', description, kind @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).ok()?.rows;
        let row = rows.first()?;
        let links = self.run_script(
            "?[idx, link_kind, link] := *policy_link{workspace: $ws, context: $ctx, policy: $name, idx, state: 'desired', link_kind, link @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).map(|r| r.rows).unwrap_or_default();
        let mut indexed = links
            .iter()
            .map(|r| (dv_i64(&r[0]), dv_str(&r[1]), dv_str(&r[2])))
            .collect::<Vec<_>>();
        indexed.sort_by_key(|(idx, _, _)| *idx);
        Some(Policy {
            name: name.to_string(),
            description: dv_str(&row[0]),
            kind: match dv_str(&row[1]).as_str() {
                "process_manager" => PolicyKind::ProcessManager,
                "integration" => PolicyKind::Integration,
                _ => PolicyKind::Domain,
            },
            triggers: indexed
                .iter()
                .filter(|(_, kind, _)| kind == "trigger")
                .map(|(_, _, link)| link.clone())
                .collect(),
            commands: indexed
                .iter()
                .filter(|(_, kind, _)| kind == "command")
                .map(|(_, _, link)| link.clone())
                .collect(),
            ownership: self.query_ownership(&ws, ctx, "policy", name, "desired"),
        })
    }

    pub fn query_read_model(&self, ws: &str, ctx: &str, name: &str) -> Option<ReadModel> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[description, source] := *read_model{workspace: $ws, context: $ctx, name: $name, state: 'desired', description, source @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).ok()?.rows;
        let row = rows.first()?;
        Some(ReadModel {
            name: name.to_string(),
            description: dv_str(&row[0]),
            source: dv_str(&row[1]),
            fields: self.query_fields(&ws, ctx, "read_model", name, "desired"),
            ownership: self.query_ownership(&ws, ctx, "read_model", name, "desired"),
        })
    }

    pub fn query_external_system(&self, ws: &str, name: &str) -> Option<ExternalSystem> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[description, kind, rationale] := *external_system{workspace: $ws, name: $name, state: 'desired', description, kind, rationale @ 'NOW'}",
            params_map(&[("ws", &ws), ("name", name)]),
            ScriptMutability::Immutable,
        ).ok()?.rows;
        let row = rows.first()?;
        Some(ExternalSystem {
            name: name.to_string(),
            description: dv_str(&row[0]),
            kind: dv_str(&row[1]),
            consumed_by_contexts: self.query_indexed_strings(
                "?[idx, context] := *external_system_context{workspace: $ws, system: $name, idx, state: 'desired', context @ 'NOW'}",
                params_map(&[("ws", &ws), ("name", name)]),
            ),
            rationale: dv_str(&row[2]),
            ownership: self.query_ownership(&ws, "", "external_system", name, "desired"),
        })
    }

    pub fn query_architectural_decision(
        &self,
        ws: &str,
        id: &str,
    ) -> Option<ArchitecturalDecision> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[title, status, scope, date, rationale] := *architectural_decision{workspace: $ws, id: $id, state: 'desired', title, status, scope, date, rationale @ 'NOW'}",
            params_map(&[("ws", &ws), ("id", id)]),
            ScriptMutability::Immutable,
        ).ok()?.rows;
        let row = rows.first()?;
        Some(ArchitecturalDecision {
            id: id.to_string(),
            title: dv_str(&row[0]),
            status: match dv_str(&row[1]).as_str() {
                "accepted" => DecisionStatus::Accepted,
                "superseded" => DecisionStatus::Superseded,
                "deprecated" => DecisionStatus::Deprecated,
                _ => DecisionStatus::Proposed,
            },
            scope: dv_str(&row[2]),
            date: dv_str(&row[3]),
            rationale: dv_str(&row[4]),
            consequences: self.query_indexed_strings(
                "?[idx, text] := *decision_consequence{workspace: $ws, decision_id: $id, idx, state: 'desired', text @ 'NOW'}",
                params_map(&[("ws", &ws), ("id", id)]),
            ),
            contexts: self.query_indexed_strings(
                "?[idx, context] := *decision_context{workspace: $ws, decision_id: $id, idx, state: 'desired', context @ 'NOW'}",
                params_map(&[("ws", &ws), ("id", id)]),
            ),
            ownership: self.query_ownership(&ws, "", "architectural_decision", id, "desired"),
        })
    }

    pub fn upsert_context(
        &self,
        workspace_path: &str,
        name: &str,
        description: &str,
        module_path: &str,
        dependencies: &[String],
        ownership: &Ownership,
    ) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.ensure_project(workspace_path)?;
        self.run_script(
            "?[workspace, name, state, description, module_path] <- [[$ws, $name, 'desired', $desc, $mp]] :put context { workspace, name, state => description, module_path }",
            params_map(&[("ws", &ws), ("name", name), ("desc", description), ("mp", module_path)]),
            ScriptMutability::Mutable,
        ).map_err(|e| anyhow::anyhow!("upsert_context: {:?}", e))?;
        self.run_mutation_script(
            "?[workspace, from_ctx, to_ctx, state, vld] := *context_dep{workspace, from_ctx, to_ctx, state @ 'NOW'}, workspace = $ws, from_ctx = $name, state = 'desired', vld = 'RETRACT' :put context_dep { workspace, from_ctx, to_ctx, state, vld }",
            params_map(&[("ws", &ws), ("name", name)]),
            format!("retract context dependencies for {name}"),
        )?;
        for dep in dependencies {
            self.run_script(
                "?[workspace, from_ctx, to_ctx, state] <- [[$ws, $from, $to, 'desired']] :put context_dep { workspace, from_ctx, to_ctx, state }",
                params_map(&[("ws", &ws), ("from", name), ("to", dep)]),
                ScriptMutability::Mutable,
            ).map_err(|e| anyhow::anyhow!("upsert_context dep: {:?}", e))?;
        }
        self.save_owner_meta(&ws, name, "context", name, ownership, "desired")?;
        Ok(())
    }

    pub fn remove_context(&self, workspace_path: &str, name: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("name", name)]);
        let exists = self.run_script(
            "?[n] := *context{workspace: $ws, name: $name, state: 'desired' @ 'NOW'}, n = $name",
            p.clone(),
            ScriptMutability::Immutable,
        ).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.run_mutation_script(
            "?[workspace, from_ctx, to_ctx, state, vld] := *context_dep{workspace, from_ctx, to_ctx, state @ 'NOW'}, workspace = $ws, from_ctx = $name, state = 'desired', vld = 'RETRACT' :put context_dep { workspace, from_ctx, to_ctx, state, vld }",
            p.clone(),
            format!("remove outgoing context dependencies for {name}"),
        )?;
        self.run_mutation_script(
            "?[workspace, from_ctx, to_ctx, state, vld] := *context_dep{workspace, from_ctx, to_ctx, state @ 'NOW'}, workspace = $ws, to_ctx = $name, state = 'desired', vld = 'RETRACT' :put context_dep { workspace, from_ctx, to_ctx, state, vld }",
            p.clone(),
            format!("remove incoming context dependencies for {name}"),
        )?;
        self.remove_owner_meta(&ws, name, "context", name)?;
        self.run_script(
            "?[workspace, name, state, vld] := workspace = $ws, name = $name, state = 'desired', vld = 'RETRACT' :put context { workspace, name, state, vld }",
            p,
            ScriptMutability::Mutable,
        ).map_err(|e| anyhow::anyhow!("remove_context: {:?}", e))?;
        Ok(true)
    }

    pub fn upsert_entity(&self, workspace_path: &str, ctx: &str, entity: &Entity) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.ensure_project(workspace_path)?;
        let mut params = params_map(&[
            ("ws", &ws),
            ("ctx", ctx),
            ("name", &entity.name),
            ("desc", &entity.description),
        ]);
        params.insert(
            "aggregate_root".into(),
            cozo::DataValue::Bool(entity.aggregate_root),
        );
        params.insert(
            "file".into(),
            cozo::DataValue::Str(entity.file_path.as_deref().unwrap_or("").into()),
        );
        params.insert("sl".into(), int_dv(entity.start_line.unwrap_or(0) as i64));
        params.insert("el".into(), int_dv(entity.end_line.unwrap_or(0) as i64));
        self.run_script(
            "?[workspace, context, name, state, description, aggregate_root, file_path, start_line, end_line] <- [[$ws, $ctx, $name, 'desired', $desc, $aggregate_root, $file, $sl, $el]] :put entity { workspace, context, name, state => description, aggregate_root, file_path, start_line, end_line }",
            params,
            ScriptMutability::Mutable,
        ).map_err(|e| anyhow::anyhow!("upsert_entity: {:?}", e))?;
        self.replace_owner_fields(&ws, ctx, "entity", &entity.name, &entity.fields)?;
        self.replace_owner_methods(&ws, ctx, "entity", &entity.name, &entity.methods)?;
        self.replace_invariants(&ws, ctx, &entity.name, &entity.invariants)?;
        Ok(())
    }

    pub fn remove_entity(&self, workspace_path: &str, ctx: &str, name: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]);
        let exists = self.run_script(
            "?[n] := *entity{workspace: $ws, context: $ctx, name: $name, state: 'desired' @ 'NOW'}, n = $name",
            p.clone(),
            ScriptMutability::Immutable,
        ).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.run_mutation_script("?[workspace, context, owner_kind, owner, name, state, vld] := *field{workspace, context, owner_kind, owner, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = 'entity', owner = $name, state = 'desired', vld = 'RETRACT' :put field { workspace, context, owner_kind, owner, name, state, vld }", p.clone(), format!("remove entity fields for {name}"))?;
        self.run_mutation_script("?[workspace, context, owner_kind, owner, name, state, vld] := *method{workspace, context, owner_kind, owner, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = 'entity', owner = $name, state = 'desired', vld = 'RETRACT' :put method { workspace, context, owner_kind, owner, name, state, vld }", p.clone(), format!("remove entity methods for {name}"))?;
        self.run_mutation_script("?[workspace, context, owner_kind, owner, method, name, state, vld] := *method_param{workspace, context, owner_kind, owner, method, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = 'entity', owner = $name, state = 'desired', vld = 'RETRACT' :put method_param { workspace, context, owner_kind, owner, method, name, state, vld }", p.clone(), format!("remove entity method params for {name}"))?;
        self.run_mutation_script("?[workspace, context, entity, idx, state, text, vld] := *invariant{workspace, context, entity, idx, state, text @ 'NOW'}, workspace = $ws, context = $ctx, entity = $name, state = 'desired', vld = 'RETRACT' :put invariant { workspace, context, entity, idx, state, vld => text }", p.clone(), format!("remove entity invariants for {name}"))?;
        self.run_script("?[workspace, context, name, state, vld] := workspace = $ws, context = $ctx, name = $name, state = 'desired', vld = 'RETRACT' :put entity { workspace, context, name, state, vld }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("remove_entity: {:?}", e))?;
        Ok(true)
    }

    pub fn upsert_api_endpoint(
        &self,
        workspace_path: &str,
        ctx: &str,
        ep: &APIEndpoint,
    ) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.ensure_project(workspace_path)?;
        let params = params_map(&[
            ("ws", &ws),
            ("ctx", ctx),
            ("id", &ep.id),
            ("svc", &ep.service_id),
            ("met", &ep.method),
            ("path", &ep.route_pattern),
            ("desc", &ep.description),
        ]);
        self.run_script(
            "?[workspace, context, id, state, service_id, method, route_pattern, description] <- \
             [[$ws, $ctx, $id, 'desired', $svc, $met, $path, $desc]] :put api_endpoint { workspace, context, id, state => service_id, method, route_pattern, description }",
            params, ScriptMutability::Mutable
        ).map_err(|e| anyhow::anyhow!("upsert_api_endpoint: {:?}", e))?;
        Ok(())
    }

    pub fn remove_api_endpoint(&self, workspace_path: &str, ctx: &str, id: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let params = params_map(&[("ws", &ws), ("ctx", ctx), ("id", id)]);
        let _ = self.run_script(
            "?[workspace, context, id, state, vld] := *api_endpoint{workspace, context, id, state @ 'NOW'}, workspace = $ws, context = $ctx, id = $id, state = 'desired', vld = 'RETRACT' :put api_endpoint { workspace, context, id, state, vld }",
            params, ScriptMutability::Mutable
        ).map_err(|e| anyhow::anyhow!("remove_api_endpoint: {:?}", e))?;
        Ok(true)
    }

    pub fn query_api_endpoint(&self, ws: &str, ctx: &str, id: &str) -> Option<APIEndpoint> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[service_id, method, route_pattern, description] := *api_endpoint{workspace: $ws, context: $ctx, id: $id, state: 'desired', service_id, method, route_pattern, description @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("id", id)]),
            ScriptMutability::Immutable
        ).ok()?.rows;
        let row = rows.first()?;
        Some(APIEndpoint {
            id: id.to_string(),
            service_id: dv_str(&row[0]),
            method: dv_str(&row[1]),
            route_pattern: dv_str(&row[2]),
            description: dv_str(&row[3]),
        })
    }

    pub fn upsert_service(&self, workspace_path: &str, ctx: &str, service: &Service) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.ensure_project(workspace_path)?;
        let kind = match service.kind {
            ServiceKind::Application => "application",
            ServiceKind::Infrastructure => "infrastructure",
            ServiceKind::Domain => "domain",
        };
        let mut params = params_map(&[
            ("ws", &ws),
            ("ctx", ctx),
            ("name", &service.name),
            ("desc", &service.description),
            ("kind", kind),
        ]);
        params.insert(
            "file".into(),
            cozo::DataValue::Str(service.file_path.as_deref().unwrap_or("").into()),
        );
        params.insert("sl".into(), int_dv(service.start_line.unwrap_or(0) as i64));
        params.insert("el".into(), int_dv(service.end_line.unwrap_or(0) as i64));
        self.run_script(
            "?[workspace, context, name, state, description, kind, file_path, start_line, end_line] <- [[$ws, $ctx, $name, 'desired', $desc, $kind, $file, $sl, $el]] :put service { workspace, context, name, state => description, kind, file_path, start_line, end_line }",
            params,
            ScriptMutability::Mutable,
        ).map_err(|e| anyhow::anyhow!("upsert_service: {:?}", e))?;
        self.replace_owner_methods(&ws, ctx, "service", &service.name, &service.methods)?;
        self.replace_service_deps(&ws, ctx, &service.name, &service.dependencies)?;
        Ok(())
    }

    pub fn remove_service(&self, workspace_path: &str, ctx: &str, name: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]);
        let exists = self.run_script("?[n] := *service{workspace: $ws, context: $ctx, name: $name, state: 'desired' @ 'NOW'}, n = $name", p.clone(), ScriptMutability::Immutable).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.run_mutation_script("?[workspace, context, owner_kind, owner, name, state, vld] := *method{workspace, context, owner_kind, owner, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = 'service', owner = $name, state = 'desired', vld = 'RETRACT' :put method { workspace, context, owner_kind, owner, name, state, vld }", p.clone(), format!("remove service methods for {name}"))?;
        self.run_mutation_script("?[workspace, context, owner_kind, owner, method, name, state, vld] := *method_param{workspace, context, owner_kind, owner, method, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = 'service', owner = $name, state = 'desired', vld = 'RETRACT' :put method_param { workspace, context, owner_kind, owner, method, name, state, vld }", p.clone(), format!("remove service method params for {name}"))?;
        self.run_mutation_script("?[workspace, context, service, dep, state, vld] := *service_dep{workspace, context, service, dep, state @ 'NOW'}, workspace = $ws, context = $ctx, service = $name, state = 'desired', vld = 'RETRACT' :put service_dep { workspace, context, service, dep, state, vld }", p.clone(), format!("remove service dependencies for {name}"))?;
        self.run_script("?[workspace, context, name, state, vld] := workspace = $ws, context = $ctx, name = $name, state = 'desired', vld = 'RETRACT' :put service { workspace, context, name, state, vld }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("remove_service: {:?}", e))?;
        Ok(true)
    }

    pub fn upsert_event(&self, workspace_path: &str, ctx: &str, event: &DomainEvent) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.ensure_project(workspace_path)?;
        let mut params = params_map(&[
            ("ws", &ws),
            ("ctx", ctx),
            ("name", &event.name),
            ("desc", &event.description),
            ("source", &event.source),
        ]);
        params.insert(
            "file".into(),
            cozo::DataValue::Str(event.file_path.as_deref().unwrap_or("").into()),
        );
        params.insert("sl".into(), int_dv(event.start_line.unwrap_or(0) as i64));
        params.insert("el".into(), int_dv(event.end_line.unwrap_or(0) as i64));
        self.run_script(
            "?[workspace, context, name, state, description, source, file_path, start_line, end_line] <- [[$ws, $ctx, $name, 'desired', $desc, $source, $file, $sl, $el]] :put event { workspace, context, name, state => description, source, file_path, start_line, end_line }",
            params,
            ScriptMutability::Mutable,
        ).map_err(|e| anyhow::anyhow!("upsert_event: {:?}", e))?;
        self.replace_owner_fields(&ws, ctx, "event", &event.name, &event.fields)?;
        Ok(())
    }

    pub fn remove_event(&self, workspace_path: &str, ctx: &str, name: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]);
        let exists = self.run_script("?[n] := *event{workspace: $ws, context: $ctx, name: $name, state: 'desired' @ 'NOW'}, n = $name", p.clone(), ScriptMutability::Immutable).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.run_mutation_script("?[workspace, context, owner_kind, owner, name, state, vld] := *field{workspace, context, owner_kind, owner, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = 'event', owner = $name, state = 'desired', vld = 'RETRACT' :put field { workspace, context, owner_kind, owner, name, state, vld }", p.clone(), format!("remove event fields for {name}"))?;
        self.run_script("?[workspace, context, name, state, vld] := workspace = $ws, context = $ctx, name = $name, state = 'desired', vld = 'RETRACT' :put event { workspace, context, name, state, vld }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("remove_event: {:?}", e))?;
        Ok(true)
    }

    pub fn upsert_value_object(
        &self,
        workspace_path: &str,
        ctx: &str,
        value_object: &ValueObject,
    ) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.ensure_project(workspace_path)?;
        let mut params = params_map(&[
            ("ws", &ws),
            ("ctx", ctx),
            ("name", &value_object.name),
            ("desc", &value_object.description),
        ]);
        params.insert(
            "file".into(),
            cozo::DataValue::Str(value_object.file_path.as_deref().unwrap_or("").into()),
        );
        params.insert(
            "sl".into(),
            int_dv(value_object.start_line.unwrap_or(0) as i64),
        );
        params.insert(
            "el".into(),
            int_dv(value_object.end_line.unwrap_or(0) as i64),
        );
        self.run_script(
            "?[workspace, context, name, state, description, file_path, start_line, end_line] <- [[$ws, $ctx, $name, 'desired', $desc, $file, $sl, $el]] :put value_object { workspace, context, name, state => description, file_path, start_line, end_line }",
            params,
            ScriptMutability::Mutable,
        ).map_err(|e| anyhow::anyhow!("upsert_value_object: {:?}", e))?;
        self.replace_owner_fields(
            &ws,
            ctx,
            "value_object",
            &value_object.name,
            &value_object.fields,
        )?;
        self.replace_vo_rules(&ws, ctx, &value_object.name, &value_object.validation_rules)?;
        Ok(())
    }

    pub fn remove_value_object(&self, workspace_path: &str, ctx: &str, name: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]);
        let exists = self.run_script("?[n] := *value_object{workspace: $ws, context: $ctx, name: $name, state: 'desired' @ 'NOW'}, n = $name", p.clone(), ScriptMutability::Immutable).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.run_mutation_script("?[workspace, context, owner_kind, owner, name, state, vld] := *field{workspace, context, owner_kind, owner, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = 'value_object', owner = $name, state = 'desired', vld = 'RETRACT' :put field { workspace, context, owner_kind, owner, name, state, vld }", p.clone(), format!("remove value object fields for {name}"))?;
        self.run_mutation_script("?[workspace, context, value_object, idx, state, text, vld] := *vo_rule{workspace, context, value_object, idx, state, text @ 'NOW'}, workspace = $ws, context = $ctx, value_object = $name, state = 'desired', vld = 'RETRACT' :put vo_rule { workspace, context, value_object, idx, state, vld => text }", p.clone(), format!("remove value object rules for {name}"))?;
        self.run_script("?[workspace, context, name, state, vld] := workspace = $ws, context = $ctx, name = $name, state = 'desired', vld = 'RETRACT' :put value_object { workspace, context, name, state, vld }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("remove_value_object: {:?}", e))?;
        Ok(true)
    }

    pub fn upsert_repository(
        &self,
        workspace_path: &str,
        ctx: &str,
        repository: &Repository,
    ) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.ensure_project(workspace_path)?;
        let mut params = params_map(&[
            ("ws", &ws),
            ("ctx", ctx),
            ("name", &repository.name),
            ("aggregate", &repository.aggregate),
        ]);
        params.insert(
            "file".into(),
            cozo::DataValue::Str(repository.file_path.as_deref().unwrap_or("").into()),
        );
        params.insert(
            "sl".into(),
            int_dv(repository.start_line.unwrap_or(0) as i64),
        );
        params.insert("el".into(), int_dv(repository.end_line.unwrap_or(0) as i64));
        self.run_script(
            "?[workspace, context, name, state, aggregate, file_path, start_line, end_line] <- [[$ws, $ctx, $name, 'desired', $aggregate, $file, $sl, $el]] :put repository { workspace, context, name, state => aggregate, file_path, start_line, end_line }",
            params,
            ScriptMutability::Mutable,
        ).map_err(|e| anyhow::anyhow!("upsert_repository: {:?}", e))?;
        self.replace_owner_methods(
            &ws,
            ctx,
            "repository",
            &repository.name,
            &repository.methods,
        )?;
        Ok(())
    }

    pub fn remove_repository(&self, workspace_path: &str, ctx: &str, name: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]);
        let exists = self.run_script("?[n] := *repository{workspace: $ws, context: $ctx, name: $name, state: 'desired' @ 'NOW'}, n = $name", p.clone(), ScriptMutability::Immutable).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.run_mutation_script("?[workspace, context, owner_kind, owner, name, state, vld] := *method{workspace, context, owner_kind, owner, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = 'repository', owner = $name, state = 'desired', vld = 'RETRACT' :put method { workspace, context, owner_kind, owner, name, state, vld }", p.clone(), format!("remove repository methods for {name}"))?;
        self.run_mutation_script("?[workspace, context, owner_kind, owner, method, name, state, vld] := *method_param{workspace, context, owner_kind, owner, method, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = 'repository', owner = $name, state = 'desired', vld = 'RETRACT' :put method_param { workspace, context, owner_kind, owner, method, name, state, vld }", p.clone(), format!("remove repository method params for {name}"))?;
        self.run_script("?[workspace, context, name, state, vld] := workspace = $ws, context = $ctx, name = $name, state = 'desired', vld = 'RETRACT' :put repository { workspace, context, name, state, vld }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("remove_repository: {:?}", e))?;
        Ok(true)
    }

    pub fn query_module(&self, ws: &str, ctx: &str, name: &str) -> Option<Module> {
        let ws = canonicalize_path(ws);
        let rows = self.run_script(
            "?[path, public, file_path, description] := *module{workspace: $ws, context: $ctx, name: $name, state: 'desired', path, public, file_path, description @ 'NOW'}",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]),
            ScriptMutability::Immutable,
        ).ok()?.rows;
        let row = rows.first()?;
        Some(Module {
            name: name.to_string(),
            path: dv_str(&row[0]),
            public: matches!(&row[1], cozo::DataValue::Bool(true)),
            file_path: dv_str(&row[2]),
            description: dv_str(&row[3]),
        })
    }

    pub fn upsert_module(&self, workspace_path: &str, ctx: &str, module: &Module) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.ensure_project(workspace_path)?;
        let mut params = params_map(&[
            ("ws", &ws),
            ("ctx", ctx),
            ("name", &module.name),
            ("path", &module.path),
            ("fp", &module.file_path),
            ("desc", &module.description),
        ]);
        params.insert("public".into(), cozo::DataValue::Bool(module.public));
        self.run_script(
            "?[workspace, context, name, state, path, public, file_path, description] <- [[$ws, $ctx, $name, 'desired', $path, $public, $fp, $desc]] :put module { workspace, context, name, state => path, public, file_path, description }",
            params,
            ScriptMutability::Mutable,
        ).map_err(|e| anyhow::anyhow!("upsert_module: {:?}", e))?;
        Ok(())
    }

    pub fn remove_module(&self, workspace_path: &str, ctx: &str, name: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]);
        let exists = self.run_script(
            "?[n] := *module{workspace: $ws, context: $ctx, name: $name, state: 'desired' @ 'NOW'}, n = $name",
            p.clone(),
            ScriptMutability::Immutable,
        ).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.run_script(
            "?[workspace, context, name, state, vld] := workspace = $ws, context = $ctx, name = $name, state = 'desired', vld = 'RETRACT' :put module { workspace, context, name, state, vld }",
            p,
            ScriptMutability::Mutable,
        ).map_err(|e| anyhow::anyhow!("remove_module: {:?}", e))?;
        Ok(true)
    }

    pub fn upsert_aggregate(
        &self,
        workspace_path: &str,
        ctx: &str,
        aggregate: &Aggregate,
    ) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.run_script(
            "?[workspace, context, name, state, description, root_entity] <- [[$ws, $ctx, $name, 'desired', $desc, $root]] :put aggregate { workspace, context, name, state => description, root_entity }",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", &aggregate.name), ("desc", &aggregate.description), ("root", &aggregate.root_entity)]),
            ScriptMutability::Mutable,
        ).map_err(|e| anyhow::anyhow!("upsert_aggregate: {:?}", e))?;
        self.save_owner_meta(
            &ws,
            ctx,
            "aggregate",
            &aggregate.name,
            &aggregate.ownership,
            "desired",
        )?;
        self.run_mutation_script(
            "?[workspace, context, aggregate, member_kind, member, state, vld] := *aggregate_member{workspace, context, aggregate, member_kind, member, state @ 'NOW'}, workspace = $ws, context = $ctx, aggregate = $name, state = 'desired', vld = 'RETRACT' :put aggregate_member { workspace, context, aggregate, member_kind, member, state, vld }",
            params_map(&[("ws", &ws), ("ctx", ctx), ("name", &aggregate.name)]),
            format!("retract aggregate members for {}", aggregate.name),
        )?;
        for entity in &aggregate.entities {
            self.run_script("?[workspace, context, aggregate, member_kind, member, state] <- [[$ws, $ctx, $name, 'entity', $member, 'desired']] :put aggregate_member { workspace, context, aggregate, member_kind, member, state }", params_map(&[("ws", &ws), ("ctx", ctx), ("name", &aggregate.name), ("member", entity)]), ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("upsert_aggregate entity: {:?}", e))?;
        }
        for vo in &aggregate.value_objects {
            self.run_script("?[workspace, context, aggregate, member_kind, member, state] <- [[$ws, $ctx, $name, 'value_object', $member, 'desired']] :put aggregate_member { workspace, context, aggregate, member_kind, member, state }", params_map(&[("ws", &ws), ("ctx", ctx), ("name", &aggregate.name), ("member", vo)]), ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("upsert_aggregate vo: {:?}", e))?;
        }
        Ok(())
    }

    pub fn remove_aggregate(&self, workspace_path: &str, ctx: &str, name: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]);
        let exists = self.run_script("?[n] := *aggregate{workspace: $ws, context: $ctx, name: $name, state: 'desired' @ 'NOW'}, n = $name", p.clone(), ScriptMutability::Immutable).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.run_mutation_script("?[workspace, context, aggregate, member_kind, member, state, vld] := *aggregate_member{workspace, context, aggregate, member_kind, member, state @ 'NOW'}, workspace = $ws, context = $ctx, aggregate = $name, state = 'desired', vld = 'RETRACT' :put aggregate_member { workspace, context, aggregate, member_kind, member, state, vld }", p.clone(), format!("remove aggregate members for {name}"))?;
        self.remove_owner_meta(&ws, ctx, "aggregate", name)?;
        self.run_script("?[workspace, context, name, state, vld] := workspace = $ws, context = $ctx, name = $name, state = 'desired', vld = 'RETRACT' :put aggregate { workspace, context, name, state, vld }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("remove_aggregate: {:?}", e))?;
        Ok(true)
    }

    pub fn upsert_policy(&self, workspace_path: &str, ctx: &str, policy: &Policy) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        let kind = Self::policy_kind_key(&policy.kind).to_string();
        self.run_script("?[workspace, context, name, state, description, kind] <- [[$ws, $ctx, $name, 'desired', $desc, $kind]] :put policy { workspace, context, name, state => description, kind }", params_map(&[("ws", &ws), ("ctx", ctx), ("name", &policy.name), ("desc", &policy.description), ("kind", &kind)]), ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("upsert_policy: {:?}", e))?;
        self.save_owner_meta(
            &ws,
            ctx,
            "policy",
            &policy.name,
            &policy.ownership,
            "desired",
        )?;
        self.run_mutation_script("?[workspace, context, policy, link_kind, link, idx, state, vld] := *policy_link{workspace, context, policy, link_kind, link, idx, state @ 'NOW'}, workspace = $ws, context = $ctx, policy = $name, state = 'desired', vld = 'RETRACT' :put policy_link { workspace, context, policy, link_kind, link, idx, state, vld }", params_map(&[("ws", &ws), ("ctx", ctx), ("name", &policy.name)]), format!("retract policy links for {}", policy.name))?;
        for (idx, trigger) in policy.triggers.iter().enumerate() {
            let mut p = params_map(&[
                ("ws", &ws),
                ("ctx", ctx),
                ("name", &policy.name),
                ("link", trigger),
            ]);
            p.insert("idx".into(), int_dv(idx as i64));
            self.run_script("?[workspace, context, policy, link_kind, link, idx, state] <- [[$ws, $ctx, $name, 'trigger', $link, $idx, 'desired']] :put policy_link { workspace, context, policy, link_kind, link, idx, state }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("upsert_policy trigger: {:?}", e))?;
        }
        for (idx, command) in policy.commands.iter().enumerate() {
            let mut p = params_map(&[
                ("ws", &ws),
                ("ctx", ctx),
                ("name", &policy.name),
                ("link", command),
            ]);
            p.insert("idx".into(), int_dv(idx as i64));
            self.run_script("?[workspace, context, policy, link_kind, link, idx, state] <- [[$ws, $ctx, $name, 'command', $link, $idx, 'desired']] :put policy_link { workspace, context, policy, link_kind, link, idx, state }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("upsert_policy command: {:?}", e))?;
        }
        Ok(())
    }

    pub fn remove_policy(&self, workspace_path: &str, ctx: &str, name: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]);
        let exists = self.run_script("?[n] := *policy{workspace: $ws, context: $ctx, name: $name, state: 'desired' @ 'NOW'}, n = $name", p.clone(), ScriptMutability::Immutable).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.run_mutation_script("?[workspace, context, policy, link_kind, link, idx, state, vld] := *policy_link{workspace, context, policy, link_kind, link, idx, state @ 'NOW'}, workspace = $ws, context = $ctx, policy = $name, state = 'desired', vld = 'RETRACT' :put policy_link { workspace, context, policy, link_kind, link, idx, state, vld }", p.clone(), format!("remove policy links for {name}"))?;
        self.remove_owner_meta(&ws, ctx, "policy", name)?;
        self.run_script("?[workspace, context, name, state, vld] := workspace = $ws, context = $ctx, name = $name, state = 'desired', vld = 'RETRACT' :put policy { workspace, context, name, state, vld }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("remove_policy: {:?}", e))?;
        Ok(true)
    }

    pub fn upsert_read_model(
        &self,
        workspace_path: &str,
        ctx: &str,
        read_model: &ReadModel,
    ) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.run_script("?[workspace, context, name, state, description, source] <- [[$ws, $ctx, $name, 'desired', $desc, $src]] :put read_model { workspace, context, name, state => description, source }", params_map(&[("ws", &ws), ("ctx", ctx), ("name", &read_model.name), ("desc", &read_model.description), ("src", &read_model.source)]), ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("upsert_read_model: {:?}", e))?;
        self.save_owner_meta(
            &ws,
            ctx,
            "read_model",
            &read_model.name,
            &read_model.ownership,
            "desired",
        )?;
        self.replace_owner_fields(&ws, ctx, "read_model", &read_model.name, &read_model.fields)?;
        Ok(())
    }

    pub fn remove_read_model(&self, workspace_path: &str, ctx: &str, name: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("ctx", ctx), ("name", name)]);
        let exists = self.run_script("?[n] := *read_model{workspace: $ws, context: $ctx, name: $name, state: 'desired' @ 'NOW'}, n = $name", p.clone(), ScriptMutability::Immutable).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.remove_owner_meta(&ws, ctx, "read_model", name)?;
        self.run_mutation_script("?[workspace, context, owner_kind, owner, name, state, vld] := *field{workspace, context, owner_kind, owner, name, state @ 'NOW'}, workspace = $ws, context = $ctx, owner_kind = 'read_model', owner = $name, state = 'desired', vld = 'RETRACT' :put field { workspace, context, owner_kind, owner, name, state, vld }", p.clone(), format!("remove read model fields for {name}"))?;
        self.run_script("?[workspace, context, name, state, vld] := workspace = $ws, context = $ctx, name = $name, state = 'desired', vld = 'RETRACT' :put read_model { workspace, context, name, state, vld }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("remove_read_model: {:?}", e))?;
        Ok(true)
    }

    pub fn upsert_external_system(
        &self,
        workspace_path: &str,
        system: &ExternalSystem,
    ) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.run_script("?[workspace, name, state, description, kind, rationale] <- [[$ws, $name, 'desired', $desc, $kind, $rationale]] :put external_system { workspace, name, state => description, kind, rationale }", params_map(&[("ws", &ws), ("name", &system.name), ("desc", &system.description), ("kind", &system.kind), ("rationale", &system.rationale)]), ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("upsert_external_system: {:?}", e))?;
        self.save_owner_meta(
            &ws,
            "",
            "external_system",
            &system.name,
            &system.ownership,
            "desired",
        )?;
        self.run_mutation_script("?[workspace, system, context, idx, state, vld] := *external_system_context{workspace, system, context, idx, state @ 'NOW'}, workspace = $ws, system = $name, state = 'desired', vld = 'RETRACT' :put external_system_context { workspace, system, context, idx, state, vld }", params_map(&[("ws", &ws), ("name", &system.name)]), format!("retract external system contexts for {}", system.name))?;
        for (idx, ctx) in system.consumed_by_contexts.iter().enumerate() {
            let mut p = params_map(&[("ws", &ws), ("name", &system.name), ("ctx", ctx)]);
            p.insert("idx".into(), int_dv(idx as i64));
            self.run_script("?[workspace, system, context, idx, state] <- [[$ws, $name, $ctx, $idx, 'desired']] :put external_system_context { workspace, system, context, idx, state }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("upsert_external_system ctx: {:?}", e))?;
        }
        Ok(())
    }

    pub fn remove_external_system(&self, workspace_path: &str, name: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("name", name)]);
        let exists = self.run_script("?[n] := *external_system{workspace: $ws, name: $name, state: 'desired' @ 'NOW'}, n = $name", p.clone(), ScriptMutability::Immutable).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.remove_owner_meta(&ws, "", "external_system", name)?;
        self.run_mutation_script("?[workspace, system, context, idx, state, vld] := *external_system_context{workspace, system, context, idx, state @ 'NOW'}, workspace = $ws, system = $name, state = 'desired', vld = 'RETRACT' :put external_system_context { workspace, system, context, idx, state, vld }", p.clone(), format!("remove external system contexts for {name}"))?;
        self.run_script("?[workspace, name, state, vld] := workspace = $ws, name = $name, state = 'desired', vld = 'RETRACT' :put external_system { workspace, name, state, vld }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("remove_external_system: {:?}", e))?;
        Ok(true)
    }

    pub fn upsert_architectural_decision(
        &self,
        workspace_path: &str,
        decision: &ArchitecturalDecision,
    ) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        let status = format!("{:?}", decision.status).to_lowercase();
        self.run_script("?[workspace, id, state, title, status, scope, date, rationale] <- [[$ws, $id, 'desired', $title, $status, $scope, $date, $rationale]] :put architectural_decision { workspace, id, state => title, status, scope, date, rationale }", params_map(&[("ws", &ws), ("id", &decision.id), ("title", &decision.title), ("status", &status), ("scope", &decision.scope), ("date", &decision.date), ("rationale", &decision.rationale)]), ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("upsert_architectural_decision: {:?}", e))?;
        self.save_owner_meta(
            &ws,
            "",
            "architectural_decision",
            &decision.id,
            &decision.ownership,
            "desired",
        )?;
        self.run_mutation_script("?[workspace, decision_id, context, idx, state, vld] := *decision_context{workspace, decision_id, context, idx, state @ 'NOW'}, workspace = $ws, decision_id = $id, state = 'desired', vld = 'RETRACT' :put decision_context { workspace, decision_id, context, idx, state, vld }", params_map(&[("ws", &ws), ("id", &decision.id)]), format!("retract decision contexts for {}", decision.id))?;
        self.run_mutation_script("?[workspace, decision_id, idx, state, vld] := *decision_consequence{workspace, decision_id, idx, state @ 'NOW'}, workspace = $ws, decision_id = $id, state = 'desired', vld = 'RETRACT' :put decision_consequence { workspace, decision_id, idx, state, vld }", params_map(&[("ws", &ws), ("id", &decision.id)]), format!("retract decision consequences for {}", decision.id))?;
        for (idx, ctx) in decision.contexts.iter().enumerate() {
            let mut p = params_map(&[("ws", &ws), ("id", &decision.id), ("ctx", ctx)]);
            p.insert("idx".into(), int_dv(idx as i64));
            self.run_script("?[workspace, decision_id, context, idx, state] <- [[$ws, $id, $ctx, $idx, 'desired']] :put decision_context { workspace, decision_id, context, idx, state }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("upsert_architectural_decision ctx: {:?}", e))?;
        }
        for (idx, consequence) in decision.consequences.iter().enumerate() {
            let mut p = params_map(&[("ws", &ws), ("id", &decision.id), ("text", consequence)]);
            p.insert("idx".into(), int_dv(idx as i64));
            self.run_script("?[workspace, decision_id, idx, state, text] <- [[$ws, $id, $idx, 'desired', $text]] :put decision_consequence { workspace, decision_id, idx, state => text }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("upsert_architectural_decision consequence: {:?}", e))?;
        }
        Ok(())
    }

    pub fn remove_architectural_decision(&self, workspace_path: &str, id: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace_path);
        let p = params_map(&[("ws", &ws), ("id", id)]);
        let exists = self.run_script("?[n] := *architectural_decision{workspace: $ws, id: $id, state: 'desired' @ 'NOW'}, n = $id", p.clone(), ScriptMutability::Immutable).map(|r| !r.rows.is_empty()).unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        self.remove_owner_meta(&ws, "", "architectural_decision", id)?;
        self.run_mutation_script("?[workspace, decision_id, context, idx, state, vld] := *decision_context{workspace, decision_id, context, idx, state @ 'NOW'}, workspace = $ws, decision_id = $id, state = 'desired', vld = 'RETRACT' :put decision_context { workspace, decision_id, context, idx, state, vld }", p.clone(), format!("remove decision contexts for {id}"))?;
        self.run_mutation_script("?[workspace, decision_id, idx, state, vld] := *decision_consequence{workspace, decision_id, idx, state @ 'NOW'}, workspace = $ws, decision_id = $id, state = 'desired', vld = 'RETRACT' :put decision_consequence { workspace, decision_id, idx, state, vld }", p.clone(), format!("remove decision consequences for {id}"))?;
        self.run_script("?[workspace, id, state, vld] := workspace = $ws, id = $id, state = 'desired', vld = 'RETRACT' :put architectural_decision { workspace, id, state, vld }", p, ScriptMutability::Mutable).map_err(|e| anyhow::anyhow!("remove_architectural_decision: {:?}", e))?;
        Ok(true)
    }

    // ── Project Operations ─────────────────────────────────────────────────

    /// List all stored projects.
    pub fn list(&self) -> Result<Vec<ProjectInfo>> {
        let result = self
            .run_script(
                "?[workspace, name, updated_at] := *project{workspace, name, updated_at}",
                Default::default(),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("Failed to list projects: {:?}", e))?;

        let mut projects: Vec<ProjectInfo> = result
            .rows
            .iter()
            .map(|r| ProjectInfo {
                workspace_path: dv_str(&r[0]),
                project_name: dv_str(&r[1]),
                updated_at: dv_str(&r[2]),
            })
            .collect();
        projects.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(projects)
    }

    /// Export a domain model to a JSON file.
    /// `state` can be `"actual"`, `"both"`, or a compatibility alias such as `"desired"`.
    pub fn export_to_file(&self, workspace_path: &str, file_path: &str, state: &str) -> Result<()> {
        let json = match state {
            "actual" => {
                let model = self.load_actual(workspace_path)?.with_context(|| {
                    format!("No actual model found for workspace: {workspace_path}")
                })?;
                serde_json::to_string_pretty(&model)?
            }
            "both" => {
                let desired = self.load_desired(workspace_path)?;
                let actual = self.load_actual(workspace_path)?;
                serde_json::to_string_pretty(&serde_json::json!({
                    "desired": desired,
                    "actual": actual
                }))?
            }
            _ => {
                let model = self.load_desired(workspace_path)?.with_context(|| {
                    format!("No implemented model found for workspace: {workspace_path}")
                })?;
                serde_json::to_string_pretty(&model)?
            }
        };
        std::fs::write(file_path, json)
            .with_context(|| format!("Failed to write file: {file_path}"))?;
        Ok(())
    }

    // ── Temporal Differencing ──────────────────────────────────────────────

    /// Compute the diff between the two most recent actual graph snapshots.
    pub fn diff_graph(&self, workspace_path: &str) -> Result<serde_json::Value> {
        let snapshots = self.list_snapshots(workspace_path, "actual")?;
        if snapshots.len() < 2 {
            return Ok(json!({
                "basis": "actual_history",
                "pending_changes": [],
                "summary": {
                    "total_changes": 0,
                    "additions": 0,
                    "removals": 0
                }
            }));
        }

        let ts_new = snapshots[0];
        let ts_old = snapshots[1];
        let diff = self.diff_snapshots(workspace_path, "actual", ts_old, ts_new)?;
        let mut pending_changes = Vec::new();
        if let Some(added) = diff.get("added").and_then(Value::as_array) {
            pending_changes.extend(added.iter().cloned());
        }
        if let Some(removed) = diff.get("removed").and_then(Value::as_array) {
            pending_changes.extend(removed.iter().cloned());
        }
        let total_changes = pending_changes.len();

        Ok(json!({
            "basis": "actual_history",
            "ts_old": ts_old,
            "ts_new": ts_new,
            "pending_changes": pending_changes,
            "summary": diff.get("summary").cloned().unwrap_or_else(|| json!({
                "total_changes": total_changes,
                "additions": 0,
                "removals": 0
            })),
            "added": diff.get("added").cloned().unwrap_or_else(|| json!([])),
            "removed": diff.get("removed").cloned().unwrap_or_else(|| json!([])),
        }))
    }

    /// Persist the latest actual-history diff to the drift relation.
    pub fn compute_drift(&self, workspace_path: &str) -> Result<usize> {
        let ws = canonicalize_path(workspace_path);
        let params = params_map(&[("ws", &ws)]);

        // 1. Retract previous drift entries
        self.run_mutation_script(
            "?[workspace, category, context, name, change_type, vld] := \
             *drift{workspace, category, context, name, change_type @ 'NOW'}, workspace = $ws, vld = 'RETRACT' \
             :put drift { workspace, category, context, name, change_type, vld }",
            params.clone(),
            format!("compute_drift retract previous drift entries for '{ws}'"),
        )?;

        // 2. Persist the most recent temporal diff as drift entries.
        let diff = self.diff_graph(workspace_path)?;
        let changes = diff
            .get("pending_changes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for change in &changes {
            let kind = change.get("kind").and_then(Value::as_str).unwrap_or("");
            let action = change.get("action").and_then(Value::as_str).unwrap_or("");
            let context = change.get("context").and_then(Value::as_str).unwrap_or("");
            let name = change.get("name").and_then(Value::as_str).unwrap_or("");
            let drift_params = params_map(&[
                ("ws", &ws),
                ("category", kind),
                ("ctx", context),
                ("name", name),
                ("change", action),
            ]);
            self.run_mutation_script(
                "?[workspace, category, context, name, change_type] <- [[$ws, $category, $ctx, $name, $change]] \
                 :put drift { workspace, category, context, name, change_type }",
                drift_params,
                format!("compute_drift insert {kind}:{name}"),
            )?;
        }

        let drift_ts_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as i64;
        let mut meta_params = params_map(&[("ws", &ws)]);
        meta_params.insert("ts".into(), int_dv(drift_ts_us));
        self.run_mutation_script(
            "?[workspace, computed_at_us] <- [[$ws, $ts]] :put drift_meta { workspace => computed_at_us }",
            meta_params,
            format!("compute_drift update drift_meta for '{ws}'"),
        )?;

        self.invalidate_reasoning_claims_for_dependency(&ws, "drift")?;

        Ok(changes.len())
    }

    /// Load current drift entries for a workspace.
    pub fn load_drift(
        &self,
        workspace_path: &str,
    ) -> Result<Vec<(String, String, String, String)>> {
        let ws = canonicalize_path(workspace_path);
        let params = params_map(&[("ws", &ws)]);
        let result = self
            .run_script(
                "?[category, context, name, change_type] := \
             *drift{workspace: $ws, category, context, name, change_type @ 'NOW'}",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("load_drift: {:?}", e))?;
        Ok(result
            .rows
            .iter()
            .map(|r| (dv_str(&r[0]), dv_str(&r[1]), dv_str(&r[2]), dv_str(&r[3])))
            .collect())
    }

    /// Load the timestamp of the most recent persisted drift computation.
    pub fn load_drift_recomputed_at(&self, workspace_path: &str) -> Result<Option<i64>> {
        let ws = canonicalize_path(workspace_path);
        let params = params_map(&[("ws", &ws)]);
        let result = self
            .run_script(
                "?[computed_at_us] := *drift_meta{workspace: $ws, computed_at_us}",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("load_drift_recomputed_at: {:?}", e))?;
        Ok(result.rows.first().map(|row| dv_i64(&row[0])))
    }

    /// Report the current truth-maintenance state for implemented graph and drift facts.
    pub fn truth_maintenance_report(&self, workspace_path: &str) -> Result<TruthMaintenanceReport> {
        let actual = self.load_actual(workspace_path)?;
        let actual_snapshot = self
            .list_snapshots(workspace_path, "actual")?
            .into_iter()
            .next();
        let drift_computed_at_us = self.load_drift_recomputed_at(workspace_path)?;
        let drift_entries = self.load_drift(workspace_path)?;

        let asserted =
            summarize_fact_snapshot("implemented", "actual", actual_snapshot, actual.as_ref());
        let scanned =
            summarize_fact_snapshot("scanned", "actual", actual_snapshot, actual.as_ref());

        let basis_timestamp_us = actual_snapshot;

        let drift_status = match (basis_timestamp_us, drift_computed_at_us) {
            (Some(basis_ts), Some(computed_at_us)) if computed_at_us >= basis_ts => "fresh",
            (Some(_), Some(_)) => "stale",
            _ => "unavailable",
        };

        let mut assumptions = Vec::new();
        if !asserted.available {
            assumptions.push(
                "No implemented architecture graph is stored; run a scan before requesting proofs."
                    .to_string(),
            );
        }
        if !scanned.available {
            assumptions.push(
                "No scanned implementation model is stored; proofs about actual code structure are incomplete."
                    .to_string(),
            );
        }
        match drift_status {
            "stale" => assumptions.push(
                "Persisted drift entries predate the latest asserted or scanned snapshot and may be stale."
                    .to_string(),
            ),
            "unavailable" if basis_timestamp_us.is_some() => assumptions.push(
                "Drift has not been recomputed for the current asserted/scanned basis."
                    .to_string(),
            ),
            _ => {}
        }

        Ok(TruthMaintenanceReport {
            asserted,
            scanned,
            drift: DriftFreshness {
                available: basis_timestamp_us.is_some() && drift_computed_at_us.is_some(),
                status: drift_status.to_string(),
                computed_at_us: drift_computed_at_us,
                basis_timestamp_us,
                entry_count: drift_entries.len(),
            },
            assumptions,
        })
    }

    fn clear_reasoning_claims(&self, workspace: &str) -> Result<()> {
        let scripts = [
            (
                "reasoning_derivation",
                "?[workspace, claim_id, idx] := *reasoning_derivation{workspace, claim_id, idx}, workspace = $ws :rm reasoning_derivation { workspace, claim_id, idx }",
            ),
            (
                "reasoning_assumption",
                "?[workspace, claim_id, idx] := *reasoning_assumption{workspace, claim_id, idx}, workspace = $ws :rm reasoning_assumption { workspace, claim_id, idx }",
            ),
            (
                "reasoning_support",
                "?[workspace, claim_id, idx] := *reasoning_support{workspace, claim_id, idx}, workspace = $ws :rm reasoning_support { workspace, claim_id, idx }",
            ),
            (
                "reasoning_dependency",
                "?[workspace, claim_id, idx] := *reasoning_dependency{workspace, claim_id, idx}, workspace = $ws :rm reasoning_dependency { workspace, claim_id, idx }",
            ),
            (
                "reasoning_justification",
                "?[workspace, claim_id, idx] := *reasoning_justification{workspace, claim_id, idx}, workspace = $ws :rm reasoning_justification { workspace, claim_id, idx }",
            ),
            (
                "reasoning_claim",
                "?[workspace, claim_id] := *reasoning_claim{workspace, claim_id}, workspace = $ws :rm reasoning_claim { workspace, claim_id }",
            ),
        ];

        for (relation, script) in scripts {
            self.run_mutation_script(
                script,
                params_map(&[("ws", workspace)]),
                format!("clear {relation} rows for '{workspace}'"),
            )?;
        }

        Ok(())
    }

    fn clear_reasoning_claim_ids(&self, workspace: &str, claim_ids: &[String]) -> Result<()> {
        if claim_ids.is_empty() {
            return Ok(());
        }

        let scripts = [
            (
                "reasoning_derivation",
                "?[workspace, claim_id, idx] := *reasoning_derivation{workspace, claim_id, idx}, workspace = $ws, claim_id = $claim_id :rm reasoning_derivation { workspace, claim_id, idx }",
            ),
            (
                "reasoning_assumption",
                "?[workspace, claim_id, idx] := *reasoning_assumption{workspace, claim_id, idx}, workspace = $ws, claim_id = $claim_id :rm reasoning_assumption { workspace, claim_id, idx }",
            ),
            (
                "reasoning_support",
                "?[workspace, claim_id, idx] := *reasoning_support{workspace, claim_id, idx}, workspace = $ws, claim_id = $claim_id :rm reasoning_support { workspace, claim_id, idx }",
            ),
            (
                "reasoning_dependency",
                "?[workspace, claim_id, idx] := *reasoning_dependency{workspace, claim_id, idx}, workspace = $ws, claim_id = $claim_id :rm reasoning_dependency { workspace, claim_id, idx }",
            ),
            (
                "reasoning_justification",
                "?[workspace, claim_id, idx] := *reasoning_justification{workspace, claim_id, idx}, workspace = $ws, claim_id = $claim_id :rm reasoning_justification { workspace, claim_id, idx }",
            ),
            (
                "reasoning_claim",
                "?[workspace, claim_id] := *reasoning_claim{workspace, claim_id}, workspace = $ws, claim_id = $claim_id :rm reasoning_claim { workspace, claim_id }",
            ),
        ];

        for claim_id in claim_ids {
            let params = params_map(&[("ws", workspace), ("claim_id", claim_id)]);
            for (relation, script) in scripts {
                self.run_mutation_script(
                    script,
                    params.clone(),
                    format!("clear {relation} rows for '{workspace}' claim '{claim_id}'"),
                )?;
            }
        }

        Ok(())
    }

    fn write_reasoning_claims(
        &self,
        workspace: &str,
        claims: &[PersistedReasoningClaim],
    ) -> Result<()> {
        for claim in claims {
            let payload_json =
                serde_json::to_string(&claim.payload).unwrap_or_else(|_| "{}".into());
            let mut claim_params = params_map(&[
                ("ws", workspace),
                ("claim_id", &claim.claim_id),
                ("claim_kind", &claim.claim_kind),
                ("subject", &claim.subject),
                ("status", &claim.status),
                ("summary", &claim.summary),
                ("payload_json", &payload_json),
                ("prov_source", &claim.provenance.source),
                ("prov_state", &claim.provenance.state),
            ]);
            claim_params.insert("stale".into(), cozo::DataValue::Bool(claim.stale));
            claim_params.insert("computed_at_us".into(), int_dv(claim.computed_at_us));
            self.run_mutation_script(
                "?[workspace, claim_id, claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale, computed_at_us] <- \
                 [[$ws, $claim_id, $claim_kind, $subject, $status, $summary, $payload_json, $prov_source, $prov_state, $stale, $computed_at_us]] \
                 :put reasoning_claim { workspace, claim_id => claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale, computed_at_us }",
                claim_params,
                format!("save reasoning claim '{}'", claim.claim_id),
            )?;

            for (idx, derivation) in claim.derivations.iter().enumerate() {
                let derived_from_json =
                    serde_json::to_string(&derivation.derived_from).unwrap_or_else(|_| "[]".into());
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("claim_id", &claim.claim_id),
                    ("rule", &derivation.rule),
                    ("derived_from_json", &derived_from_json),
                ]);
                params.insert("idx".into(), int_dv(idx as i64));
                params.insert(
                    "witness_count".into(),
                    int_dv(derivation.witness_count as i64),
                );
                self.run_mutation_script(
                    "?[workspace, claim_id, idx, rule, derived_from_json, witness_count] <- \
                     [[$ws, $claim_id, $idx, $rule, $derived_from_json, $witness_count]] \
                     :put reasoning_derivation { workspace, claim_id, idx => rule, derived_from_json, witness_count }",
                    params,
                    format!("save reasoning derivation '{}' [{}]", claim.claim_id, idx),
                )?;
            }

            for (idx, assumption) in claim.assumptions.iter().enumerate() {
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("claim_id", &claim.claim_id),
                    ("assumption_kind", &assumption.assumption_kind),
                    ("text", &assumption.text),
                ]);
                params.insert("idx".into(), int_dv(idx as i64));
                self.run_mutation_script(
                    "?[workspace, claim_id, idx, assumption_kind, text] <- \
                     [[$ws, $claim_id, $idx, $assumption_kind, $text]] \
                     :put reasoning_assumption { workspace, claim_id, idx => assumption_kind, text }",
                    params,
                    format!("save reasoning assumption '{}' [{}]", claim.claim_id, idx),
                )?;
            }

            for (idx, support) in claim.supports.iter().enumerate() {
                let detail_json =
                    serde_json::to_string(&support.detail).unwrap_or_else(|_| "{}".into());
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("claim_id", &claim.claim_id),
                    ("support_kind", &support.support_kind),
                    ("summary", &support.summary),
                    ("detail_json", &detail_json),
                ]);
                params.insert("idx".into(), int_dv(idx as i64));
                self.run_mutation_script(
                    "?[workspace, claim_id, idx, support_kind, summary, detail_json] <- \
                     [[$ws, $claim_id, $idx, $support_kind, $summary, $detail_json]] \
                     :put reasoning_support { workspace, claim_id, idx => support_kind, summary, detail_json }",
                    params,
                    format!("save reasoning support '{}' [{}]", claim.claim_id, idx),
                )?;
            }

            for (idx, dependency) in claim.dependencies.iter().enumerate() {
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("claim_id", &claim.claim_id),
                    ("dependency_kind", &dependency.dependency_kind),
                    ("dependency_state", &dependency.dependency_state),
                ]);
                params.insert("idx".into(), int_dv(idx as i64));
                params.insert(
                    "basis_timestamp_us".into(),
                    int_dv(dependency.basis_timestamp_us),
                );
                self.run_mutation_script(
                    "?[workspace, claim_id, idx, dependency_kind, dependency_state, basis_timestamp_us] <- \
                     [[$ws, $claim_id, $idx, $dependency_kind, $dependency_state, $basis_timestamp_us]] \
                     :put reasoning_dependency { workspace, claim_id, idx => dependency_kind, dependency_state, basis_timestamp_us }",
                    params,
                    format!("save reasoning dependency '{}' [{}]", claim.claim_id, idx),
                )?;
            }

            for (idx, justification) in claim.justifications.iter().enumerate() {
                let mut params = params_map(&[
                    ("ws", workspace),
                    ("claim_id", &claim.claim_id),
                    ("fact_kind", &justification.fact_kind),
                    ("fact_key", &justification.fact_key),
                    ("fact_state", &justification.fact_state),
                ]);
                params.insert("idx".into(), int_dv(idx as i64));
                params.insert(
                    "basis_timestamp_us".into(),
                    int_dv(justification.basis_timestamp_us),
                );
                self.run_mutation_script(
                    "?[workspace, claim_id, idx, fact_kind, fact_key, fact_state, basis_timestamp_us] <- \
                     [[$ws, $claim_id, $idx, $fact_kind, $fact_key, $fact_state, $basis_timestamp_us]] \
                     :put reasoning_justification { workspace, claim_id, idx => fact_kind, fact_key, fact_state, basis_timestamp_us }",
                    params,
                    format!("save reasoning justification '{}' [{}]", claim.claim_id, idx),
                )?;
            }
        }

        Ok(())
    }

    pub fn save_reasoning_claims(
        &self,
        workspace_path: &str,
        claims: &[PersistedReasoningClaim],
    ) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        self.clear_reasoning_claims(&ws)?;

        self.write_reasoning_claims(&ws, claims)
    }

    pub fn upsert_reasoning_claims(
        &self,
        workspace_path: &str,
        claims: &[PersistedReasoningClaim],
    ) -> Result<()> {
        let ws = canonicalize_path(workspace_path);
        let claim_ids: Vec<String> = claims.iter().map(|claim| claim.claim_id.clone()).collect();
        self.clear_reasoning_claim_ids(&ws, &claim_ids)?;

        self.write_reasoning_claims(&ws, claims)
    }

    pub fn load_reasoning_claims(
        &self,
        workspace_path: &str,
    ) -> Result<Vec<PersistedReasoningClaim>> {
        let ws = canonicalize_path(workspace_path);
        let result = self
            .run_script(
                "?[claim_id] := *reasoning_claim{workspace: $ws, claim_id} :sort claim_id",
                params_map(&[("ws", &ws)]),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("load_reasoning_claims: {:?}", e))?;

        let mut claims = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            if let Some(claim) = self.load_reasoning_claim(&ws, &dv_str(&row[0]))? {
                claims.push(claim);
            }
        }
        Ok(claims)
    }

    pub fn load_reasoning_claim(
        &self,
        workspace_path: &str,
        claim_id: &str,
    ) -> Result<Option<PersistedReasoningClaim>> {
        let ws = canonicalize_path(workspace_path);
        let header = self
            .run_script(
                "?[claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale, computed_at_us] := \
                 *reasoning_claim{workspace: $ws, claim_id: $claim_id, claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale, computed_at_us}",
                params_map(&[("ws", &ws), ("claim_id", claim_id)]),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("load_reasoning_claim '{}': {:?}", claim_id, e))?;

        let Some(row) = header.rows.first() else {
            return Ok(None);
        };

        let derivations = self
            .run_script(
                "?[idx, rule, derived_from_json, witness_count] := *reasoning_derivation{workspace: $ws, claim_id: $claim_id, idx, rule, derived_from_json, witness_count} :sort idx",
                params_map(&[("ws", &ws), ("claim_id", claim_id)]),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("load_reasoning_derivation '{}': {:?}", claim_id, e))?
            .rows
            .iter()
            .map(|row| ReasoningDerivation {
                rule: dv_str(&row[1]),
                derived_from: serde_json::from_str::<Vec<String>>(&dv_str(&row[2]))
                    .unwrap_or_default(),
                witness_count: dv_i64(&row[3]) as usize,
            })
            .collect();

        let assumptions = self
            .run_script(
                "?[idx, assumption_kind, text] := *reasoning_assumption{workspace: $ws, claim_id: $claim_id, idx, assumption_kind, text} :sort idx",
                params_map(&[("ws", &ws), ("claim_id", claim_id)]),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("load_reasoning_assumption '{}': {:?}", claim_id, e))?
            .rows
            .iter()
            .map(|row| ReasoningAssumption {
                assumption_kind: dv_str(&row[1]),
                text: dv_str(&row[2]),
            })
            .collect();

        let supports = self
            .run_script(
                "?[idx, support_kind, summary, detail_json] := *reasoning_support{workspace: $ws, claim_id: $claim_id, idx, support_kind, summary, detail_json} :sort idx",
                params_map(&[("ws", &ws), ("claim_id", claim_id)]),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("load_reasoning_support '{}': {:?}", claim_id, e))?
            .rows
            .iter()
            .map(|row| ReasoningSupportEdge {
                support_kind: dv_str(&row[1]),
                summary: dv_str(&row[2]),
                detail: serde_json::from_str::<Value>(&dv_str(&row[3]))
                    .unwrap_or_else(|_| json!({})),
            })
            .collect();

        let dependencies = self
            .run_script(
                "?[idx, dependency_kind, dependency_state, basis_timestamp_us] := *reasoning_dependency{workspace: $ws, claim_id: $claim_id, idx, dependency_kind, dependency_state, basis_timestamp_us} :sort idx",
                params_map(&[("ws", &ws), ("claim_id", claim_id)]),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("load_reasoning_dependency '{}': {:?}", claim_id, e))?
            .rows
            .iter()
            .map(|row| ReasoningDependency {
                dependency_kind: dv_str(&row[1]),
                dependency_state: dv_str(&row[2]),
                basis_timestamp_us: dv_i64(&row[3]),
            })
            .collect();

        let justifications = self
            .run_script(
                "?[idx, fact_kind, fact_key, fact_state, basis_timestamp_us] := *reasoning_justification{workspace: $ws, claim_id: $claim_id, idx, fact_kind, fact_key, fact_state, basis_timestamp_us} :sort idx",
                params_map(&[("ws", &ws), ("claim_id", claim_id)]),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("load_reasoning_justification '{}': {:?}", claim_id, e))?
            .rows
            .iter()
            .map(|row| ReasoningJustification {
                fact_kind: dv_str(&row[1]),
                fact_key: dv_str(&row[2]),
                fact_state: dv_str(&row[3]),
                basis_timestamp_us: dv_i64(&row[4]),
            })
            .collect();

        Ok(Some(PersistedReasoningClaim {
            claim_id: claim_id.to_string(),
            claim_kind: dv_str(&row[0]),
            subject: dv_str(&row[1]),
            status: dv_str(&row[2]),
            summary: dv_str(&row[3]),
            payload: serde_json::from_str::<Value>(&dv_str(&row[4])).unwrap_or_else(|_| json!({})),
            provenance: ReasoningProvenance {
                source: dv_str(&row[5]),
                state: dv_str(&row[6]),
            },
            stale: matches!(&row[7], cozo::DataValue::Bool(true)),
            computed_at_us: dv_i64(&row[8]),
            derivations,
            assumptions,
            supports,
            dependencies,
            justifications,
        }))
    }

    pub fn load_stale_reasoning_claims(
        &self,
        workspace_path: &str,
    ) -> Result<Vec<PersistedReasoningClaim>> {
        let ws = canonicalize_path(workspace_path);
        let result = self
            .run_script(
                "?[claim_id] := *reasoning_claim{workspace: $ws, claim_id, claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale: true, computed_at_us} :sort claim_id",
                params_map(&[("ws", &ws)]),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("load_stale_reasoning_claims: {:?}", e))?;

        let mut claims = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            if let Some(claim) = self.load_reasoning_claim(&ws, &dv_str(&row[0]))? {
                claims.push(claim);
            }
        }
        Ok(claims)
    }

    pub fn load_stale_reasoning_claims_for_dependency(
        &self,
        workspace_path: &str,
        dependency_state: &str,
    ) -> Result<Vec<PersistedReasoningClaim>> {
        let ws = canonicalize_path(workspace_path);
        let result = self
            .run_script(
                "?[claim_id] := \
                 *reasoning_dependency{workspace: $ws, claim_id, idx, dependency_kind, dependency_state: $dependency_state, basis_timestamp_us}, \
                 *reasoning_claim{workspace: $ws, claim_id, claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale: true, computed_at_us} \
                 :sort claim_id",
                params_map(&[("ws", &ws), ("dependency_state", dependency_state)]),
                ScriptMutability::Immutable,
            )
            .map_err(|e| {
                anyhow::anyhow!(
                    "load_stale_reasoning_claims_for_dependency '{}': {:?}",
                    dependency_state,
                    e
                )
            })?;

        let mut claim_ids = BTreeSet::new();
        let mut claims = Vec::new();
        for row in &result.rows {
            let claim_id = dv_str(&row[0]);
            if claim_ids.insert(claim_id.clone()) {
                if let Some(claim) = self.load_reasoning_claim(&ws, &claim_id)? {
                    claims.push(claim);
                }
            }
        }

        Ok(claims)
    }

    pub fn invalidate_reasoning_claims_for_dependency(
        &self,
        workspace_path: &str,
        dependency_state: &str,
    ) -> Result<usize> {
        let ws = canonicalize_path(workspace_path);
        let result = self
            .run_script(
                "?[claim_id] := *reasoning_dependency{workspace: $ws, claim_id, idx, dependency_kind, dependency_state: $dependency_state, basis_timestamp_us} :sort claim_id",
                params_map(&[("ws", &ws), ("dependency_state", dependency_state)]),
                ScriptMutability::Immutable,
            )
            .map_err(|e| {
                anyhow::anyhow!(
                    "invalidate_reasoning_claims_for_dependency '{}': {:?}",
                    dependency_state,
                    e
                )
            })?;

        for row in &result.rows {
            let claim_id = dv_str(&row[0]);
            self.run_mutation_script(
                "current[claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, computed_at_us] := \
                 *reasoning_claim{workspace: $ws, claim_id: $claim_id, claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale: current_stale, computed_at_us} \
                 ?[workspace, claim_id, claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale, computed_at_us] := \
                 current[claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, computed_at_us], workspace = $ws, claim_id = $claim_id, stale = true \
                 :put reasoning_claim { workspace, claim_id => claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale, computed_at_us }",
                params_map(&[("ws", &ws), ("claim_id", &claim_id)]),
                format!(
                    "mark reasoning claim '{}' stale for dependency '{}'",
                    claim_id,
                    dependency_state
                ),
            )?;
        }

        Ok(result.rows.len())
    }

    pub fn invalidate_reasoning_claims_for_facts(
        &self,
        workspace_path: &str,
        facts: &[ReasoningFactRef],
    ) -> Result<usize> {
        let ws = canonicalize_path(workspace_path);
        let mut claim_ids = BTreeSet::new();

        for fact in facts {
            let result = self
                .run_script(
                    "?[claim_id, fact_key] := *reasoning_justification{workspace: $ws, claim_id, idx, fact_kind: $fact_kind, fact_key, fact_state: $fact_state, basis_timestamp_us}",
                    params_map(&[
                        ("ws", &ws),
                        ("fact_kind", &fact.fact_kind),
                        ("fact_state", &fact.fact_state),
                    ]),
                    ScriptMutability::Immutable,
                )
                .map_err(|e| anyhow::anyhow!("invalidate_reasoning_claims_for_facts '{:?}': {:?}", fact, e))?;

            for row in &result.rows {
                let claim_id = dv_str(&row[0]);
                let stored_key = dv_str(&row[1]);
                if fact.fact_key == "*" || stored_key == "*" || stored_key == fact.fact_key {
                    claim_ids.insert(claim_id);
                }
            }
        }

        for claim_id in &claim_ids {
            self.run_mutation_script(
                "current[claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, computed_at_us] := \
                 *reasoning_claim{workspace: $ws, claim_id: $claim_id, claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale: current_stale, computed_at_us} \
                 ?[workspace, claim_id, claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale, computed_at_us] := \
                 current[claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, computed_at_us], workspace = $ws, claim_id = $claim_id, stale = true \
                 :put reasoning_claim { workspace, claim_id => claim_kind, subject, status, summary, payload_json, provenance_source, provenance_state, stale, computed_at_us }",
                params_map(&[("ws", &ws), ("claim_id", claim_id)]),
                format!("mark reasoning claim '{}' stale for fact invalidation", claim_id),
            )?;
        }

        Ok(claim_ids.len())
    }

    /// List distinct save timestamps for a workspace+state, derived from
    /// the `snapshot_log` relation. Returns microsecond timestamps in
    /// descending order (most recent first).
    pub fn list_snapshots(&self, workspace_path: &str, state: &str) -> Result<Vec<i64>> {
        let ws = canonicalize_path(workspace_path);
        let params = params_map(&[("ws", &ws), ("st", state)]);
        let result = self
            .run_script(
                "?[ts] := *snapshot_log{workspace: $ws, state: $st, timestamp_us: ts} \
             :sort -ts",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("list_snapshots: {:?}", e))?;
        Ok(result.rows.iter().map(|r| dv_i64(&r[0])).collect())
    }

    /// Compare two Validity timestamps and return the diff of entities present
    /// at `ts_new` but not at `ts_old` (added) and vice versa (removed).
    /// Timestamps are microsecond epoch values from `list_snapshots`.
    pub fn diff_snapshots(
        &self,
        workspace_path: &str,
        state: &str,
        ts_old: i64,
        ts_new: i64,
    ) -> Result<serde_json::Value> {
        let ws = canonicalize_path(workspace_path);
        let mut params = params_map(&[("ws", &ws), ("st", state)]);
        params.insert("ts_old".into(), cozo::DataValue::from(ts_old));
        params.insert("ts_new".into(), cozo::DataValue::from(ts_new));

        // Use parameterized @ for point-in-time queries, then diff via derived rules.
        let script = "\
            ctx_new[name] := *context{workspace: $ws, name, state: $st @ $ts_new} \
            ctx_old[name] := *context{workspace: $ws, name, state: $st @ $ts_old} \
            ent_new[ctx, name] := *entity{workspace: $ws, context: ctx, name, state: $st @ $ts_new} \
            ent_old[ctx, name] := *entity{workspace: $ws, context: ctx, name, state: $st @ $ts_old} \
            svc_new[ctx, name] := *service{workspace: $ws, context: ctx, name, state: $st @ $ts_new} \
            svc_old[ctx, name] := *service{workspace: $ws, context: ctx, name, state: $st @ $ts_old} \
            evt_new[ctx, name] := *event{workspace: $ws, context: ctx, name, state: $st @ $ts_new} \
            evt_old[ctx, name] := *event{workspace: $ws, context: ctx, name, state: $st @ $ts_old} \
            vo_new[ctx, name] := *value_object{workspace: $ws, context: ctx, name, state: $st @ $ts_new} \
            vo_old[ctx, name] := *value_object{workspace: $ws, context: ctx, name, state: $st @ $ts_old} \
            repo_new[ctx, name] := *repository{workspace: $ws, context: ctx, name, state: $st @ $ts_new} \
            repo_old[ctx, name] := *repository{workspace: $ws, context: ctx, name, state: $st @ $ts_old} \
            mod_new[ctx, name] := *module{workspace: $ws, context: ctx, name, state: $st @ $ts_new} \
            mod_old[ctx, name] := *module{workspace: $ws, context: ctx, name, state: $st @ $ts_old} \
            fld_new[ctx, ok, ow, name] := *field{workspace: $ws, context: ctx, owner_kind: ok, owner: ow, name, state: $st @ $ts_new} \
            fld_old[ctx, ok, ow, name] := *field{workspace: $ws, context: ctx, owner_kind: ok, owner: ow, name, state: $st @ $ts_old} \
            mth_new[ctx, ok, ow, name] := *method{workspace: $ws, context: ctx, owner_kind: ok, owner: ow, name, state: $st @ $ts_new} \
            mth_old[ctx, ok, ow, name] := *method{workspace: $ws, context: ctx, owner_kind: ok, owner: ow, name, state: $st @ $ts_old} \
            inv_new[ctx, ow, text] := *invariant{workspace: $ws, context: ctx, entity: ow, text, state: $st @ $ts_new} \
            inv_old[ctx, ow, text] := *invariant{workspace: $ws, context: ctx, entity: ow, text, state: $st @ $ts_old} \
            ?[kind, action, ctx, name, owner_kind, owner] := ctx_new[name], not ctx_old[name], kind = 'context', action = 'add', ctx = '', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := ctx_old[name], not ctx_new[name], kind = 'context', action = 'remove', ctx = '', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := ent_new[ctx, name], not ent_old[ctx, name], kind = 'entity', action = 'add', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := ent_old[ctx, name], not ent_new[ctx, name], kind = 'entity', action = 'remove', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := svc_new[ctx, name], not svc_old[ctx, name], kind = 'service', action = 'add', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := svc_old[ctx, name], not svc_new[ctx, name], kind = 'service', action = 'remove', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := evt_new[ctx, name], not evt_old[ctx, name], kind = 'event', action = 'add', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := evt_old[ctx, name], not evt_new[ctx, name], kind = 'event', action = 'remove', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := vo_new[ctx, name], not vo_old[ctx, name], kind = 'value_object', action = 'add', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := vo_old[ctx, name], not vo_new[ctx, name], kind = 'value_object', action = 'remove', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := repo_new[ctx, name], not repo_old[ctx, name], kind = 'repository', action = 'add', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := repo_old[ctx, name], not repo_new[ctx, name], kind = 'repository', action = 'remove', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := mod_new[ctx, name], not mod_old[ctx, name], kind = 'module', action = 'add', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := mod_old[ctx, name], not mod_new[ctx, name], kind = 'module', action = 'remove', owner_kind = '', owner = '' \
            ?[kind, action, ctx, name, owner_kind, owner] := fld_new[ctx, owner_kind, owner, name], not fld_old[ctx, owner_kind, owner, name], kind = 'field', action = 'add' \
            ?[kind, action, ctx, name, owner_kind, owner] := fld_old[ctx, owner_kind, owner, name], not fld_new[ctx, owner_kind, owner, name], kind = 'field', action = 'remove' \
            ?[kind, action, ctx, name, owner_kind, owner] := mth_new[ctx, owner_kind, owner, name], not mth_old[ctx, owner_kind, owner, name], kind = 'method', action = 'add' \
            ?[kind, action, ctx, name, owner_kind, owner] := mth_old[ctx, owner_kind, owner, name], not mth_new[ctx, owner_kind, owner, name], kind = 'method', action = 'remove' \
            ?[kind, action, ctx, name, owner_kind, owner] := inv_new[ctx, owner, name], not inv_old[ctx, owner, name], kind = 'invariant', action = 'add', owner_kind = 'entity' \
            ?[kind, action, ctx, name, owner_kind, owner] := inv_old[ctx, owner, name], not inv_new[ctx, owner, name], kind = 'invariant', action = 'remove', owner_kind = 'entity'";

        let result = self
            .run_script(script, params, ScriptMutability::Immutable)
            .map_err(|e| anyhow::anyhow!("diff_snapshots: {:?}", e))?;

        let changes: Vec<serde_json::Value> = result
            .rows
            .iter()
            .map(|r| {
                let mut entry = json!({
                    "kind": dv_str(&r[0]),
                    "action": dv_str(&r[1]),
                    "name": dv_str(&r[3]),
                });
                let ctx = dv_str(&r[2]);
                if !ctx.is_empty() {
                    entry["context"] = json!(ctx);
                }
                let owner_kind = dv_str(&r[4]);
                if !owner_kind.is_empty() {
                    entry["owner_kind"] = json!(owner_kind);
                    entry["owner"] = json!(dv_str(&r[5]));
                }
                entry
            })
            .collect();

        let added: Vec<_> = changes
            .iter()
            .filter(|c| c["action"] == "add")
            .cloned()
            .collect();
        let removed: Vec<_> = changes
            .iter()
            .filter(|c| c["action"] == "remove")
            .cloned()
            .collect();

        Ok(json!({
            "ts_old": ts_old,
            "ts_new": ts_new,
            "state": state,
            "summary": {
                "total_changes": changes.len(),
                "additions": added.len(),
                "removals": removed.len(),
            },
            "added": added,
            "removed": removed,
        }))
    }

    // ── Live AST Bridge ───────────────────────────────────────────────────

    /// Project live AST imports into the ephemeral `live_import` table,
    /// then cross-reference against the domain model to detect violations.
    pub fn check_live_dependencies(
        &self,
        workspace_path: &str,
        live_deps: &[crate::domain::analyze::LiveDependency],
    ) -> Result<Vec<crate::domain::analyze::LiveDependency>> {
        let ws = canonicalize_path(workspace_path);

        // 1. Clear previous live_import rows
        let clear_params = params_map(&[("ws", &ws)]);
        let _ = self.run_script(
            "?[workspace, from_file, to_module] := *live_import{workspace: $ws, from_file, to_module} :rm live_import { workspace, from_file, to_module }",
            clear_params,
            ScriptMutability::Mutable,
        );

        // 2. Insert current live imports
        if !live_deps.is_empty() {
            let mut values = Vec::new();
            for dep in live_deps {
                values.push(cozo::DataValue::List(vec![
                    cozo::DataValue::Str(ws.clone().into()),
                    cozo::DataValue::Str(dep.from_file.clone().into()),
                    cozo::DataValue::Str(dep.to_module.clone().into()),
                ]));
            }
            let params = BTreeMap::from([("rows".to_string(), cozo::DataValue::List(values))]);
            self.run_script(
                "?[workspace, from_file, to_module] <- $rows \
                     :put live_import { workspace, from_file, to_module }",
                params,
                ScriptMutability::Mutable,
            )
            .map_err(|e| anyhow::anyhow!("insert live_imports: {:?}", e))?;
        }

        // 3. Cross-reference against modeled contexts (desired state)
        let query_params = params_map(&[("ws", &ws)]);
        let result = self
            .run_script(
                "modeled[m] := *context{workspace: $ws, module_path: m, state: 'desired' @ 'NOW'}, m != '' \
                 ?[from_file, to_module] := *live_import{workspace: $ws, from_file, to_module}, \
                     not modeled[to_module]",
                query_params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("check_live_dependencies: {:?}", e))?;

        Ok(result
            .rows
            .iter()
            .map(|r| crate::domain::analyze::LiveDependency {
                from_file: dv_str(&r[0]),
                to_module: dv_str(&r[1]),
            })
            .collect())
    }

    // ── Datalog Query Runners ─────────────────────────────────────────────

    /// Run an arbitrary Datalog query with `$ws` parameter.
    pub fn run_datalog(&self, script: &str, workspace: &str) -> Result<Vec<Vec<String>>> {
        let params = params_map(&[("ws", workspace)]);
        let result = self
            .run_script(script, params, ScriptMutability::Immutable)
            .map_err(|e| anyhow::anyhow!("Datalog query failed: {:?}", e))?;
        Ok(result
            .rows
            .iter()
            .map(|row| row.iter().map(dv_str).collect())
            .collect())
    }

    /// Run an arbitrary Datalog query, returning headers + rows.
    pub fn run_datalog_full(
        &self,
        script: &str,
        workspace: &str,
    ) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        let params = params_map(&[("ws", workspace)]);
        let result = self
            .run_script(script, params, ScriptMutability::Immutable)
            .map_err(|e| anyhow::anyhow!("Datalog query failed: {:?}", e))?;
        let headers = result.headers.iter().map(|h| h.to_string()).collect();
        let rows = result
            .rows
            .iter()
            .map(|row| row.iter().map(dv_str).collect())
            .collect();
        Ok((headers, rows))
    }

    // ── Datalog Inference Queries (always query desired state) ─────────────

    pub fn transitive_deps(&self, workspace: &str, context: &str) -> Result<Vec<String>> {
        let params = params_map(&[("ws", workspace), ("ctx", context)]);
        let result = self
            .run_script(
                "transitive[a, c] := *context_dep{workspace: $ws, from_ctx: a, to_ctx: c, state: 'desired' @ 'NOW'} \
                 transitive[a, c] := transitive[a, b], *context_dep{workspace: $ws, from_ctx: b, to_ctx: c, state: 'desired' @ 'NOW'} \
                 ?[dep] := transitive[$ctx, dep]",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("transitive_deps: {:?}", e))?;
        Ok(result.rows.iter().map(|r| dv_str(&r[0])).collect())
    }

    pub fn circular_deps(&self, workspace: &str) -> Result<Vec<(String, String)>> {
        let params = params_map(&[("ws", workspace)]);
        let result = self
            .run_script(
                "transitive[a, c] := *context_dep{workspace: $ws, from_ctx: a, to_ctx: c, state: 'desired' @ 'NOW'} \
                 transitive[a, c] := transitive[a, b], *context_dep{workspace: $ws, from_ctx: b, to_ctx: c, state: 'desired' @ 'NOW'} \
                 ?[a, b] := transitive[a, b], transitive[b, a]",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("circular_deps: {:?}", e))?;
        Ok(result
            .rows
            .iter()
            .map(|r| (dv_str(&r[0]), dv_str(&r[1])))
            .collect())
    }

    pub fn layer_violations(&self, workspace: &str) -> Result<Vec<(String, String, String)>> {
        let params = params_map(&[("ws", workspace)]);
        let result = self
            .run_script(
                "?[context, service, dep] := \
                    *service{workspace: $ws, context, name: service, kind: 'domain', state: 'desired' @ 'NOW'}, \
                    *service_dep{workspace: $ws, context, service, dep, state: 'desired' @ 'NOW'}, \
                    *service{workspace: $ws, context, name: dep, kind: 'infrastructure', state: 'desired' @ 'NOW'}",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("layer_violations: {:?}", e))?;
        Ok(result
            .rows
            .iter()
            .map(|r| (dv_str(&r[0]), dv_str(&r[1]), dv_str(&r[2])))
            .collect())
    }

    // ── Architecture Policy Operations ────────────────────────────────────

    /// Assign a bounded context to an architectural layer.
    pub fn upsert_layer_assignment(
        &self,
        workspace: &str,
        context: &str,
        layer: &str,
    ) -> Result<()> {
        let ws = canonicalize_path(workspace);
        let params = params_map(&[("ws", &ws), ("ctx", context), ("layer", layer)]);
        self.run_script(
            "?[workspace, context, layer] <- [[$ws, $ctx, $layer]] \
                 :put layer_assignment { workspace, context => layer }",
            params,
            ScriptMutability::Mutable,
        )
        .map_err(|e| anyhow::anyhow!("upsert_layer_assignment: {:?}", e))?;
        Ok(())
    }

    /// Remove a layer assignment for a bounded context.
    pub fn remove_layer_assignment(&self, workspace: &str, context: &str) -> Result<bool> {
        let ws = canonicalize_path(workspace);
        let params = params_map(&[("ws", &ws), ("ctx", context)]);
        let existing = self
            .run_script(
                "?[workspace, context] := *layer_assignment{workspace: $ws, context: $ctx} :rm layer_assignment { workspace, context }",
                params,
                ScriptMutability::Mutable,
            )
            .map_err(|e| anyhow::anyhow!("remove_layer_assignment: {:?}", e))?;
        Ok(!existing.rows.is_empty())
    }

    /// Add a dependency constraint between layers or contexts.
    /// `constraint_kind` is `"layer"` or `"context"`.
    /// `rule` is `"forbidden"` or `"allowed"`.
    pub fn upsert_dependency_constraint(
        &self,
        workspace: &str,
        constraint_kind: &str,
        source: &str,
        target: &str,
        rule: &str,
    ) -> Result<()> {
        let ws = canonicalize_path(workspace);
        let params = params_map(&[
            ("ws", &ws),
            ("kind", constraint_kind),
            ("src", source),
            ("tgt", target),
            ("rule", rule),
        ]);
        self
            .run_script(
                "?[workspace, constraint_kind, source, target, rule] <- [[$ws, $kind, $src, $tgt, $rule]] \
                 :put dependency_constraint { workspace, constraint_kind, source, target => rule }",
                params,
                ScriptMutability::Mutable,
            )
            .map_err(|e| anyhow::anyhow!("upsert_dependency_constraint: {:?}", e))?;
        Ok(())
    }

    /// Remove a dependency constraint.
    pub fn remove_dependency_constraint(
        &self,
        workspace: &str,
        constraint_kind: &str,
        source: &str,
        target: &str,
    ) -> Result<bool> {
        let ws = canonicalize_path(workspace);
        let params = params_map(&[
            ("ws", &ws),
            ("kind", constraint_kind),
            ("src", source),
            ("tgt", target),
        ]);
        let existing = self
            .run_script(
                "?[workspace, constraint_kind, source, target] := \
                    *dependency_constraint{workspace: $ws, constraint_kind: $kind, source: $src, target: $tgt} \
                 :rm dependency_constraint { workspace, constraint_kind, source, target }",
                params,
                ScriptMutability::Mutable,
            )
            .map_err(|e| anyhow::anyhow!("remove_dependency_constraint: {:?}", e))?;
        Ok(!existing.rows.is_empty())
    }

    /// List all layer assignments for a workspace.
    pub fn list_layer_assignments(&self, workspace: &str) -> Result<Vec<(String, String)>> {
        let ws = canonicalize_path(workspace);
        let params = params_map(&[("ws", &ws)]);
        let result = self
            .run_script(
                "?[context, layer] := *layer_assignment{workspace: $ws, context, layer}",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("list_layer_assignments: {:?}", e))?;
        Ok(result
            .rows
            .iter()
            .map(|r| (dv_str(&r[0]), dv_str(&r[1])))
            .collect())
    }

    /// List all dependency constraints for a workspace.
    pub fn list_dependency_constraints(
        &self,
        workspace: &str,
    ) -> Result<Vec<(String, String, String, String)>> {
        let ws = canonicalize_path(workspace);
        let params = params_map(&[("ws", &ws)]);
        let result = self
            .run_script(
                "?[constraint_kind, source, target, rule] := \
                    *dependency_constraint{workspace: $ws, constraint_kind, source, target, rule}",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("list_dependency_constraints: {:?}", e))?;
        Ok(result
            .rows
            .iter()
            .map(|r| (dv_str(&r[0]), dv_str(&r[1]), dv_str(&r[2]), dv_str(&r[3])))
            .collect())
    }

    /// Evaluate policy violations: find context dependencies that violate layer
    /// or context-level forbidden constraints.
    pub fn evaluate_policy_violations(&self, workspace: &str) -> Result<serde_json::Value> {
        let ws = canonicalize_path(workspace);
        let params = params_map(&[("ws", &ws)]);

        // Layer-based violations: context A (layer X) depends on context B (layer Y)
        // where X→Y is forbidden
        let layer_violations = self
            .run_script(
                "?[from_ctx, to_ctx, from_layer, to_layer] := \
                    *context_dep{workspace: $ws, from_ctx, to_ctx, state: 'desired' @ 'NOW'}, \
                    *layer_assignment{workspace: $ws, context: from_ctx, layer: from_layer}, \
                    *layer_assignment{workspace: $ws, context: to_ctx, layer: to_layer}, \
                    *dependency_constraint{workspace: $ws, constraint_kind: 'layer', \
                        source: from_layer, target: to_layer, rule: 'forbidden'}",
                params.clone(),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("policy layer violations: {:?}", e))?;

        // Context-level violations: context A depends on context B where A→B is forbidden
        let context_violations = self
            .run_script(
                "?[from_ctx, to_ctx] := \
                    *context_dep{workspace: $ws, from_ctx, to_ctx, state: 'desired' @ 'NOW'}, \
                    *dependency_constraint{workspace: $ws, constraint_kind: 'context', \
                        source: from_ctx, target: to_ctx, rule: 'forbidden'}",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("policy context violations: {:?}", e))?;

        let layer_items: Vec<serde_json::Value> = layer_violations
            .rows
            .iter()
            .map(|r| {
                json!({
                    "kind": "layer",
                    "from_context": dv_str(&r[0]),
                    "to_context": dv_str(&r[1]),
                    "from_layer": dv_str(&r[2]),
                    "to_layer": dv_str(&r[3]),
                    "rule": "forbidden",
                })
            })
            .collect();

        let context_items: Vec<serde_json::Value> = context_violations
            .rows
            .iter()
            .map(|r| {
                json!({
                    "kind": "context",
                    "from_context": dv_str(&r[0]),
                    "to_context": dv_str(&r[1]),
                    "rule": "forbidden",
                })
            })
            .collect();

        let all_violations: Vec<serde_json::Value> =
            layer_items.into_iter().chain(context_items).collect();

        Ok(json!({
            "status": if all_violations.is_empty() { "true" } else { "false" },
            "violations": all_violations,
            "count": all_violations.len(),
        }))
    }

    pub fn impact_analysis(
        &self,
        workspace: &str,
        context: &str,
        entity_name: &str,
    ) -> Result<serde_json::Value> {
        let params = params_map(&[("ws", workspace), ("ctx", context), ("ent", entity_name)]);

        let events = self
            .run_script(
                "?[context, event_name] := \
                    *event{workspace: $ws, context, name: event_name, source: $ent, state: 'desired' @ 'NOW'}",
                params.clone(),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("impact events: {:?}", e))?;

        let services = self
            .run_script(
                "?[context, service_name] := \
                    *repository{workspace: $ws, context: $ctx, aggregate: $ent, name: repo_name, state: 'desired' @ 'NOW'}, \
                    *service_dep{workspace: $ws, context, service: service_name, dep: repo_name, state: 'desired' @ 'NOW'}",
                params.clone(),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("impact services: {:?}", e))?;

        let reverse_params = params_map(&[("ws", workspace), ("ctx", context)]);
        let dependents = self
            .run_script(
                "transitive[a, c] := *context_dep{workspace: $ws, from_ctx: a, to_ctx: c, state: 'desired' @ 'NOW'} \
                 transitive[a, c] := transitive[a, b], *context_dep{workspace: $ws, from_ctx: b, to_ctx: c, state: 'desired' @ 'NOW'} \
                 ?[dependent] := transitive[dependent, $ctx]",
                reverse_params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("impact dependents: {:?}", e))?;

        let ast_impact = self
            .run_script(
                "ast[target, type] := *ast_edge{workspace: $ws, state: 'actual', from_node: $ent, to_node: target, edge_type: type @ 'NOW'} \
                 ast[target, type] := ast[mid, _], *ast_edge{workspace: $ws, state: 'actual', from_node: mid, to_node: target, edge_type: type @ 'NOW'} \
                 ?[target, type] := ast[target, type]",
                params_map(&[("ws", workspace), ("ent", entity_name)]),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("ast impact: {:?}", e))?;

        // Symbol-level: find files that import modules containing this entity name
        let importing_files = self
            .run_script(
                "?[from_file, to_module, context] := *import_edge{workspace: $ws, from_file, to_module, state: 'actual', context @ 'NOW'}, \
                 is_in(to_module, $ent)",
                params_map(&[("ws", workspace), ("ent", entity_name)]),
                ScriptMutability::Immutable,
            )
            .map(|r| r.rows)
            .unwrap_or_default();

        Ok(json!({
            "entity": entity_name,
            "context": context,
            "affected_events": events.rows.iter()
                .map(|r| json!({"context": dv_str(&r[0]), "event": dv_str(&r[1])}))
                .collect::<Vec<_>>(),
            "affected_services": services.rows.iter()
                .map(|r| json!({"context": dv_str(&r[0]), "service": dv_str(&r[1])}))
                .collect::<Vec<_>>(),
            "dependent_contexts": dependents.rows.iter()
                .map(|r| dv_str(&r[0]))
                .collect::<Vec<_>>(),
            "ast_impact": ast_impact.rows.iter()
                .map(|r| json!({"target": dv_str(&r[0]), "type": dv_str(&r[1])}))
                .collect::<Vec<_>>(),
            "importing_files": importing_files.iter()
                .map(|r| json!({"file": dv_str(&r[0]), "import": dv_str(&r[1]), "context": dv_str(&r[2])}))
                .collect::<Vec<_>>(),
        }))
    }

    pub fn aggregate_roots_without_invariants(
        &self,
        workspace: &str,
    ) -> Result<Vec<(String, String)>> {
        let params = params_map(&[("ws", workspace)]);
        let result = self
            .run_script(
                "has_inv[ctx, ent] := *invariant{workspace: $ws, context: ctx, entity: ent, state: 'desired' @ 'NOW'} \
                 ?[context, entity] := \
                    *entity{workspace: $ws, context, name: entity, aggregate_root: true, state: 'desired' @ 'NOW'}, \
                    not has_inv[context, entity]",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("aggregate_roots_without_invariants: {:?}", e))?;
        Ok(result
            .rows
            .iter()
            .map(|r| (dv_str(&r[0]), dv_str(&r[1])))
            .collect())
    }

    pub fn query_dependency_path(
        &self,
        workspace: &str,
        from_context: &str,
        to_context: &str,
    ) -> Result<Vec<Vec<String>>> {
        let params = params_map(&[
            ("ws", workspace),
            ("from_ctx", from_context),
            ("to_ctx", to_context),
        ]);
        let result = self
            .run_script(
                "reachable[a, b] := *context_dep{workspace: $ws, from_ctx: a, to_ctx: b, state: 'desired' @ 'NOW'} \
                 reachable[a, c] := reachable[a, b], *context_dep{workspace: $ws, from_ctx: b, to_ctx: c, state: 'desired' @ 'NOW'} \
                 on_path[a, b] := *context_dep{workspace: $ws, from_ctx: a, to_ctx: b, state: 'desired' @ 'NOW'}, reachable[a, $to_ctx], a == $from_ctx \
                 on_path[a, b] := *context_dep{workspace: $ws, from_ctx: a, to_ctx: b, state: 'desired' @ 'NOW'}, reachable[$from_ctx, a], reachable[b, $to_ctx] \
                 on_path[a, b] := *context_dep{workspace: $ws, from_ctx: a, to_ctx: b, state: 'desired' @ 'NOW'}, reachable[$from_ctx, a], b == $to_ctx \
                 ?[a, b] := on_path[a, b]",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("query_dependency_path: {:?}", e))?;

        Ok(result
            .rows
            .iter()
            .map(|r| vec![dv_str(&r[0]), dv_str(&r[1])])
            .collect())
    }

    pub fn can_delete_symbol(
        &self,
        workspace: &str,
        context: &str,
        entity_name: &str,
    ) -> Result<serde_json::Value> {
        let params = params_map(&[("ws", workspace), ("ctx", context), ("ent", entity_name)]);

        let aggreg = self.run_script(
            "?[agg] := *aggregate_member{workspace: $ws, context: $ctx, member: $ent, state: 'desired', aggregate: agg @ 'NOW'}",
            params.clone(),
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("check aggregate: {:?}", e))?;

        let events = self.run_script(
            "?[evt, file_path, start_line, end_line] := *event{workspace: $ws, context: $ctx, source: $ent, state: 'desired', name: evt, file_path, start_line, end_line @ 'NOW'}",
            params.clone(),
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("check events: {:?}", e))?;

        let repos = self.run_script(
            "?[repo, file_path, start_line, end_line] := *repository{workspace: $ws, context: $ctx, aggregate: $ent, state: 'desired', name: repo, file_path, start_line, end_line @ 'NOW'}",
            params.clone(),
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("check repo: {:?}", e))?;

        let has_deps = !aggreg.rows.is_empty() || !events.rows.is_empty() || !repos.rows.is_empty();

        // Symbol-level: check if any import edges reference this symbol
        let import_refs = self.run_script(
            "?[from_file, to_module] := *import_edge{workspace: $ws, from_file, to_module, state: 'actual' @ 'NOW'}, \
             is_in(to_module, $ent)",
            params.clone(),
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("check import references: {:?}", e))?.rows;

        // AST edges: check if any node references this symbol
        let ast_refs = self.run_script(
            "?[from_node, edge_type] := *ast_edge{workspace: $ws, state: 'actual', from_node, to_node: $ent, edge_type @ 'NOW'}",
            params.clone(),
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("check ast references: {:?}", e))?.rows;

        // Call graph: check if any caller targets this symbol
        let call_refs = self.run_script(
            "?[caller, file_path, line] := *calls_symbol{workspace: $ws, caller, callee: $ent, state: 'actual', file_path, line @ 'NOW'}",
            params.clone(),
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("check call references: {:?}", e))?.rows;

        let has_symbol_refs =
            !import_refs.is_empty() || !ast_refs.is_empty() || !call_refs.is_empty();

        Ok(serde_json::json!({
            "can_delete": !has_deps && !has_symbol_refs,
            "aggregates_referencing": aggreg.rows.iter().map(|r| dv_str(&r[0])).collect::<Vec<_>>(),
            "events_sourced": events.rows.iter().map(|r| dv_str(&r[0])).collect::<Vec<_>>(),
            "repositories_managing": repos.rows.iter().map(|r| dv_str(&r[0])).collect::<Vec<_>>(),
            "event_references": events.rows.iter().map(|r| json!({
                "event": dv_str(&r[0]),
                "file": dv_str(&r[1]),
                "start_line": dv_i64(&r[2]),
                "end_line": dv_i64(&r[3]),
            })).collect::<Vec<_>>(),
            "repository_references": repos.rows.iter().map(|r| json!({
                "repository": dv_str(&r[0]),
                "file": dv_str(&r[1]),
                "start_line": dv_i64(&r[2]),
                "end_line": dv_i64(&r[3]),
            })).collect::<Vec<_>>(),
            "import_references": import_refs.iter().map(|r| json!({"file": dv_str(&r[0]), "import": dv_str(&r[1])})).collect::<Vec<_>>(),
            "ast_references": ast_refs.iter().map(|r| json!({"from": dv_str(&r[0]), "edge_type": dv_str(&r[1])})).collect::<Vec<_>>(),
            "call_references": call_refs.iter().map(|r| json!({"caller": dv_str(&r[0]), "file": dv_str(&r[1]), "line": dv_i64(&r[2])})).collect::<Vec<_>>(),
        }))
    }

    // ── Call Graph Queries ────────────────────────────────────────────────

    /// Return all direct callers of a symbol.
    pub fn call_graph_callers(&self, workspace: &str, symbol: &str) -> Result<serde_json::Value> {
        let ws = canonicalize_path(workspace);
        let params = params_map(&[("ws", &ws), ("sym", symbol)]);
        let rows = self.run_script(
            "?[caller, file_path, line, context] := *calls_symbol{workspace: $ws, caller, callee: $sym, state: 'actual', file_path, line, context @ 'NOW'}",
            params,
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("call_graph_callers: {:?}", e))?;
        Ok(json!({
            "symbol": symbol,
            "callers": rows.rows.iter().map(|r| json!({
                "caller": dv_str(&r[0]),
                "file": dv_str(&r[1]),
                "line": dv_i64(&r[2]),
                "context": dv_str(&r[3]),
            })).collect::<Vec<_>>(),
            "count": rows.rows.len(),
        }))
    }

    /// Return all direct callees of a symbol.
    pub fn call_graph_callees(&self, workspace: &str, symbol: &str) -> Result<serde_json::Value> {
        let ws = canonicalize_path(workspace);
        let params = params_map(&[("ws", &ws), ("sym", symbol)]);
        let rows = self.run_script(
            "?[callee, file_path, line, context] := *calls_symbol{workspace: $ws, caller: $sym, callee, state: 'actual', file_path, line, context @ 'NOW'}",
            params,
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("call_graph_callees: {:?}", e))?;
        Ok(json!({
            "symbol": symbol,
            "callees": rows.rows.iter().map(|r| json!({
                "callee": dv_str(&r[0]),
                "file": dv_str(&r[1]),
                "line": dv_i64(&r[2]),
                "context": dv_str(&r[3]),
            })).collect::<Vec<_>>(),
            "count": rows.rows.len(),
        }))
    }

    /// Compute transitive call reachability from a symbol using Datalog fixed-point.
    pub fn call_graph_reachability(
        &self,
        workspace: &str,
        symbol: &str,
    ) -> Result<serde_json::Value> {
        let ws = canonicalize_path(workspace);
        let params = params_map(&[("ws", &ws), ("sym", symbol)]);
        let rows = self.run_script(
            "reachable[callee] := *calls_symbol{workspace: $ws, caller: $sym, callee, state: 'actual' @ 'NOW'} \
             reachable[c] := reachable[b], *calls_symbol{workspace: $ws, caller: b, callee: c, state: 'actual' @ 'NOW'} \
             ?[callee] := reachable[callee]",
            params,
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("call_graph_reachability: {:?}", e))?;
        Ok(json!({
            "symbol": symbol,
            "reachable": rows.rows.iter().map(|r| dv_str(&r[0])).collect::<Vec<_>>(),
            "count": rows.rows.len(),
        }))
    }

    /// Summary statistics for the call graph in a workspace.
    pub fn call_graph_stats(&self, workspace: &str) -> Result<serde_json::Value> {
        let ws = canonicalize_path(workspace);
        let params = params_map(&[("ws", &ws)]);

        let total = self.run_script(
            "?[count(caller)] := *calls_symbol{workspace: $ws, caller, state: 'actual' @ 'NOW'}",
            params.clone(),
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("call_graph_stats total: {:?}", e))?;

        let unique_callers = self.run_script(
            "?[count_unique(caller)] := *calls_symbol{workspace: $ws, caller, state: 'actual' @ 'NOW'}",
            params.clone(),
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("call_graph_stats callers: {:?}", e))?;

        let unique_callees = self.run_script(
            "?[count_unique(callee)] := *calls_symbol{workspace: $ws, callee, state: 'actual' @ 'NOW'}",
            params.clone(),
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("call_graph_stats callees: {:?}", e))?;

        // Top-10 most-called symbols
        let hot_callees = self.run_script(
            "?[callee, count(caller)] := *calls_symbol{workspace: $ws, caller, callee, state: 'actual' @ 'NOW'} \
             :order -count(caller) \
             :limit 10",
            params.clone(),
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("call_graph_stats hot: {:?}", e))?;

        Ok(json!({
            "total_edges": if total.rows.is_empty() { 0 } else { dv_i64(&total.rows[0][0]) },
            "unique_callers": if unique_callers.rows.is_empty() { 0 } else { dv_i64(&unique_callers.rows[0][0]) },
            "unique_callees": if unique_callees.rows.is_empty() { 0 } else { dv_i64(&unique_callees.rows[0][0]) },
            "hottest_callees": hot_callees.rows.iter().map(|r| json!({
                "callee": dv_str(&r[0]),
                "call_count": dv_i64(&r[1]),
            })).collect::<Vec<_>>(),
        }))
    }

    pub fn dependency_graph(&self, workspace: &str) -> Result<serde_json::Value> {
        let params = params_map(&[("ws", workspace)]);
        let contexts = self
            .run_script(
                "?[name, module_path] := *context{workspace: $ws, name, module_path, state: 'desired' @ 'NOW'}",
                params.clone(),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("dependency_graph contexts: {:?}", e))?;
        let deps = self
            .run_script(
                "?[from_ctx, to_ctx] := *context_dep{workspace: $ws, from_ctx, to_ctx, state: 'desired' @ 'NOW'}",
                params.clone(),
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("dependency_graph deps: {:?}", e))?;
        let circular = self.circular_deps(workspace)?;

        Ok(json!({
            "nodes": contexts.rows.iter()
                .map(|r| json!({"name": dv_str(&r[0]), "module_path": dv_str(&r[1])}))
                .collect::<Vec<_>>(),
            "edges": deps.rows.iter()
                .map(|r| json!({"from": dv_str(&r[0]), "to": dv_str(&r[1])}))
                .collect::<Vec<_>>(),
            "circular_dependencies": circular.iter()
                .map(|(a, b)| json!({"a": a, "b": b}))
                .collect::<Vec<_>>(),
        }))
    }

    // ── Full-Text Search ──────────────────────────────────────────────────

    /// Search architecture entities by keyword using CozoDB FTS indices.
    /// Returns matches across contexts, entities, services, events, and decisions.
    pub fn search_text(
        &self,
        workspace: &str,
        query: &str,
        limit: usize,
    ) -> Result<serde_json::Value> {
        let ws = canonicalize_path(workspace);
        let mut params = params_map(&[("ws", &ws), ("q", query)]);
        params.insert("k".into(), int_dv(limit as i64));

        let mut results: Vec<serde_json::Value> = Vec::new();

        // Search contexts
        if let Ok(r) = self.run_script(
            "?[name, description, score] := ~context:fts{workspace, name, state | query: $q, k: $k, bind_score: score}, \
             workspace = $ws, state = 'desired', *context{workspace, name, state, description @ 'NOW'}",
            params.clone(), ScriptMutability::Immutable,
        ) {
            for row in &r.rows {
                results.push(json!({"kind": "context", "name": dv_str(&row[0]), "description": dv_str(&row[1]), "score": dv_str(&row[2])}));
            }
        }

        // Search entities
        if let Ok(r) = self.run_script(
            "?[context, name, description, score] := ~entity:fts{workspace, context, name, state | query: $q, k: $k, bind_score: score}, \
             workspace = $ws, state = 'desired', *entity{workspace, context, name, state, description @ 'NOW'}",
            params.clone(), ScriptMutability::Immutable,
        ) {
            for row in &r.rows {
                results.push(json!({"kind": "entity", "context": dv_str(&row[0]), "name": dv_str(&row[1]), "description": dv_str(&row[2]), "score": dv_str(&row[3])}));
            }
        }

        // Search services
        if let Ok(r) = self.run_script(
            "?[context, name, description, score] := ~service:fts{workspace, context, name, state | query: $q, k: $k, bind_score: score}, \
             workspace = $ws, state = 'desired', *service{workspace, context, name, state, description @ 'NOW'}",
            params.clone(), ScriptMutability::Immutable,
        ) {
            for row in &r.rows {
                results.push(json!({"kind": "service", "context": dv_str(&row[0]), "name": dv_str(&row[1]), "description": dv_str(&row[2]), "score": dv_str(&row[3])}));
            }
        }

        // Search events
        if let Ok(r) = self.run_script(
            "?[context, name, description, score] := ~event:fts{workspace, context, name, state | query: $q, k: $k, bind_score: score}, \
             workspace = $ws, state = 'desired', *event{workspace, context, name, state, description @ 'NOW'}",
            params.clone(), ScriptMutability::Immutable,
        ) {
            for row in &r.rows {
                results.push(json!({"kind": "event", "context": dv_str(&row[0]), "name": dv_str(&row[1]), "description": dv_str(&row[2]), "score": dv_str(&row[3])}));
            }
        }

        // Search decision titles
        if let Ok(r) = self.run_script(
            "?[id, title, score] := ~architectural_decision:title_fts{workspace, id, state | query: $q, k: $k, bind_score: score}, \
             workspace = $ws, state = 'desired', *architectural_decision{workspace, id, state, title @ 'NOW'}",
            params.clone(), ScriptMutability::Immutable,
        ) {
            for row in &r.rows {
                results.push(json!({"kind": "architectural_decision", "id": dv_str(&row[0]), "title": dv_str(&row[1]), "score": dv_str(&row[2])}));
            }
        }

        // Search decision rationales
        if let Ok(r) = self.run_script(
            "?[id, title, rationale, score] := ~architectural_decision:rationale_fts{workspace, id, state | query: $q, k: $k, bind_score: score}, \
             workspace = $ws, state = 'desired', *architectural_decision{workspace, id, state, title, rationale @ 'NOW'}",
            params.clone(), ScriptMutability::Immutable,
        ) {
            for row in &r.rows {
                // Avoid duplicate if already found by title
                let id = dv_str(&row[0]);
                if !results.iter().any(|r| r["kind"] == "architectural_decision" && r["id"] == id) {
                    results.push(json!({"kind": "architectural_decision", "id": id, "title": dv_str(&row[1]), "rationale_match": dv_str(&row[2]), "score": dv_str(&row[3])}));
                }
            }
        }

        // Search invariant text
        if let Ok(r) = self.run_script(
            "?[context, entity, text, score] := ~invariant:text_fts{workspace, context, entity, idx, state | query: $q, k: $k, bind_score: score}, \
             workspace = $ws, state = 'desired', *invariant{workspace, context, entity, idx, state, text @ 'NOW'}",
            params.clone(), ScriptMutability::Immutable,
        ) {
            for row in &r.rows {
                results.push(json!({"kind": "invariant", "context": dv_str(&row[0]), "entity": dv_str(&row[1]), "text": dv_str(&row[2]), "score": dv_str(&row[3])}));
            }
        }

        if results.is_empty() && !query.trim().is_empty() {
            let needle = query.to_lowercase();
            if let Some(model) = self.load_actual(&ws)? {
                for context in &model.bounded_contexts {
                    if text_matches(&context.name, &needle)
                        || text_matches(&context.description, &needle)
                    {
                        results.push(json!({
                            "kind": "context",
                            "name": &context.name,
                            "description": &context.description,
                            "score": "1.0",
                            "search_mode": "model_scan",
                        }));
                    }
                    for entity in &context.entities {
                        if text_matches(&entity.name, &needle)
                            || text_matches(&entity.description, &needle)
                        {
                            results.push(json!({
                                "kind": "entity",
                                "context": &context.name,
                                "name": &entity.name,
                                "description": &entity.description,
                                "score": "1.0",
                                "search_mode": "model_scan",
                            }));
                        }
                        for invariant in &entity.invariants {
                            if text_matches(invariant, &needle) {
                                results.push(json!({
                                    "kind": "invariant",
                                    "context": &context.name,
                                    "entity": &entity.name,
                                    "text": invariant,
                                    "score": "1.0",
                                    "search_mode": "model_scan",
                                }));
                            }
                        }
                    }
                    for service in &context.services {
                        if text_matches(&service.name, &needle)
                            || text_matches(&service.description, &needle)
                        {
                            results.push(json!({
                                "kind": "service",
                                "context": &context.name,
                                "name": &service.name,
                                "description": &service.description,
                                "score": "1.0",
                                "search_mode": "model_scan",
                            }));
                        }
                    }
                    for event in &context.events {
                        if text_matches(&event.name, &needle)
                            || text_matches(&event.description, &needle)
                        {
                            results.push(json!({
                                "kind": "event",
                                "context": &context.name,
                                "name": &event.name,
                                "description": &event.description,
                                "score": "1.0",
                                "search_mode": "model_scan",
                            }));
                        }
                    }
                }
                for decision in &model.architectural_decisions {
                    if text_matches(&decision.id, &needle)
                        || text_matches(&decision.title, &needle)
                        || text_matches(&decision.rationale, &needle)
                    {
                        results.push(json!({
                            "kind": "architectural_decision",
                            "id": &decision.id,
                            "title": &decision.title,
                            "rationale_match": &decision.rationale,
                            "score": "1.0",
                            "search_mode": "model_scan",
                        }));
                    }
                }
            }
        }

        results.sort_by(|a, b| {
            let sa: f64 = a["score"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            let sb: f64 = b["score"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(json!({
            "query": query,
            "results": results,
            "count": results.len(),
        }))
    }

    // ── Graph Algorithms (CozoDB Fixed Rules) ─────────────────────────────

    /// Compute PageRank over the context dependency graph.
    pub fn pagerank(&self, workspace: &str) -> Result<Vec<(String, f64)>> {
        let params = params_map(&[("ws", workspace)]);
        let result = self.run_script(
            "edges[from, to] := *context_dep{workspace: $ws, from_ctx: from, to_ctx: to, state: 'desired' @ 'NOW'} \
             ?[node, rank] <~ PageRank(edges[])",
            params,
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("pagerank: {:?}", e))?;
        let mut ranked: Vec<(String, f64)> = result
            .rows
            .iter()
            .map(|r| {
                let rank = match &r[1] {
                    cozo::DataValue::Num(cozo::Num::Float(f)) => *f,
                    cozo::DataValue::Num(cozo::Num::Int(i)) => *i as f64,
                    _ => 0.0,
                };
                (dv_str(&r[0]), rank)
            })
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(ranked)
    }

    /// Compute community detection (Louvain) over the context dependency graph.
    pub fn community_detection(&self, workspace: &str) -> Result<Vec<(String, u64)>> {
        let params = params_map(&[("ws", workspace)]);
        let result = self.run_script(
            "edges[from, to] := *context_dep{workspace: $ws, from_ctx: from, to_ctx: to, state: 'desired' @ 'NOW'} \
             ?[node, community] <~ CommunityDetectionLouvain(edges[])",
            params,
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("community_detection: {:?}", e))?;
        Ok(result
            .rows
            .iter()
            .map(|r| {
                let community = match &r[1] {
                    cozo::DataValue::Num(cozo::Num::Int(i)) => *i as u64,
                    _ => 0,
                };
                (dv_str(&r[0]), community)
            })
            .collect())
    }

    /// Compute betweenness centrality over the context dependency graph.
    pub fn betweenness_centrality(&self, workspace: &str) -> Result<Vec<(String, f64)>> {
        let params = params_map(&[("ws", workspace)]);
        let result = self.run_script(
            "edges[from, to] := *context_dep{workspace: $ws, from_ctx: from, to_ctx: to, state: 'desired' @ 'NOW'} \
             ?[node, centrality] <~ BetweennessCentrality(edges[])",
            params,
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("betweenness_centrality: {:?}", e))?;
        let mut ranked: Vec<(String, f64)> = result
            .rows
            .iter()
            .map(|r| {
                let centrality = match &r[1] {
                    cozo::DataValue::Num(cozo::Num::Float(f)) => *f,
                    cozo::DataValue::Num(cozo::Num::Int(i)) => *i as f64,
                    _ => 0.0,
                };
                (dv_str(&r[0]), centrality)
            })
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(ranked)
    }

    /// Compute in-degree and out-degree for each context in the dependency graph.
    pub fn degree_centrality(&self, workspace: &str) -> Result<Vec<(String, u32, u32)>> {
        let params = params_map(&[("ws", workspace)]);
        let result = self.run_script(
            "ctx_now[ctx] := *context{workspace: $ws, name: ctx, state: 'desired' @ 'NOW'} \
             dep_from[ctx] := *context_dep{workspace: $ws, from_ctx: ctx, state: 'desired' @ 'NOW'} \
             dep_to[ctx] := *context_dep{workspace: $ws, to_ctx: ctx, state: 'desired' @ 'NOW'} \
             out_deg[ctx, count(to)] := *context_dep{workspace: $ws, from_ctx: ctx, to_ctx: to, state: 'desired' @ 'NOW'} \
             out_deg[ctx, 0] := ctx_now[ctx], not dep_from[ctx] \
             in_deg[ctx, count(from)] := *context_dep{workspace: $ws, to_ctx: ctx, from_ctx: from, state: 'desired' @ 'NOW'} \
             in_deg[ctx, 0] := ctx_now[ctx], not dep_to[ctx] \
             ?[ctx, in_d, out_d] := in_deg[ctx, in_d], out_deg[ctx, out_d]",
            params,
            ScriptMutability::Immutable,
        ).map_err(|e| anyhow::anyhow!("degree_centrality: {:?}", e))?;
        Ok(result
            .rows
            .iter()
            .map(|r| (dv_str(&r[0]), dv_u32(&r[1]), dv_u32(&r[2])))
            .collect())
    }

    /// Compute topological ordering of context dependencies (if acyclic).
    pub fn topological_order(&self, workspace: &str) -> Result<serde_json::Value> {
        let params = params_map(&[("ws", workspace)]);
        let result = self.run_script(
            "edges[from, to] := *context_dep{workspace: $ws, from_ctx: from, to_ctx: to, state: 'desired' @ 'NOW'} \
             nodes[name] := *context{workspace: $ws, name, state: 'desired' @ 'NOW'} \
             ?[node, order] <~ TopologicalSort(nodes[], edges[])",
            params,
            ScriptMutability::Immutable,
        );
        match result {
            Ok(r) => {
                let mut items: Vec<(String, i64)> = r
                    .rows
                    .iter()
                    .map(|row| (dv_str(&row[0]), dv_i64(&row[1])))
                    .collect();
                items.sort_by_key(|(_, order)| *order);
                Ok(json!({
                    "status": "acyclic",
                    "order": items.iter().map(|(n, o)| json!({"context": n, "order": o})).collect::<Vec<_>>(),
                }))
            }
            Err(_) => {
                let cycles = self.circular_deps(workspace)?;
                Ok(json!({
                    "status": "cyclic",
                    "message": "Graph contains cycles; topological sort is not possible.",
                    "cycles": cycles.iter().map(|(a, b)| json!({"from": a, "to": b})).collect::<Vec<_>>(),
                }))
            }
        }
    }

    // ── Metalayer: Model Health ────────────────────────────────────────────

    pub fn model_health(&self, workspace: &str) -> Result<ModelHealth> {
        let canonical = canonicalize_path(workspace);
        let circular = self.circular_deps(&canonical)?;
        let violations = self.layer_violations(&canonical)?;
        let missing_invariants = self.aggregate_roots_without_invariants(&canonical)?;
        let orphans = self.orphan_contexts(&canonical)?;
        let complexity = self.context_complexity(&canonical)?;
        let god_contexts: Vec<String> = complexity
            .iter()
            .filter(|c| c.entity_count + c.service_count > 10)
            .map(|c| c.context.clone())
            .collect();
        let unsourced_events = self.unsourced_events(&canonical)?;

        // Graph algorithms via CozoDB fixed rules
        let bottleneck_contexts: Vec<String> = match self.betweenness_centrality(&canonical) {
            Ok(rows) => rows
                .into_iter()
                .filter(|(_, c)| *c > 0.0)
                .map(|(name, _)| name)
                .collect(),
            Err(e) => {
                tracing::debug!("Betweenness centrality unavailable for model_health: {e}");
                Vec::new()
            }
        };
        let communities = match self.community_detection(&canonical) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::debug!("Community detection unavailable for model_health: {e}");
                Vec::new()
            }
        };

        let critical = circular.len() + violations.len();
        let warnings = missing_invariants.len() + god_contexts.len() + unsourced_events.len();
        let info = orphans.len();
        let score = (100i32 - (critical as i32 * 20) - (warnings as i32 * 5) - (info as i32 * 2))
            .max(0) as u32;

        Ok(ModelHealth {
            score,
            circular_deps: circular.into_iter().map(|(a, b)| [a, b]).collect(),
            layer_violations: violations
                .into_iter()
                .map(|(ctx, svc, dep)| LayerViolation {
                    context: ctx,
                    domain_service: svc,
                    infra_dependency: dep,
                })
                .collect(),
            missing_invariants: missing_invariants
                .into_iter()
                .map(|(ctx, ent)| [ctx, ent])
                .collect(),
            orphan_contexts: orphans,
            god_contexts,
            unsourced_events,
            complexity,
            bottleneck_contexts,
            communities: communities
                .into_iter()
                .map(|(name, cid)| CommunityMembership {
                    context: name,
                    community: cid,
                })
                .collect(),
        })
    }

    fn orphan_contexts(&self, workspace: &str) -> Result<Vec<String>> {
        let params = params_map(&[("ws", workspace)]);
        let result = self
            .run_script(
                "has_dep[ctx] := *context_dep{workspace: $ws, from_ctx: ctx, state: 'desired' @ 'NOW'} \
                 has_dep[ctx] := *context_dep{workspace: $ws, to_ctx: ctx, state: 'desired' @ 'NOW'} \
                 ?[name] := *context{workspace: $ws, name, state: 'desired' @ 'NOW'}, not has_dep[name]",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("orphan_contexts: {:?}", e))?;
        Ok(result.rows.iter().map(|r| dv_str(&r[0])).collect())
    }

    fn context_complexity(&self, workspace: &str) -> Result<Vec<ContextComplexity>> {
        let params = params_map(&[("ws", workspace)]);
        let contexts = self
            .run_script(
                "?[ctx] := *context{workspace: $ws, name: ctx, state: 'desired' @ 'NOW'}",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("context_complexity contexts: {:?}", e))?;

        let mut complexity = Vec::with_capacity(contexts.rows.len());
        for row in contexts.rows {
            let context = dv_str(&row[0]);
            let count_params = params_map(&[("ws", workspace), ("ctx", &context)]);
            let entity_count = self
                .run_script(
                    "?[name] := *entity{workspace: $ws, context: $ctx, name, state: 'desired' @ 'NOW'}",
                    count_params.clone(),
                    ScriptMutability::Immutable,
                )
                .map_err(|e| anyhow::anyhow!("context_complexity entity count: {:?}", e))?
                .rows
                .len() as u32;
            let service_count = self
                .run_script(
                    "?[name] := *service{workspace: $ws, context: $ctx, name, state: 'desired' @ 'NOW'}",
                    count_params.clone(),
                    ScriptMutability::Immutable,
                )
                .map_err(|e| anyhow::anyhow!("context_complexity service count: {:?}", e))?
                .rows
                .len() as u32;
            let event_count = self
                .run_script(
                    "?[name] := *event{workspace: $ws, context: $ctx, name, state: 'desired' @ 'NOW'}",
                    count_params.clone(),
                    ScriptMutability::Immutable,
                )
                .map_err(|e| anyhow::anyhow!("context_complexity event count: {:?}", e))?
                .rows
                .len() as u32;
            let dep_count = self
                .run_script(
                    "?[dep] := *context_dep{workspace: $ws, from_ctx: $ctx, to_ctx: dep, state: 'desired' @ 'NOW'}",
                    count_params,
                    ScriptMutability::Immutable,
                )
                .map_err(|e| anyhow::anyhow!("context_complexity dependency count: {:?}", e))?
                .rows
                .len() as u32;
            complexity.push(ContextComplexity {
                context,
                entity_count,
                service_count,
                event_count,
                dep_count,
            });
        }

        Ok(complexity)
    }

    fn unsourced_events(&self, workspace: &str) -> Result<Vec<[String; 2]>> {
        let params = params_map(&[("ws", workspace)]);
        let result = self
            .run_script(
                "?[context, name] := *event{workspace: $ws, context, name, source: '', state: 'desired' @ 'NOW'}",
                params,
                ScriptMutability::Immutable,
            )
            .map_err(|e| anyhow::anyhow!("unsourced_events: {:?}", e))?;
        Ok(result
            .rows
            .iter()
            .map(|r| [dv_str(&r[0]), dv_str(&r[1])])
            .collect())
    }
}

// ── Data Types ─────────────────────────────────────────────────────────────

/// Metadata about a stored project.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub workspace_path: String,
    pub project_name: String,
    pub updated_at: String,
}

/// Comprehensive model health report computed via Datalog inference.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelHealth {
    pub score: u32,
    pub circular_deps: Vec<[String; 2]>,
    pub layer_violations: Vec<LayerViolation>,
    pub missing_invariants: Vec<[String; 2]>,
    pub orphan_contexts: Vec<String>,
    pub god_contexts: Vec<String>,
    pub unsourced_events: Vec<[String; 2]>,
    pub complexity: Vec<ContextComplexity>,
    pub bottleneck_contexts: Vec<String>,
    pub communities: Vec<CommunityMembership>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LayerViolation {
    pub context: String,
    pub domain_service: String,
    pub infra_dependency: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ContextComplexity {
    pub context: String,
    pub entity_count: u32,
    pub service_count: u32,
    pub event_count: u32,
    pub dep_count: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CommunityMembership {
    pub context: String,
    pub community: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FactSnapshotSummary {
    pub knowledge_kind: String,
    pub state: String,
    pub available: bool,
    pub snapshot_timestamp_us: Option<i64>,
    pub context_count: usize,
    pub entity_count: usize,
    pub value_object_count: usize,
    pub service_count: usize,
    pub repository_count: usize,
    pub event_count: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DriftFreshness {
    pub available: bool,
    pub status: String,
    pub computed_at_us: Option<i64>,
    pub basis_timestamp_us: Option<i64>,
    pub entry_count: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TruthMaintenanceReport {
    pub asserted: FactSnapshotSummary,
    pub scanned: FactSnapshotSummary,
    pub drift: DriftFreshness,
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningProvenance {
    pub source: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningDerivation {
    pub rule: String,
    #[serde(default)]
    pub derived_from: Vec<String>,
    pub witness_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningAssumption {
    pub assumption_kind: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningSupportEdge {
    pub support_kind: String,
    pub summary: String,
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningDependency {
    pub dependency_kind: String,
    pub dependency_state: String,
    pub basis_timestamp_us: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningJustification {
    pub fact_kind: String,
    pub fact_key: String,
    pub fact_state: String,
    pub basis_timestamp_us: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningFactRef {
    pub fact_kind: String,
    pub fact_key: String,
    pub fact_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedReasoningClaim {
    pub claim_id: String,
    pub claim_kind: String,
    pub subject: String,
    pub status: String,
    pub summary: String,
    pub payload: Value,
    pub provenance: ReasoningProvenance,
    pub stale: bool,
    pub computed_at_us: i64,
    #[serde(default)]
    pub derivations: Vec<ReasoningDerivation>,
    #[serde(default)]
    pub assumptions: Vec<ReasoningAssumption>,
    #[serde(default)]
    pub supports: Vec<ReasoningSupportEdge>,
    #[serde(default)]
    pub dependencies: Vec<ReasoningDependency>,
    #[serde(default)]
    pub justifications: Vec<ReasoningJustification>,
}

impl PersistedReasoningClaim {
    pub fn proof_json(&self) -> Option<Value> {
        match self.derivations.as_slice() {
            [] => None,
            [single] => Some(json!({
                "rule": single.rule,
                "derived_from": single.derived_from,
                "witness_count": single.witness_count,
            })),
            many => Some(Value::Array(
                many.iter()
                    .map(|derivation| {
                        json!({
                            "rule": derivation.rule,
                            "derived_from": derivation.derived_from,
                            "witness_count": derivation.witness_count,
                        })
                    })
                    .collect(),
            )),
        }
    }

    pub fn evidence_json(&self) -> Option<Value> {
        match self.supports.as_slice() {
            [] => None,
            [single] => Some(single.detail.clone()),
            many => Some(Value::Array(
                many.iter()
                    .map(|support| {
                        json!({
                            "support_kind": support.support_kind,
                            "summary": support.summary,
                            "detail": support.detail,
                        })
                    })
                    .collect(),
            )),
        }
    }

    pub fn limitation_texts(&self) -> Vec<String> {
        self.assumptions
            .iter()
            .filter(|assumption| assumption.assumption_kind == "limitation")
            .map(|assumption| assumption.text.clone())
            .collect()
    }

    pub fn assumption_texts(&self) -> Vec<String> {
        self.assumptions
            .iter()
            .filter(|assumption| assumption.assumption_kind != "limitation")
            .map(|assumption| assumption.text.clone())
            .collect()
    }
}

// ── Helper Functions ───────────────────────────────────────────────────────

fn summarize_fact_snapshot(
    knowledge_kind: &str,
    state: &str,
    snapshot_timestamp_us: Option<i64>,
    model: Option<&DomainModel>,
) -> FactSnapshotSummary {
    let Some(model) = model else {
        return FactSnapshotSummary {
            knowledge_kind: knowledge_kind.to_string(),
            state: state.to_string(),
            available: false,
            snapshot_timestamp_us,
            context_count: 0,
            entity_count: 0,
            value_object_count: 0,
            service_count: 0,
            repository_count: 0,
            event_count: 0,
        };
    };

    FactSnapshotSummary {
        knowledge_kind: knowledge_kind.to_string(),
        state: state.to_string(),
        available: true,
        snapshot_timestamp_us,
        context_count: model.bounded_contexts.len(),
        entity_count: model
            .bounded_contexts
            .iter()
            .map(|context| context.entities.len())
            .sum(),
        value_object_count: model
            .bounded_contexts
            .iter()
            .map(|context| context.value_objects.len())
            .sum(),
        service_count: model
            .bounded_contexts
            .iter()
            .map(|context| context.services.len())
            .sum(),
        repository_count: model
            .bounded_contexts
            .iter()
            .map(|context| context.repositories.len())
            .sum(),
        event_count: model
            .bounded_contexts
            .iter()
            .map(|context| context.events.len())
            .sum(),
    }
}

/// Normalize workspace path for consistent keying.
pub fn canonicalize_path(path: &str) -> String {
    let normalized = path.trim_end_matches('/');
    match std::fs::canonicalize(normalized) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => normalized.to_string(),
    }
}

fn strip_state_dimension_from_script(script: &str) -> String {
    let mut stripped = script.to_string();

    for value in ["$st", "$from", "$to", "'desired'", "'actual'"] {
        for pattern in [
            format!(", state: {value}"),
            format!("state: {value}, "),
            format!(", state = {value}"),
            format!("state = {value}, "),
        ] {
            stripped = stripped.replace(&pattern, "");
        }
    }

    for value in ["$st", "'desired'", "'actual'"] {
        for (pattern, replacement) in [
            (format!(", {value},"), ",".to_string()),
            (format!(", {value}]]"), "]]".to_string()),
            (format!(", {value}]"), "]".to_string()),
            (format!("[[{value}, "), "[[".to_string()),
            (format!("[{value}, "), "[".to_string()),
        ] {
            stripped = stripped.replace(&pattern, &replacement);
        }
    }

    for (pattern, replacement) in [
        (", state,", ","),
        (", state]", "]"),
        (", state =>", " =>"),
        (", state @", " @"),
        (", state }", " }"),
        (", state}", "}"),
        ("[state, ", "["),
        ("{state, ", "{"),
        ("(state, ", "("),
    ] {
        stripped = stripped.replace(pattern, replacement);
    }

    loop {
        let cleaned = stripped
            .replace(", ,", ",")
            .replace("[,", "[")
            .replace("{,", "{")
            .replace(",]", "]")
            .replace(",}", "}")
            .replace(", }", " }")
            .replace("(,", "(")
            .replace(",)", ")");
        if cleaned == stripped {
            return stripped;
        }
        stripped = cleaned;
    }
}

fn params_map(pairs: &[(&str, &str)]) -> BTreeMap<String, cozo::DataValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), cozo::DataValue::Str(v.to_string().into())))
        .collect()
}

fn int_dv(n: i64) -> cozo::DataValue {
    cozo::DataValue::Num(cozo::Num::Int(n))
}

fn text_matches(haystack: &str, needle_lowercase: &str) -> bool {
    haystack.to_lowercase().contains(needle_lowercase)
}

/// Extract display string from a DataValue.
fn dv_str(val: &cozo::DataValue) -> String {
    match val {
        cozo::DataValue::Null => String::new(),
        cozo::DataValue::Bool(b) => b.to_string(),
        cozo::DataValue::Num(n) => match n {
            cozo::Num::Int(i) => i.to_string(),
            cozo::Num::Float(f) => f.to_string(),
        },
        cozo::DataValue::Str(s) => s.to_string(),
        cozo::DataValue::List(l) => {
            let items: Vec<String> = l.iter().map(dv_str).collect();
            format!("[{}]", items.join(", "))
        }
        _ => format!("{:?}", val),
    }
}

fn dv_u32(val: &cozo::DataValue) -> u32 {
    match val {
        cozo::DataValue::Num(cozo::Num::Int(i)) => *i as u32,
        cozo::DataValue::Num(cozo::Num::Float(f)) => *f as u32,
        _ => 0,
    }
}

fn dv_i64(val: &cozo::DataValue) -> i64 {
    match val {
        cozo::DataValue::Num(cozo::Num::Int(i)) => *i,
        cozo::DataValue::Num(cozo::Num::Float(f)) => *f as i64,
        _ => 0,
    }
}

fn dv_opt_string(val: &cozo::DataValue) -> Option<String> {
    let value = dv_str(val);
    if value.is_empty() { None } else { Some(value) }
}

fn dv_opt_usize(val: &cozo::DataValue) -> Option<usize> {
    match dv_i64(val) {
        n if n > 0 => Some(n as usize),
        _ => None,
    }
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs_per_day = 86400u64;
    let days = now / secs_per_day;
    let rem = now % secs_per_day;
    let hours = rem / 3600;
    let minutes = (rem % 3600) / 60;
    let seconds = rem % 60;
    let (year, month, day) = days_to_date(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

fn days_to_date(days: u64) -> (u64, u64, u64) {
    let mut y = 1970;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let month_days: &[u64] = if is_leap(y) {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 1u64;
    for &md in month_days {
        if remaining < md {
            break;
        }
        remaining -= md;
        m += 1;
    }
    (y, m, remaining + 1)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn test_model(name: &str) -> DomainModel {
        DomainModel {
            name: name.into(),
            description: "Test project".into(),
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
        }
    }

    fn full_model() -> DomainModel {
        DomainModel {
            name: "FullTest".into(),
            description: "Full test model".into(),
            bounded_contexts: vec![
                BoundedContext {
                    api_endpoints: vec![],
                    name: "Identity".into(),
                    description: "Auth context".into(),
                    module_path: "src/identity".into(),
                    ownership: Ownership::default(),
                    aggregates: vec![],
                    policies: vec![],
                    read_models: vec![],
                    entities: vec![Entity {
                        name: "User".into(),
                        description: "A user".into(),
                        aggregate_root: true,
                        fields: vec![Field {
                            name: "id".into(),
                            field_type: "UserId".into(),
                            required: true,
                            description: "".into(),
                        }],
                        methods: vec![],
                        invariants: vec!["Email must be unique".into()],
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    }],
                    value_objects: vec![],
                    services: vec![Service {
                        name: "AuthService".into(),
                        description: "Handles auth".into(),
                        kind: ServiceKind::Application,
                        methods: vec![],
                        dependencies: vec![],
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    }],
                    repositories: vec![],
                    events: vec![],
                    modules: vec![],
                    dependencies: vec![],
                },
                BoundedContext {
                    api_endpoints: vec![],
                    name: "Billing".into(),
                    description: "Billing context".into(),
                    module_path: "src/billing".into(),
                    ownership: Ownership::default(),
                    aggregates: vec![],
                    policies: vec![],
                    read_models: vec![],
                    entities: vec![Entity {
                        name: "Subscription".into(),
                        description: "A subscription".into(),
                        aggregate_root: false,
                        fields: vec![],
                        methods: vec![],
                        invariants: vec![],
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    }],
                    value_objects: vec![],
                    services: vec![],
                    repositories: vec![],
                    events: vec![],
                    modules: vec![],
                    dependencies: vec!["Identity".into()],
                },
            ],
            external_systems: vec![],
            architectural_decisions: vec![],
            ownership: Ownership::default(),
            rules: vec![ArchitecturalRule {
                id: "LAYER-001".into(),
                description: "Domain must not depend on infra".into(),
                severity: Severity::Error,
                scope: "domain".into(),
            }],
            tech_stack: TechStack::default(),
            conventions: Conventions::default(),
            ast_edges: vec![],
            source_files: vec![],
            symbols: vec![],
            import_edges: vec![],
            call_edges: vec![],
        }
    }

    /// Model with rich sub-structures to exercise field/method/param round-tripping.
    fn rich_model() -> DomainModel {
        DomainModel {
            name: "RichTest".into(),
            description: "Rich model with all sub-structures".into(),
            bounded_contexts: vec![BoundedContext {
                api_endpoints: vec![],
                name: "Catalog".into(),
                description: "Product catalog".into(),
                module_path: "src/catalog".into(),
                ownership: Ownership::default(),
                aggregates: vec![],
                policies: vec![],
                read_models: vec![],
                entities: vec![Entity {
                    name: "Product".into(),
                    description: "A product".into(),
                    aggregate_root: true,
                    fields: vec![
                        Field {
                            name: "id".into(),
                            field_type: "ProductId".into(),
                            required: true,
                            description: "Primary key".into(),
                        },
                        Field {
                            name: "name".into(),
                            field_type: "String".into(),
                            required: true,
                            description: "".into(),
                        },
                        Field {
                            name: "price".into(),
                            field_type: "Money".into(),
                            required: false,
                            description: "".into(),
                        },
                    ],
                    methods: vec![
                        Method {
                            name: "create".into(),
                            description: "Create a new product".into(),
                            parameters: vec![
                                Field {
                                    name: "name".into(),
                                    field_type: "String".into(),
                                    required: true,
                                    description: "".into(),
                                },
                                Field {
                                    name: "price".into(),
                                    field_type: "Money".into(),
                                    required: true,
                                    description: "".into(),
                                },
                            ],
                            return_type: "Product".into(),
                            file_path: None,
                            start_line: None,
                            end_line: None,
                        },
                        Method {
                            name: "update_price".into(),
                            description: "".into(),
                            parameters: vec![Field {
                                name: "new_price".into(),
                                field_type: "Money".into(),
                                required: true,
                                description: "".into(),
                            }],
                            return_type: "".into(),
                            file_path: None,
                            start_line: None,
                            end_line: None,
                        },
                    ],
                    invariants: vec![
                        "Name must not be empty".into(),
                        "Price must be positive".into(),
                    ],
                    file_path: Some("src/catalog/domain/product.rs".into()),
                    start_line: Some(12),
                    end_line: Some(82),
                }],
                value_objects: vec![ValueObject {
                    name: "Money".into(),
                    description: "Monetary value".into(),
                    fields: vec![
                        Field {
                            name: "amount".into(),
                            field_type: "Decimal".into(),
                            required: true,
                            description: "".into(),
                        },
                        Field {
                            name: "currency".into(),
                            field_type: "String".into(),
                            required: true,
                            description: "".into(),
                        },
                    ],
                    validation_rules: vec![
                        "Amount must be non-negative".into(),
                        "Currency must be ISO 4217".into(),
                    ],
                    file_path: Some("src/catalog/domain/money.rs".into()),
                    start_line: Some(3),
                    end_line: Some(27),
                }],
                services: vec![Service {
                    name: "CatalogService".into(),
                    description: "Application service".into(),
                    kind: ServiceKind::Application,
                    methods: vec![Method {
                        name: "list_products".into(),
                        description: "List all products".into(),
                        parameters: vec![],
                        return_type: "Vec<Product>".into(),
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    }],
                    dependencies: vec![],
                    file_path: Some("src/catalog/application/catalog_service.rs".into()),
                    start_line: Some(8),
                    end_line: Some(34),
                }],
                repositories: vec![Repository {
                    name: "ProductRepository".into(),
                    aggregate: "Product".into(),
                    methods: vec![
                        Method {
                            name: "find_by_id".into(),
                            description: "".into(),
                            parameters: vec![Field {
                                name: "id".into(),
                                field_type: "ProductId".into(),
                                required: true,
                                description: "".into(),
                            }],
                            return_type: "Option<Product>".into(),
                            file_path: None,
                            start_line: None,
                            end_line: None,
                        },
                        Method {
                            name: "save".into(),
                            description: "".into(),
                            parameters: vec![Field {
                                name: "product".into(),
                                field_type: "Product".into(),
                                required: true,
                                description: "".into(),
                            }],
                            return_type: "".into(),
                            file_path: None,
                            start_line: None,
                            end_line: None,
                        },
                    ],
                    file_path: Some("src/catalog/infrastructure/product_repository.rs".into()),
                    start_line: Some(5),
                    end_line: Some(41),
                }],
                events: vec![DomainEvent {
                    name: "ProductCreated".into(),
                    description: "Emitted when a product is created".into(),
                    source: "Product".into(),
                    fields: vec![
                        Field {
                            name: "product_id".into(),
                            field_type: "ProductId".into(),
                            required: true,
                            description: "".into(),
                        },
                        Field {
                            name: "name".into(),
                            field_type: "String".into(),
                            required: true,
                            description: "".into(),
                        },
                    ],
                    file_path: Some("src/catalog/domain/events.rs".into()),
                    start_line: Some(4),
                    end_line: Some(18),
                }],
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
        }
    }

    fn temp_store() -> Store {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = temp_dir().join(format!(
            "axon_cozo_test_{}_{}.db",
            std::process::id(),
            id
        ));
        Store::open(&path).unwrap()
    }

    #[test]
    fn test_save_and_load() {
        let store = temp_store();
        let model = full_model();
        store.save_desired("/tmp/test-save", &model).unwrap();
        let loaded = store.load_desired("/tmp/test-save").unwrap().unwrap();
        assert_eq!(loaded.name, "FullTest");
        assert_eq!(loaded.bounded_contexts.len(), 2);
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        assert_eq!(identity.entities.len(), 1);
        assert_eq!(identity.entities[0].fields.len(), 1);
        assert_eq!(identity.entities[0].fields[0].name, "id");
        assert_eq!(identity.entities[0].fields[0].field_type, "UserId");
        assert!(identity.entities[0].fields[0].required);
        assert_eq!(loaded.rules.len(), 1);
    }

    #[test]
    fn test_rich_model_round_trip() {
        let store = temp_store();
        let model = rich_model();
        store.save_desired("/tmp/test-rich", &model).unwrap();
        let loaded = store.load_desired("/tmp/test-rich").unwrap().unwrap();

        let catalog = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Catalog")
            .unwrap();

        // Entity fields
        let product = catalog
            .entities
            .iter()
            .find(|e| e.name == "Product")
            .unwrap();
        assert_eq!(
            product.file_path.as_deref(),
            Some("src/catalog/domain/product.rs")
        );
        assert_eq!(product.start_line, Some(12));
        assert_eq!(product.end_line, Some(82));
        assert_eq!(product.fields.len(), 3);
        assert_eq!(product.fields[0].name, "id");
        assert_eq!(product.fields[1].name, "name");
        assert_eq!(product.fields[2].name, "price");
        assert!(!product.fields[2].required);

        // Entity methods + parameters
        assert_eq!(product.methods.len(), 2);
        assert_eq!(product.methods[0].name, "create");
        assert_eq!(product.methods[0].return_type, "Product");
        assert_eq!(product.methods[0].parameters.len(), 2);
        assert_eq!(product.methods[0].parameters[0].name, "name");
        assert_eq!(product.methods[0].parameters[1].name, "price");
        assert_eq!(product.methods[1].name, "update_price");
        assert_eq!(product.methods[1].parameters.len(), 1);

        // Entity invariants (ordered)
        assert_eq!(product.invariants.len(), 2);
        assert_eq!(product.invariants[0], "Name must not be empty");
        assert_eq!(product.invariants[1], "Price must be positive");

        // Value object fields + validation rules
        let money = catalog
            .value_objects
            .iter()
            .find(|v| v.name == "Money")
            .unwrap();
        assert_eq!(
            money.file_path.as_deref(),
            Some("src/catalog/domain/money.rs")
        );
        assert_eq!(money.start_line, Some(3));
        assert_eq!(money.end_line, Some(27));
        assert_eq!(money.fields.len(), 2);
        assert_eq!(money.fields[0].name, "amount");
        assert_eq!(money.validation_rules.len(), 2);
        assert_eq!(money.validation_rules[0], "Amount must be non-negative");
        assert_eq!(money.validation_rules[1], "Currency must be ISO 4217");

        // Service methods
        let cat_svc = catalog
            .services
            .iter()
            .find(|s| s.name == "CatalogService")
            .unwrap();
        assert_eq!(
            cat_svc.file_path.as_deref(),
            Some("src/catalog/application/catalog_service.rs")
        );
        assert_eq!(cat_svc.start_line, Some(8));
        assert_eq!(cat_svc.end_line, Some(34));
        assert_eq!(cat_svc.methods.len(), 1);
        assert_eq!(cat_svc.methods[0].name, "list_products");
        assert_eq!(cat_svc.methods[0].return_type, "Vec<Product>");
        assert!(cat_svc.methods[0].parameters.is_empty());

        // Repository methods + params
        let repo = catalog
            .repositories
            .iter()
            .find(|r| r.name == "ProductRepository")
            .unwrap();
        assert_eq!(
            repo.file_path.as_deref(),
            Some("src/catalog/infrastructure/product_repository.rs")
        );
        assert_eq!(repo.start_line, Some(5));
        assert_eq!(repo.end_line, Some(41));
        assert_eq!(repo.aggregate, "Product");
        assert_eq!(repo.methods.len(), 2);
        assert_eq!(repo.methods[0].name, "find_by_id");
        assert_eq!(repo.methods[0].parameters.len(), 1);
        assert_eq!(repo.methods[0].parameters[0].name, "id");
        assert_eq!(repo.methods[1].name, "save");

        // Event fields
        let evt = catalog
            .events
            .iter()
            .find(|e| e.name == "ProductCreated")
            .unwrap();
        assert_eq!(
            evt.file_path.as_deref(),
            Some("src/catalog/domain/events.rs")
        );
        assert_eq!(evt.start_line, Some(4));
        assert_eq!(evt.end_line, Some(18));
        assert_eq!(evt.fields.len(), 2);
        assert_eq!(evt.fields[0].name, "product_id");
        assert_eq!(evt.source, "Product");
    }

    #[test]
    fn test_rich_model_accept_and_reset() {
        let store = temp_store();
        let ws = "/tmp/test-rich-accept";
        store.save_desired(ws, &rich_model()).unwrap();
        store.accept(ws).unwrap();

        let actual = store.load_actual(ws).unwrap().unwrap();
        let cat = actual
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Catalog")
            .unwrap();
        assert_eq!(
            cat.entities[0].file_path.as_deref(),
            Some("src/catalog/domain/product.rs")
        );
        assert_eq!(cat.entities[0].start_line, Some(12));
        assert_eq!(cat.entities[0].end_line, Some(82));
        assert_eq!(cat.entities[0].fields.len(), 3);
        assert_eq!(cat.entities[0].methods.len(), 2);
        assert_eq!(cat.value_objects[0].fields.len(), 2);
        assert_eq!(cat.repositories[0].methods.len(), 2);
        assert_eq!(cat.events[0].fields.len(), 2);

        // Modify implemented graph; reset is an actual-first compatibility no-op.
        let mut modified = rich_model();
        modified.bounded_contexts[0].entities[0].fields.push(Field {
            name: "sku".into(),
            field_type: "String".into(),
            required: false,
            description: "".into(),
        });
        store.save_desired(ws, &modified).unwrap();
        let desired = store.load_desired(ws).unwrap().unwrap();
        assert_eq!(desired.bounded_contexts[0].entities[0].fields.len(), 4);

        let reset = store.reset(ws).unwrap().unwrap();
        assert_eq!(reset.bounded_contexts[0].entities[0].fields.len(), 4);
        assert_eq!(
            reset.bounded_contexts[0].entities[0].file_path.as_deref(),
            Some("src/catalog/domain/product.rs")
        );
        assert_eq!(reset.bounded_contexts[0].entities[0].start_line, Some(12));
        assert_eq!(reset.bounded_contexts[0].entities[0].end_line, Some(82));
    }

    #[test]
    fn test_upsert_entity_preserves_source_location() {
        let store = temp_store();
        let ws = "/tmp/test-upsert-entity-location";
        store.save_desired(ws, &rich_model()).unwrap();

        let mut product = store.query_entity(ws, "Catalog", "Product").unwrap();
        assert_eq!(
            product.file_path.as_deref(),
            Some("src/catalog/domain/product.rs")
        );
        assert_eq!(product.start_line, Some(12));
        assert_eq!(product.end_line, Some(82));

        product.description = "Updated product".into();
        store.upsert_entity(ws, "Catalog", &product).unwrap();

        let queried = store.query_entity(ws, "Catalog", "Product").unwrap();
        assert_eq!(queried.description, "Updated product");
        assert_eq!(
            queried.file_path.as_deref(),
            Some("src/catalog/domain/product.rs")
        );
        assert_eq!(queried.start_line, Some(12));
        assert_eq!(queried.end_line, Some(82));

        let loaded = store.load_desired(ws).unwrap().unwrap();
        let loaded_product = loaded.bounded_contexts[0]
            .entities
            .iter()
            .find(|entity| entity.name == "Product")
            .unwrap();
        assert_eq!(loaded_product.description, "Updated product");
        assert_eq!(
            loaded_product.file_path.as_deref(),
            Some("src/catalog/domain/product.rs")
        );
        assert_eq!(loaded_product.start_line, Some(12));
        assert_eq!(loaded_product.end_line, Some(82));
    }

    #[test]
    fn test_diff_graph_field_level() {
        let store = temp_store();
        let ws = "/tmp/test-diff-field";
        store.save_desired(ws, &rich_model()).unwrap();
        store.accept(ws).unwrap();

        // Add a field to Product
        let mut modified = rich_model();
        modified.bounded_contexts[0].entities[0].fields.push(Field {
            name: "sku".into(),
            field_type: "String".into(),
            required: false,
            description: "".into(),
        });
        store.save_desired(ws, &modified).unwrap();

        let diff = store.diff_graph(ws).unwrap();
        let changes = diff["pending_changes"].as_array().unwrap();
        assert!(!changes.is_empty());

        // Should contain a field-level add for "sku"
        let field_add = changes
            .iter()
            .find(|c| c["kind"] == "field" && c["name"] == "sku" && c["action"] == "add");
        assert!(
            field_add.is_some(),
            "Expected field-level diff for 'sku': {:?}",
            changes
        );
        let fa = field_add.unwrap();
        assert_eq!(fa["owner_kind"], "entity");
        assert_eq!(fa["owner"], "Product");
    }

    #[test]
    fn test_diff_graph_method_level() {
        let store = temp_store();
        let ws = "/tmp/test-diff-method";
        store.save_desired(ws, &rich_model()).unwrap();
        store.accept(ws).unwrap();

        // Add a method to CatalogService
        let mut modified = rich_model();
        modified.bounded_contexts[0].services[0]
            .methods
            .push(Method {
                name: "search".into(),
                description: "".into(),
                parameters: vec![],
                return_type: "Vec<Product>".into(),
                file_path: None,
                start_line: None,
                end_line: None,
            });
        store.save_desired(ws, &modified).unwrap();

        let diff = store.diff_graph(ws).unwrap();
        let changes = diff["pending_changes"].as_array().unwrap();

        let method_add = changes
            .iter()
            .find(|c| c["kind"] == "method" && c["name"] == "search" && c["action"] == "add");
        assert!(
            method_add.is_some(),
            "Expected method-level diff for 'search': {:?}",
            changes
        );
        assert_eq!(method_add.unwrap()["owner_kind"], "service");
    }

    #[test]
    fn test_datalog_query_fields() {
        let store = temp_store();
        let ws = "/tmp/test-datalog-fields";
        store.save_desired(ws, &rich_model()).unwrap();

        // Query all entity fields via raw Datalog
        let rows = store
            .run_datalog(
                "?[ctx, entity, field_name, field_type] := \
                    *field{workspace: $ws, context: ctx, owner_kind: 'entity', \
                           owner: entity, name: field_name, state: 'desired', field_type @ 'NOW'}",
                ws,
            )
            .unwrap();
        assert_eq!(rows.len(), 3); // id, name, price on Product

        // Query all methods across all owner types
        let methods = store
            .run_datalog(
                "?[owner_kind, owner, method_name] := \
                    *method{workspace: $ws, owner_kind, owner, name: method_name, state: 'desired' @ 'NOW'}",
                ws,
            )
            .unwrap();
        // Product: create, update_price; CatalogService: list_products; ProductRepository: find_by_id, save
        assert_eq!(methods.len(), 5);

        // Query method parameters
        let params = store
            .run_datalog(
                "?[owner, method, param_name, param_type] := \
                    *method_param{workspace: $ws, owner, method, name: param_name, \
                                  state: 'desired', param_type @ 'NOW'}",
                ws,
            )
            .unwrap();
        // create(name, price), update_price(new_price), find_by_id(id), save(product)
        assert_eq!(params.len(), 5);
    }

    #[test]
    fn test_upsert() {
        let store = temp_store();
        let ws = "/tmp/test-upsert";
        store.save_desired(ws, &test_model("First")).unwrap();
        store.save_desired(ws, &test_model("Second")).unwrap();
        let loaded = store.load_desired(ws).unwrap().unwrap();
        assert_eq!(loaded.name, "Second");
    }

    #[test]
    fn test_load_nonexistent() {
        let store = temp_store();
        assert!(store.load_desired("/tmp/nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_list_projects() {
        let store = temp_store();
        store
            .save_desired("/tmp/test-list-1", &test_model("P1"))
            .unwrap();
        store
            .save_desired("/tmp/test-list-2", &test_model("P2"))
            .unwrap();
        let projects = store.list().unwrap();
        assert_eq!(projects.len(), 2);
    }

    #[test]
    fn test_accept_and_load_actual() {
        let store = temp_store();
        let ws = "/tmp/test-accept";
        let model = full_model();
        store.save_desired(ws, &model).unwrap();
        store.accept(ws).unwrap();
        let actual = store.load_actual(ws).unwrap().unwrap();
        assert_eq!(actual.bounded_contexts.len(), 2);
    }

    #[test]
    fn test_reset() {
        let store = temp_store();
        let ws = "/tmp/test-reset";
        let model = full_model();
        store.save_desired(ws, &model).unwrap();
        store.accept(ws).unwrap();
        let mut modified = full_model();
        modified.bounded_contexts.push(BoundedContext {
            api_endpoints: vec![],
            name: "NewCtx".into(),
            description: "".into(),
            module_path: "".into(),
            ownership: Ownership::default(),
            aggregates: vec![],
            policies: vec![],
            read_models: vec![],
            entities: vec![],
            value_objects: vec![],
            services: vec![],
            repositories: vec![],
            events: vec![],
            modules: vec![],
            dependencies: vec![],
        });
        store.save_desired(ws, &modified).unwrap();
        let desired = store.load_desired(ws).unwrap().unwrap();
        assert_eq!(desired.bounded_contexts.len(), 3);
        let reset = store.reset(ws).unwrap().unwrap();
        assert_eq!(reset.bounded_contexts.len(), 3);
    }

    #[test]
    fn test_diff_graph_pure_datalog() {
        let store = temp_store();
        let ws = "/tmp/test-diff";
        let model = full_model();
        store.save_desired(ws, &model).unwrap();
        let diff = store.diff_graph(ws).unwrap();
        let changes = diff["pending_changes"].as_array().unwrap();
        assert!(changes.is_empty());

        let mut modified = full_model();
        modified.bounded_contexts.push(BoundedContext {
            api_endpoints: vec![],
            name: "Telemetry".into(),
            description: "Observed status context".into(),
            module_path: "src/telemetry".into(),
            ownership: Ownership::default(),
            aggregates: vec![],
            policies: vec![],
            read_models: vec![],
            entities: vec![],
            value_objects: vec![],
            services: vec![],
            repositories: vec![],
            events: vec![],
            modules: vec![],
            dependencies: vec![],
        });
        store.save_desired(ws, &modified).unwrap();
        let diff = store.diff_graph(ws).unwrap();
        let changes = diff["pending_changes"].as_array().unwrap();
        assert!(
            changes
                .iter()
                .any(|change| change["kind"] == "context" && change["name"] == "Telemetry")
        );
    }

    #[test]
    fn test_compute_drift() {
        let store = temp_store();
        let ws = "/tmp/test-drift";
        let model = full_model();
        store.save_desired(ws, &model).unwrap();

        store.compute_drift(ws).unwrap();

        let entries = store.load_drift(ws).unwrap();
        assert!(
            entries.is_empty(),
            "First actual snapshot has no prior drift basis"
        );

        let mut modified = full_model();
        modified.bounded_contexts.push(BoundedContext {
            api_endpoints: vec![],
            name: "Telemetry".into(),
            description: "Observed status context".into(),
            module_path: "src/telemetry".into(),
            ownership: Ownership::default(),
            aggregates: vec![],
            policies: vec![],
            read_models: vec![],
            entities: vec![],
            value_objects: vec![],
            services: vec![],
            repositories: vec![],
            events: vec![],
            modules: vec![],
            dependencies: vec![],
        });
        store.save_desired(ws, &modified).unwrap();
        store.compute_drift(ws).unwrap();

        let entries = store.load_drift(ws).unwrap();
        assert!(
            entries
                .iter()
                .any(|(category, _, name, change_type)| category == "context"
                    && name == "Telemetry"
                    && change_type == "add")
        );
    }

    #[test]
    fn test_truth_maintenance_report_tracks_freshness() {
        let store = temp_store();
        let ws = "/tmp/test-truth-maintenance";

        let empty = store.truth_maintenance_report(ws).unwrap();
        assert!(!empty.asserted.available);
        assert!(!empty.scanned.available);
        assert_eq!(empty.drift.status, "unavailable");

        store.save_desired(ws, &full_model()).unwrap();
        let desired_only = store.truth_maintenance_report(ws).unwrap();
        assert!(desired_only.asserted.available);
        assert!(desired_only.scanned.available);
        assert_eq!(desired_only.drift.status, "unavailable");

        store.save_actual(ws, &full_model()).unwrap();
        let before_drift = store.truth_maintenance_report(ws).unwrap();
        assert_eq!(before_drift.drift.status, "unavailable");
        assert!(!before_drift.drift.available);

        store.compute_drift(ws).unwrap();
        let fresh = store.truth_maintenance_report(ws).unwrap();
        assert_eq!(fresh.drift.status, "fresh");
        assert!(fresh.drift.available);
        assert!(fresh.drift.computed_at_us.is_some());
        assert!(fresh.assumptions.is_empty());

        std::thread::sleep(std::time::Duration::from_millis(2));
        let mut modified = full_model();
        modified.bounded_contexts.push(BoundedContext {
            api_endpoints: vec![],
            name: "LateChange".into(),
            description: "Introduced after drift computation".into(),
            module_path: "src/late_change".into(),
            ownership: Ownership::default(),
            aggregates: vec![],
            policies: vec![],
            read_models: vec![],
            entities: vec![],
            value_objects: vec![],
            services: vec![],
            repositories: vec![],
            events: vec![],
            modules: vec![],
            dependencies: vec![],
        });
        store.save_desired(ws, &modified).unwrap();

        let stale = store.truth_maintenance_report(ws).unwrap();
        assert_eq!(stale.drift.status, "stale");
        assert!(stale.drift.available);
        assert!(
            stale
                .assumptions
                .iter()
                .any(|assumption| assumption.contains("may be stale"))
        );
    }

    #[test]
    fn test_reasoning_claim_roundtrip_and_invalidation() {
        let store = temp_store();
        let ws = "/tmp/test-reasoning-claim-roundtrip";

        let claim = PersistedReasoningClaim {
            claim_id: "check.layer_violations".into(),
            claim_kind: "check".into(),
            subject: "layer_violations".into(),
            status: "true".into(),
            summary: "No layer violations detected.".into(),
            payload: json!({
                "invariant": "layer_violations",
                "status": "true",
                "count": 0,
            }),
            provenance: ReasoningProvenance {
                source: "datalog".into(),
                state: "actual".into(),
            },
            stale: false,
            computed_at_us: 42,
            derivations: vec![ReasoningDerivation {
                rule: "domain_service MUST_NOT depend_on infrastructure_service".into(),
                derived_from: vec!["service".into(), "service_dep".into()],
                witness_count: 0,
            }],
            assumptions: vec![ReasoningAssumption {
                assumption_kind: "limitation".into(),
                text: "Only stored implemented dependencies are considered.".into(),
            }],
            supports: vec![ReasoningSupportEdge {
                support_kind: "evidence".into(),
                summary: "No witnesses".into(),
                detail: json!([]),
            }],
            dependencies: vec![ReasoningDependency {
                dependency_kind: "snapshot".into(),
                dependency_state: "desired".into(),
                basis_timestamp_us: 7,
            }],
            justifications: vec![ReasoningJustification {
                fact_kind: "service".into(),
                fact_key: "*".into(),
                fact_state: "desired".into(),
                basis_timestamp_us: 7,
            }],
        };

        store.save_reasoning_claims(ws, &[claim.clone()]).unwrap();

        let loaded = store
            .load_reasoning_claim(ws, &claim.claim_id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.claim_kind, claim.claim_kind);
        assert_eq!(loaded.summary, claim.summary);
        assert!(!loaded.stale);
        assert_eq!(loaded.derivations.len(), 1);
        assert_eq!(loaded.supports.len(), 1);
        assert_eq!(loaded.assumption_texts().len(), 0);
        assert_eq!(loaded.limitation_texts().len(), 1);

        assert_eq!(
            store
                .invalidate_reasoning_claims_for_dependency(ws, "actual")
                .unwrap(),
            0
        );
        let still_fresh = store
            .load_reasoning_claim(ws, &claim.claim_id)
            .unwrap()
            .unwrap();
        assert!(!still_fresh.stale);

        assert_eq!(
            store
                .invalidate_reasoning_claims_for_dependency(ws, "desired")
                .unwrap(),
            1
        );
        let stale = store
            .load_reasoning_claim(ws, &claim.claim_id)
            .unwrap()
            .unwrap();
        assert!(stale.stale);
    }

    #[test]
    fn test_reasoning_fact_invalidation_is_precise() {
        let store = temp_store();
        let ws = "/tmp/test-reasoning-fact-invalidation";

        let entity_claim = PersistedReasoningClaim {
            claim_id: "claim.entity".into(),
            claim_kind: "check".into(),
            subject: "entity".into(),
            status: "true".into(),
            summary: "entity claim".into(),
            payload: json!({ "status": "true" }),
            provenance: ReasoningProvenance {
                source: "test".into(),
                state: "desired".into(),
            },
            stale: false,
            computed_at_us: 1,
            derivations: vec![],
            assumptions: vec![],
            supports: vec![],
            dependencies: vec![],
            justifications: vec![ReasoningJustification {
                fact_kind: "entity".into(),
                fact_key: "Sales/Order".into(),
                fact_state: "desired".into(),
                basis_timestamp_us: 1,
            }],
        };

        let service_claim = PersistedReasoningClaim {
            claim_id: "claim.service".into(),
            claim_kind: "check".into(),
            subject: "service".into(),
            status: "true".into(),
            summary: "service claim".into(),
            payload: json!({ "status": "true" }),
            provenance: ReasoningProvenance {
                source: "test".into(),
                state: "desired".into(),
            },
            stale: false,
            computed_at_us: 1,
            derivations: vec![],
            assumptions: vec![],
            supports: vec![],
            dependencies: vec![],
            justifications: vec![ReasoningJustification {
                fact_kind: "service".into(),
                fact_key: "Sales/BillingService".into(),
                fact_state: "desired".into(),
                basis_timestamp_us: 1,
            }],
        };

        store
            .save_reasoning_claims(ws, &[entity_claim, service_claim])
            .unwrap();

        let invalidated = store
            .invalidate_reasoning_claims_for_facts(
                ws,
                &[ReasoningFactRef {
                    fact_kind: "entity".into(),
                    fact_key: "Sales/Order".into(),
                    fact_state: "desired".into(),
                }],
            )
            .unwrap();
        assert_eq!(invalidated, 1);

        let entity_claim = store
            .load_reasoning_claim(ws, "claim.entity")
            .unwrap()
            .unwrap();
        let service_claim = store
            .load_reasoning_claim(ws, "claim.service")
            .unwrap()
            .unwrap();
        assert!(entity_claim.stale);
        assert!(!service_claim.stale);
    }

    #[test]
    fn test_list_snapshots() {
        let store = temp_store();
        let ws = "/tmp/test-snapshots";

        // No data → no snapshots
        let snaps = store.list_snapshots(ws, "desired").unwrap();
        assert!(snaps.is_empty(), "No snapshots before any data");

        // Save desired → at least one snapshot
        store.save_desired(ws, &full_model()).unwrap();
        let snaps = store.list_snapshots(ws, "desired").unwrap();
        assert!(!snaps.is_empty(), "Must have snapshot after save");
        assert!(snaps[0] > 0, "Snapshot timestamp must be positive");

        // Save again → may have 1 or 2 timestamps (depending on timing)
        std::thread::sleep(std::time::Duration::from_millis(2));
        let mut model2 = full_model();
        model2.bounded_contexts.push(BoundedContext {
            api_endpoints: vec![],
            name: "Extra".into(),
            description: "".into(),
            module_path: "".into(),
            ownership: Ownership::default(),
            aggregates: vec![],
            policies: vec![],
            read_models: vec![],
            entities: vec![],
            value_objects: vec![],
            services: vec![],
            repositories: vec![],
            events: vec![],
            modules: vec![],
            dependencies: vec![],
        });
        store.save_desired(ws, &model2).unwrap();
        let snaps2 = store.list_snapshots(ws, "desired").unwrap();
        assert!(
            snaps2.len() >= 2,
            "Must have multiple snapshots: got {}",
            snaps2.len()
        );
        assert!(snaps2[0] >= snaps2[1], "Snapshots must be descending");
    }

    #[test]
    fn test_diff_snapshots() {
        let store = temp_store();
        let ws = "/tmp/test-diff-snap";

        // Save initial model
        store.save_desired(ws, &full_model()).unwrap();
        let snaps1 = store.list_snapshots(ws, "desired").unwrap();
        let ts1 = snaps1[0];

        // Save modified model after brief pause
        std::thread::sleep(std::time::Duration::from_millis(2));
        let mut model2 = full_model();
        model2.bounded_contexts.push(BoundedContext {
            api_endpoints: vec![],
            name: "NewCtx".into(),
            description: "Added later".into(),
            module_path: "".into(),
            ownership: Ownership::default(),
            aggregates: vec![],
            policies: vec![],
            read_models: vec![],
            entities: vec![],
            value_objects: vec![],
            services: vec![],
            repositories: vec![],
            events: vec![],
            modules: vec![],
            dependencies: vec![],
        });
        store.save_desired(ws, &model2).unwrap();
        let snaps2 = store.list_snapshots(ws, "desired").unwrap();
        let ts2 = snaps2[0];

        // Diff between the two snapshots
        let diff = store.diff_snapshots(ws, "desired", ts1, ts2).unwrap();
        let added = diff["added"].as_array().unwrap();
        assert!(
            added.iter().any(|e| e["name"] == "NewCtx"),
            "NewCtx must appear as added: {:?}",
            diff
        );
        assert_eq!(diff["summary"]["removals"].as_i64().unwrap(), 0);
    }

    #[test]
    fn test_transitive_deps() {
        let store = temp_store();
        let ws = "/tmp/test-trans";
        let model = full_model();
        store.save_desired(ws, &model).unwrap();
        let deps = store
            .transitive_deps(&canonicalize_path(ws), "Billing")
            .unwrap();
        assert!(deps.contains(&"Identity".to_string()));
    }

    #[test]
    fn test_circular_deps() {
        let store = temp_store();
        let ws = "/tmp/test-circular";
        let mut model = full_model();
        if let Some(identity) = model
            .bounded_contexts
            .iter_mut()
            .find(|c| c.name == "Identity")
        {
            identity.dependencies.push("Billing".into());
        }
        store.save_desired(ws, &model).unwrap();
        let cycles = store.circular_deps(&canonicalize_path(ws)).unwrap();
        assert!(!cycles.is_empty());
    }

    #[test]
    fn test_no_circular_deps() {
        let store = temp_store();
        let ws = "/tmp/test-no-circ";
        store.save_desired(ws, &full_model()).unwrap();
        let cycles = store.circular_deps(&canonicalize_path(ws)).unwrap();
        assert!(cycles.is_empty());
    }

    #[test]
    fn test_aggregate_roots_without_invariants() {
        let store = temp_store();
        let ws = "/tmp/test-agg";
        let model = full_model();
        store.save_desired(ws, &model).unwrap();
        let missing = store
            .aggregate_roots_without_invariants(&canonicalize_path(ws))
            .unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn test_impact_analysis() {
        let store = temp_store();
        let ws = "/tmp/test-impact";
        store.save_desired(ws, &full_model()).unwrap();
        let canonical = canonicalize_path(ws);
        let result = store
            .impact_analysis(&canonical, "Identity", "User")
            .unwrap();
        assert!(result.get("entity").is_some());
    }

    #[test]
    fn test_dependency_graph() {
        let store = temp_store();
        let ws = "/tmp/test-depgraph";
        store.save_desired(ws, &full_model()).unwrap();
        let canonical = canonicalize_path(ws);
        let graph = store.dependency_graph(&canonical).unwrap();
        let nodes = graph["nodes"].as_array().unwrap();
        let edges = graph["edges"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["from"], "Billing");
        assert_eq!(edges[0]["to"], "Identity");
    }

    #[test]
    fn test_raw_datalog_query() {
        let store = temp_store();
        let model = full_model();
        store.save_desired("/tmp/test-raw", &model).unwrap();
        let rows = store
            .run_datalog(
                "?[name, aggregate_root] := *entity{workspace: $ws, name, aggregate_root, state: 'desired' @ 'NOW'}",
                "/tmp/test-raw",
            )
            .unwrap();
        assert_eq!(rows.len(), 2);
    }
}
