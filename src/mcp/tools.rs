use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::process::Command;

use crate::domain::model::DomainModel;
use crate::mcp::protocol::{
    ToolCallResult, ToolDefinition, error_tool_result, json_tool_result, with_reasoning_context,
    with_workspace_context_schema,
};
use crate::reasoning::ReasoningKernel;
use crate::store::{PersistedReasoningClaim, Store};

/// Returns the list of tools the Axon server exposes.
pub fn list_tools() -> Vec<ToolDefinition> {
    rust_native_read_tools()
}

fn rust_native_read_tools() -> Vec<ToolDefinition> {
    read_tool_specs().into_iter().map(Into::into).collect()
}

struct ReadToolSpec {
    name: String,
    description: String,
    input_schema: Value,
}

impl From<ReadToolSpec> for ToolDefinition {
    fn from(spec: ReadToolSpec) -> Self {
        Self {
            name: spec.name,
            description: spec.description,
            input_schema: with_workspace_context_schema(spec.input_schema),
        }
    }
}

#[derive(Default, Deserialize)]
struct RustStatusArgs {
    detail: Option<RustStatusDetail>,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RustStatusDetail {
    Summary,
    Full,
}

impl RustStatusArgs {
    fn is_full(&self) -> bool {
        self.detail == Some(RustStatusDetail::Full)
    }
}

fn read_tool_specs() -> Vec<ReadToolSpec> {
    vec![
        ReadToolSpec {
            name: "rust_status".into(),
            description: "Show the current actual-state Rust model: crate inventory, module tree, source files, Rust symbols, imports, calls, semantic annotations, health, graph confidence, and snapshot freshness. Call this first before planning Rust changes.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "detail": {
                        "type": "string",
                        "enum": ["summary", "full"],
                        "description": "Response detail level. Summary is bounded for chat; full returns the complete architecture payload."
                    }
                },
                "required": []
            }),
        },
        ReadToolSpec {
            name: "rust_graph".into(),
            description: "Query persisted actual-state Rust facts through bounded graph views: modules, source files, symbols, import edges, reference edges, call edges, compiler-resolved call edges, AST edges, neighborhoods, paths, and relation counts. Reference edges (`relation=reference_edge`) capture code paths from types, trait bounds, qualified expressions, and macros. AST edges (`relation=ast_edge`) carry `extends`/`implements` plus compiler directives as `decorators` edges; `to_node` is the directive text and `from_node` is the annotated item, each with a file/line. Use `view=paths` with `relation=context_dep` or `relation=calls_symbol` for connectivity. Results use compact schema-plus-rows JSON (`schema`, `cols`, `rows`). Arbitrary Datalog is not exposed.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "view": {
                        "type": "string",
                        "enum": ["overview", "relations", "nodes", "edges", "neighborhood", "paths"],
                        "description": "Graph view to return (default: overview)"
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["all", "module", "source_file", "symbol", "struct", "enum", "function", "method"],
                        "description": "Rust node kind filter for nodes/neighborhood views"
                    },
                    "relation": {
                        "type": "string",
                        "enum": ["all", "context_dep", "import_edge", "reference_edge", "calls_symbol", "ast_edge", "resolved_call"],
                        "description": "Rust relation filter for edges/paths views"
                    },
                    "module": { "type": "string", "description": "Rust module path/name filter" },
                    "file": { "type": "string", "description": "Source file filter" },
                    "symbol": { "type": "string", "description": "Rust symbol filter" },
                    "struct": { "type": "string", "description": "Rust struct-name alias for symbol" },
                    "from": { "type": "string", "description": "Source node for paths or edge filtering" },
                    "to": { "type": "string", "description": "Target node for paths or edge filtering" },
                    "scope": {
                        "type": "string",
                        "enum": ["production", "test", "all"],
                        "description": "Filter row-returning graph views by fact scope (default: all)."
                    },
                    "limit": { "type": "integer", "description": "Max returned rows per collection (default: 50, max: 200)", "default": 50 },
                    "offset": { "type": "integer", "description": "Zero-based row offset for paginating nodes, edges, and neighborhoods (default: 0)", "default": 0 }
                },
                "required": []
            }),
        },
        ReadToolSpec {
            name: "rust_readiness".into(),
            description: "Report Axon's product readiness for this Rust workspace: graph confidence, semantic call-resolution coverage, embedded rust-analyzer mode, Cargo/Rust toolchain visibility, version/runtime identity, and concrete remediation actions.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ReadToolSpec {
            name: "rust_impact".into(),
            description: "Analyze blast radius, dependency shape, call graph usage, centrality, refactor recommendations, and Rust practice findings in the actual Rust graph. Use module for dependency analysis and symbol or struct for call/entity analyses.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "analysis": {
                        "type": "string",
                        "enum": ["transitive_deps", "circular_deps", "layer_violations", "impact_analysis",
                                 "aggregate_quality", "dependency_graph", "field_usage", "method_search",
                                 "shared_fields", "pagerank", "community_detection", "betweenness_centrality",
                                 "degree_centrality", "topological_order",
                                 "call_graph_callers", "call_graph_callees", "call_graph_reachability", "call_graph_stats",
                                 "optimization_recommendations", "practice_findings"],
                        "description": "The specific actual-state Rust analysis to run"
                    },
                    "module": { "type": "string", "description": "Rust module name/path for dependency analyses" },
                    "struct": { "type": "string", "description": "Rust struct name for struct/entity or call graph analyses" },
                    "symbol": { "type": "string", "description": "Rust symbol name" },
                    "field_type": { "type": "string", "description": "Field type to search (required for field_usage)" },
                    "method_name": { "type": "string", "description": "Method name to search (required for method_search)" },
                    "scope": {
                        "type": "string",
                        "enum": ["production", "test", "all"],
                        "description": "Fact scope for advice analyses. practice_findings and optimization_recommendations default to production; pass all to include tests."
                    }
                },
                "required": ["analysis"]
            }),
        },
        ReadToolSpec {
            name: "rust_delete_safety".into(),
            description: "Check whether a Rust symbol or struct can be safely deleted. Evaluates inbound call edges, imports, AST references, type references, and semantic annotation dependents. Module is optional; omit it for a workspace-wide symbol check.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "module": { "type": "string", "description": "Optional Rust module filter" },
                    "struct": { "type": "string", "description": "Rust struct name" },
                    "symbol": { "type": "string", "description": "Rust symbol name" }
                },
                "required": []
            }),
        },
        ReadToolSpec {
            name: "rust_invariants".into(),
            description: "Check actual Rust graph invariants and configured constraints: circular dependencies, layer violations, missing invariants on annotated core structs, isolated modules/contexts, policy violations, and drift freshness. Run without parameters to check everything.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "invariant": {
                        "type": "string",
                        "enum": ["layer_violations", "circular_deps", "aggregate_quality", "orphan_contexts", "policy_violations", "drift"],
                        "description": "Specific invariant to run (default: all)"
                    }
                },
                "required": []
            }),
        },
        ReadToolSpec {
            name: "rust_explain".into(),
            description: "Explain why a Rust graph invariant or configured constraint is failing. Returns evidence-backed witness paths and remediation context.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "violation_type": {
                        "type": "string",
                        "enum": ["layer_violations", "circular_deps", "policy_violations", "aggregate_quality", "orphan_contexts"],
                        "description": "The failing invariant or constraint type to explain"
                    }
                },
                "required": ["violation_type"]
            }),
        },
        ReadToolSpec {
            name: "rust_history".into(),
            description: "List actual Rust graph snapshots, compare two snapshot timestamps, or return the latest graph diff with mode=latest_diff. Use latest_diff as the replacement for the removed rust_diff tool.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["list", "compare", "latest_diff"],
                        "description": "History operation (default: list). Use latest_diff to compare the two most recent actual snapshots."
                    },
                    "state": {
                        "type": "string",
                        "enum": ["actual", "implemented", "current", "planned"],
                        "description": "History stream to query (default: actual; implemented/current/planned are accepted aliases)"
                    },
                    "ts_old": { "type": "integer", "description": "Older snapshot timestamp (microseconds). Required for comparison." },
                    "ts_new": { "type": "integer", "description": "Newer snapshot timestamp (microseconds). Omit for latest." }
                },
                "required": []
            }),
        },
        ReadToolSpec {
            name: "rust_search".into(),
            description: "Search actual Rust facts and semantic annotations by keyword. Finds modules, structs, symbols, labels, policies, constraints, and decisions across the codebase.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search keywords" },
                    "limit": { "type": "integer", "description": "Max results (default: 20)", "default": 20 }
                },
                "required": ["query"]
            }),
        },
    ]
}

pub fn call_tool(store: &Store, workspace_path: &str, name: &str, args: &Value) -> ToolCallResult {
    match name {
        "rust_status" => {
            let status_args = match parse_tool_args::<RustStatusArgs>(args) {
                Ok(args) => args,
                Err(e) => return error_result(format!("rust_status invalid arguments: {e}")),
            };
            let kernel = ReasoningKernel::new(store);
            match kernel.architecture(workspace_path) {
                Ok(mut claim) => {
                    if !status_args.is_full() {
                        claim.payload = compact_architecture_status_payload(claim.payload);
                    }
                    attach_graph_confidence(&mut claim.payload, store, workspace_path);
                    stored_claim_result(store, workspace_path, &claim)
                }
                Err(e) => error_result(format!("rust_status failed: {e}")),
            }
        }

        "rust_graph" => match store.query_rust_graph(workspace_path, args) {
            Ok(payload) => {
                let payload = compact_rust_graph_payload(payload);
                let witness_count = graph_witness_count(&payload);
                let relations_used = payload["relations_used"].clone();
                let mut envelope = with_reasoning_context(
                    payload,
                    Some(json!({
                        "rule": "bounded Rust graph query over persisted Cozo relations",
                        "derived_from": relations_used,
                        "witness_count": witness_count,
                    })),
                    Some(json!({
                        "filters": args,
                        "witness_count": witness_count,
                    })),
                    vec![
                        "Graph queries return persisted actual-state facts only; run rust_scan before relying on freshly edited files.".into(),
                        "Output is bounded and may be truncated; increase limit up to 200 or narrow filters for exhaustive local evidence.".into(),
                        "Fact resolution: `import_edge` (use-paths) and `calls_symbol` (caller/callee names) are syntactic/name-based from the syn scanner and may be ambiguous across same-named symbols; `resolved_call` is compiler-resolved and refreshed by rust_scan when embedded rust-analyzer can load the Cargo workspace.".into(),
                    ],
                    Some(json!({"source": "rust_graph_query", "state": "actual"})),
                );
                if let Some(object) = envelope.as_object_mut() {
                    let confidence = build_graph_confidence_report(store, workspace_path);
                    object.insert(
                        "graph_confidence".into(),
                        confidence["graph_confidence"].clone(),
                    );
                    object.insert("readiness_summary".into(), confidence["summary"].clone());
                    object.insert(
                        "truth_maintenance".into(),
                        compact_truth_maintenance_json(truth_maintenance_json(
                            store,
                            workspace_path,
                        )),
                    );
                }
                json_result(envelope)
            }
            Err(e) => error_result(format!("query_rust_graph failed: {e}")),
        },

        "rust_readiness" => json_result(build_readiness_report(store, workspace_path)),

        "rust_impact" => {
            let kernel = ReasoningKernel::new(store);
            match kernel.impact(workspace_path, args) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!("rust_impact failed: {e}")),
            }
        }

        "rust_delete_safety" => {
            let context = args["context"]
                .as_str()
                .or_else(|| args["module"].as_str())
                .unwrap_or("");
            let entity = match args["entity"]
                .as_str()
                .or_else(|| args["struct"].as_str())
                .or_else(|| args["symbol"].as_str())
            {
                Some(e) => e,
                None => {
                    return error_result(
                        "'entity', 'struct', or 'symbol' parameter is required".into(),
                    );
                }
            };
            let kernel = ReasoningKernel::new(store);
            match kernel.safe_to_delete(workspace_path, context, entity) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!("rust_delete_safety failed: {e}")),
            }
        }

        "rust_invariants" => {
            let invariant = args["check_name"]
                .as_str()
                .or_else(|| args["invariant"].as_str())
                .unwrap_or("all");
            let kernel = ReasoningKernel::new(store);
            match kernel.check(workspace_path, invariant) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!(
                    "check '{}' failed: {e}",
                    if invariant.is_empty() {
                        "all"
                    } else {
                        invariant
                    }
                )),
            }
        }

        "rust_explain" => {
            let violation_type = match args["violation_type"].as_str() {
                Some(v) => v,
                None => return error_result("'violation_type' parameter is required".into()),
            };

            let kernel = ReasoningKernel::new(store);
            match kernel.explain(workspace_path, violation_type) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!("why '{}' failed: {e}", violation_type)),
            }
        }

        "rust_history" => {
            let kernel = ReasoningKernel::new(store);
            match kernel.history(workspace_path, args) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!("history failed: {e}")),
            }
        }

        "rust_search" => {
            let query = match args["query"].as_str() {
                Some(q) => q,
                None => return error_result("'query' parameter is required".into()),
            };
            let limit = u64_to_usize_saturating(args["limit"].as_u64().unwrap_or(20));
            let kernel = ReasoningKernel::new(store);
            match kernel.search(workspace_path, query, limit) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!("rust_search failed: {e}")),
            }
        }

        _ => error_result(format!("Unknown tool: {}", name)),
    }
}

fn parse_tool_args<T>(args: &Value) -> Result<T, serde_json::Error>
where
    T: DeserializeOwned,
{
    serde_json::from_value(args.clone())
}

fn error_result(msg: String) -> ToolCallResult {
    error_tool_result(msg)
}

fn json_result(payload: Value) -> ToolCallResult {
    json_tool_result(payload)
}

pub(crate) fn attach_graph_confidence(payload: &mut Value, store: &Store, workspace_path: &str) {
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    let confidence = build_graph_confidence_report(store, workspace_path);
    object.insert(
        "graph_confidence".into(),
        confidence["graph_confidence"].clone(),
    );
    object.insert("readiness_summary".into(), confidence["summary"].clone());
}

pub(crate) fn build_graph_confidence_report(store: &Store, workspace_path: &str) -> Value {
    let relation_counts_result = store.rust_graph_relation_counts(workspace_path);
    let relation_counts = relation_counts_result.as_ref().ok();
    let count = |name: &str| {
        relation_counts
            .and_then(|counts| counts.get(name).copied())
            .unwrap_or(0)
    };

    let source_files = count("source_file");
    let symbols = count("symbol");
    let import_edges = count("import_edge");
    let reference_edges = count("reference_edge");
    let call_edges = count("calls_symbol");
    let resolved_call_edges = count("resolved_call");
    let ast_edges = count("ast_edge");
    let cfg_decorator_edges = cfg_decorator_edge_count(store, workspace_path);
    let build_script_present = std::path::Path::new(workspace_path)
        .join("build.rs")
        .is_file();
    let truth = truth_maintenance_json(store, workspace_path);
    let drift_status = truth["drift"]["status"].as_str().unwrap_or("unknown");
    let scan_available = truth["scanned"]["available"].as_bool().unwrap_or(false)
        || truth["implemented"]["available"].as_bool().unwrap_or(false);

    let mut score = if source_files == 0 || symbols == 0 {
        0_i64
    } else {
        100_i64
    };
    let mut warnings = Vec::new();
    let mut next_actions = Vec::new();

    if relation_counts_result.is_err() {
        score = score.min(40);
        warnings.push("Persisted Rust relation counts could not be loaded.".to_string());
        next_actions.push(
            "Run rust_scan and inspect store errors if relation counts still fail.".to_string(),
        );
    }

    if !scan_available || source_files == 0 || symbols == 0 {
        score = 0;
        warnings.push("No persisted Rust scan is available for this workspace.".to_string());
        next_actions.push(
            "Run rust_scan before relying on architecture, impact, or deletion answers."
                .to_string(),
        );
    }

    if call_edges > 0 && resolved_call_edges == 0 {
        score -= 25;
        warnings.push("Call graph evidence is syntactic/name-based only; compiler-resolved calls are unavailable.".to_string());
        next_actions.push(
            "Run rust_readiness, ensure cargo/rustc are visible to the daemon, then run rust_scan."
                .to_string(),
        );
    }

    if source_files > 1 && symbols > 10 && call_edges == 0 {
        score -= 15;
        warnings.push(
            "No syntactic call edges are persisted despite a non-trivial symbol graph.".to_string(),
        );
        next_actions.push(
            "Run rust_scan and check scanner parse failures if call edges remain empty."
                .to_string(),
        );
    }

    if drift_status != "fresh" {
        score -= 20;
        warnings.push(format!(
            "Truth-maintenance drift status is '{drift_status}', not 'fresh'."
        ));
        next_actions.push("Run rust_scan to refresh actual facts and recompute drift.".to_string());
    }

    if source_files > 1 && reference_edges == 0 {
        score -= 5;
        warnings.push(
            "Reference edges are empty; type/trait/macro reference evidence may be incomplete."
                .to_string(),
        );
    }

    let score = usize::try_from(score.clamp(0, 100)).unwrap_or(100);
    let status = if source_files == 0 || symbols == 0 {
        "not_scanned"
    } else if score >= 90 {
        "ready"
    } else if score >= 70 {
        "usable_with_warnings"
    } else {
        "degraded"
    };

    json!({
        "summary": {
            "status": status,
            "score": score,
            "warning_count": warnings.len(),
            "next_action_count": next_actions.len(),
        },
        "graph_confidence": {
            "status": status,
            "score": score,
            "warnings": warnings,
            "next_actions": next_actions,
            "counts": {
                "source_files": source_files,
                "symbols": symbols,
                "import_edges": import_edges,
                "reference_edges": reference_edges,
                "call_edges": call_edges,
                "resolved_call_edges": resolved_call_edges,
                "ast_edges": ast_edges,
            },
            "capabilities": {
                "static_rust_scan": {
                    "status": if source_files > 0 && symbols > 0 { "ready" } else { "missing" },
                    "source_files": source_files,
                    "symbols": symbols,
                },
                "syntactic_call_graph": {
                    "status": if call_edges > 0 { "ready" } else { "empty" },
                    "relation": "calls_symbol",
                    "edges": call_edges,
                    "precision": "name-based; ambiguous across same-named symbols",
                },
                "semantic_call_graph": {
                    "status": if resolved_call_edges > 0 { "ready" } else if call_edges > 0 { "missing" } else { "not_applicable" },
                    "relation": "resolved_call",
                    "edges": resolved_call_edges,
                    "precision": "compiler-resolved through embedded rust-analyzer libraries when Cargo workspace loading succeeds",
                    "persisted_fields": ["caller", "caller_file", "caller_line", "callee", "callee_file", "callee_line", "call_site_line", "call_expr", "dispatch_kind"],
                    "when_missing": {
                        "fallback": "Axon keeps source files, symbols, imports, references, AST decorators, and syntactic calls_symbol edges.",
                        "capability_lost": ["exact receiver/trait/alias/inference-sensitive call targets", "resolved call-site provenance", "compiler-resolved move/facade evidence"],
                        "analyses_degraded": ["call_graph_stats resolved_project_callee sections", "call_graph_callers/callees when relying on aliases", "optimization_recommendations move_or_facade precision", "delete-safety and impact reviews that need exact inbound calls"],
                    },
                },
                "truth_maintenance": {
                    "status": drift_status,
                    "drift": truth["drift"],
                },
                "cfg_feature_awareness": {
                    "status": "partial",
                    "cfg_decorator_edges": cfg_decorator_edges,
                    "persisted_actual_profile": "single_profile",
                    "transient_profile_comparison": "rust_feature_diff compares two scan profiles without persisting either as actual state",
                    "persisted_profile_slices": false,
                    "details": "cfg predicates are captured as AST decorator edges when present. Cargo feature metadata is visible and rust_feature_diff can compare transient profiles; the persisted actual graph remains one selected scan profile.",
                    "analyses_degraded_without_persisted_slices": ["historical per-feature impact", "feature-specific delete safety", "feature-specific dependency neighborhoods from stored actual state"],
                },
                "generated_code": {
                    "status": if build_script_present { "detected_unmodeled" } else { "not_modeled" },
                    "build_script_present": build_script_present,
                    "build_script_out_dirs": "disabled",
                    "proc_macro_expansion": "disabled",
                    "details": "The semantic resolver intentionally does not run build scripts or proc-macro expansion during daemon scans. Materialized Rust files are scanned; generated out-dir files and macro-expanded items are not persisted.",
                    "analyses_degraded": ["symbols/imports/calls that only exist after build-script generation", "proc-macro-generated methods and trait impls", "call edges inside generated files"],
                }
            }
        }
    })
}

fn cfg_decorator_edge_count(store: &Store, workspace_path: &str) -> usize {
    store
        .run_datalog(
            "?[to_node] := *ast_edge{workspace: $ws, to_node, edge_type: 'decorators', state: 'actual' @ 'NOW'}",
            workspace_path,
        )
        .map(|rows| {
            rows.iter()
                .filter(|row| row.first().is_some_and(|text| text.starts_with("cfg(")))
                .count()
        })
        .unwrap_or(0)
}

fn build_readiness_report(store: &Store, workspace_path: &str) -> Value {
    let graph = build_graph_confidence_report(store, workspace_path);
    let rust_analyzer = rust_analyzer_readiness();
    let rust_toolchain = rust_toolchain_readiness();
    let cargo_metadata = cargo_metadata_readiness(workspace_path);
    let graph_status = graph["summary"]["status"].as_str().unwrap_or("unknown");
    let rust_toolchain_status = rust_toolchain["status"].as_str().unwrap_or("unknown");
    let cargo_metadata_status = cargo_metadata["status"].as_str().unwrap_or("unknown");
    let semantic_calls_required = graph["graph_confidence"]["counts"]["call_edges"]
        .as_u64()
        .unwrap_or(0)
        > 0
        && graph["graph_confidence"]["counts"]["resolved_call_edges"]
            .as_u64()
            .unwrap_or(0)
            == 0;
    let semantic_dependencies_ready =
        rust_toolchain_status == "ready" && cargo_metadata_status == "ready";
    let status =
        if graph_status == "ready" && (!semantic_calls_required || semantic_dependencies_ready) {
            "ready"
        } else if graph_status == "not_scanned" {
            "not_scanned"
        } else if graph_status == "degraded"
            || (semantic_calls_required && !semantic_dependencies_ready)
        {
            "degraded"
        } else {
            "usable_with_warnings"
        };

    json!({
        "status": status,
        "workspace": workspace_path,
        "runtime": {
            "axon_version": crate::VERSION,
            "process_binary": std::env::current_exe()
                .ok()
                .map(|path| path.to_string_lossy().to_string()),
        },
        "graph_confidence": graph["graph_confidence"],
        "environment": {
            "rust_analyzer": rust_analyzer,
            "rust_toolchain": rust_toolchain,
            "cargo_metadata": cargo_metadata,
        },
        "refactor_loop": {
            "recommended_sequence": [
                "rust_scan",
                "rust_readiness",
                "rust_impact or rust_delete_safety for the target",
                "apply edits with rust-analyzer-aware tooling where possible",
                "cargo check or targeted cargo test",
                "rust_scan",
                "rust_history with mode=latest_diff and rust_diagnose"
            ]
        }
    })
}

fn rust_analyzer_readiness() -> Value {
    json!({
        "status": "ready",
        "mode": "embedded_library",
        "external_binary_required": false,
        "crates": [
            "ra_ap_ide",
            "ra_ap_load-cargo",
            "ra_ap_project_model",
            "ra_ap_vfs",
        ],
        "details": "Axon links rust-analyzer libraries in-process; semantic resolution depends on Cargo/Rust toolchain visibility, not the rust-analyzer executable.",
        "fix": Value::Null,
    })
}

fn rust_toolchain_readiness() -> Value {
    let cargo = command_version("cargo");
    let rustc = command_version("rustc");
    let cargo_available = cargo["available"].as_bool().unwrap_or(false);
    let rustc_available = rustc["available"].as_bool().unwrap_or(false);
    let ready = cargo_available && rustc_available;

    json!({
        "status": if ready { "ready" } else { "missing" },
        "cargo": cargo,
        "rustc": rustc,
        "required_by": "embedded rust-analyzer workspace loading",
        "fix": if ready {
            Value::Null
        } else {
            json!("Ensure the daemon PATH can execute cargo and rustc; Homebrew services should include /opt/homebrew/bin.")
        },
    })
}

fn command_version(command: &str) -> Value {
    match Command::new(command).arg("--version").output() {
        Ok(output) if output.status.success() => json!({
            "available": true,
            "version": output_text(&output.stdout),
            "error": Value::Null,
        }),
        Ok(output) => json!({
            "available": false,
            "version": Value::Null,
            "error": output_text(&output.stderr),
        }),
        Err(error) => json!({
            "available": false,
            "version": Value::Null,
            "error": error.to_string(),
        }),
    }
}

fn cargo_metadata_readiness(workspace_path: &str) -> Value {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(workspace_path)
        .output();
    let Ok(output) = output else {
        return json!({
            "status": "unavailable",
            "error": output.err().map(|error| error.to_string()),
            "details": "cargo metadata could not be executed for this workspace.",
        });
    };
    if !output.status.success() {
        return json!({
            "status": "failed",
            "error": output_text(&output.stderr),
            "details": "cargo metadata failed; feature and workspace-package visibility is incomplete.",
        });
    }
    let metadata = serde_json::from_slice::<Value>(&output.stdout).unwrap_or_else(|_| json!({}));
    let package_count = metadata["packages"]
        .as_array()
        .map(|packages| packages.len())
        .unwrap_or(0);
    let workspace_member_count = metadata["workspace_members"]
        .as_array()
        .map(|members| members.len())
        .unwrap_or(0);
    let feature_count = metadata["packages"]
        .as_array()
        .map(|packages| {
            packages
                .iter()
                .map(|package| {
                    package["features"]
                        .as_object()
                        .map(|features| features.len())
                        .unwrap_or(0)
                })
                .sum::<usize>()
        })
        .unwrap_or(0);

    json!({
        "status": "ready",
        "package_count": package_count,
        "workspace_member_count": workspace_member_count,
        "feature_count": feature_count,
        "target_directory": metadata["target_directory"],
        "details": "Cargo package and feature metadata is visible. rust_feature_diff can compare transient scan profiles; the persisted actual graph stores one selected profile at a time.",
        "persisted_profile_slices": false,
        "transient_profile_comparison": "rust_feature_diff",
    })
}

fn output_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

fn u64_to_usize_saturating(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn graph_witness_count(payload: &Value) -> usize {
    u64_to_usize_saturating(
        payload["count"]
            .as_u64()
            .or_else(|| payload["rows"].as_array().map(|rows| rows.len() as u64))
            .or_else(|| payload["summary"]["node_count"].as_u64())
            .or_else(|| payload["summary"]["edge_count"].as_u64())
            .or_else(|| payload["summary"]["relation_count"].as_u64())
            .unwrap_or(0),
    )
}

fn compact_rust_graph_payload(mut payload: Value) -> Value {
    let Some(object) = payload.as_object_mut() else {
        return payload;
    };
    let view = object
        .get("view")
        .and_then(Value::as_str)
        .unwrap_or("overview")
        .to_string();
    object.remove("workspace");
    object.remove("graph_schema");
    object.insert("schema".into(), json!(format!("axon.rust_graph.{view}.v1")));
    object.insert("format".into(), json!("schema_rows"));

    match view.as_str() {
        "overview" | "relations" => {
            if let Some(counts) = object.remove("relation_counts") {
                object.insert("cols".into(), json!(["rel", "count"]));
                object.insert("rows".into(), relation_count_rows(&counts));
            }
            if let Some(rows) = object.get("rows").and_then(Value::as_array) {
                object.insert("count".into(), json!(rows.len()));
            }
        }
        "nodes" => {
            let rows = object
                .remove("nodes")
                .and_then(|nodes| nodes.as_array().map(|nodes| compact_node_rows(nodes)))
                .unwrap_or_default();
            object.insert("cols".into(), rust_graph_node_cols());
            object.insert("rows".into(), Value::Array(rows));
        }
        "edges" => {
            let rows = object
                .remove("edges")
                .and_then(|edges| edges.as_array().map(|edges| compact_edge_rows(edges)))
                .unwrap_or_default();
            object.insert("cols".into(), rust_graph_edge_cols());
            object.insert("rows".into(), Value::Array(rows));
        }
        "neighborhood" => {
            let node_rows = object
                .remove("nodes")
                .and_then(|nodes| nodes.as_array().map(|nodes| compact_node_rows(nodes)))
                .unwrap_or_default();
            let edge_rows = object
                .remove("edges")
                .and_then(|edges| edges.as_array().map(|edges| compact_edge_rows(edges)))
                .unwrap_or_default();
            object.insert(
                "tables".into(),
                json!({
                    "nodes": {
                        "cols": rust_graph_node_cols(),
                        "rows": node_rows,
                    },
                    "edges": {
                        "cols": rust_graph_edge_cols(),
                        "rows": edge_rows,
                    }
                }),
            );
        }
        "paths" => {
            if let Some(paths) = object.remove("paths") {
                let rows = paths
                    .as_array()
                    .map(|paths| paths.iter().map(|path| json!([path])).collect::<Vec<_>>())
                    .unwrap_or_default();
                object.insert("cols".into(), json!(["path"]));
                object.insert("rows".into(), Value::Array(rows));
            } else if let Some(result) = object.remove("result") {
                let rows = result["reachable"]
                    .as_array()
                    .map(|items| items.iter().map(|item| json!([item])).collect::<Vec<_>>())
                    .unwrap_or_default();
                object.insert("cols".into(), json!(["callee"]));
                object.insert("rows".into(), Value::Array(rows));
            }
        }
        _ => {}
    }

    payload
}

fn relation_count_rows(counts: &Value) -> Value {
    let rows = counts
        .as_object()
        .map(|counts| {
            counts
                .iter()
                .map(|(relation, count)| json!([relation, count]))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Value::Array(rows)
}

fn rust_graph_node_cols() -> Value {
    json!([
        "id", "kind", "name", "ctx", "path", "file", "start", "end", "vis", "pub", "lang", "desc",
        "rel"
    ])
}

fn rust_graph_edge_cols() -> Value {
    json!([
        "id",
        "rel",
        "from",
        "to",
        "from_kind",
        "to_kind",
        "file",
        "line",
        "ctx",
        "edge_type"
    ])
}

fn compact_node_rows(nodes: &[Value]) -> Vec<Value> {
    nodes.iter().map(compact_node_row).collect()
}

fn compact_node_row(node: &Value) -> Value {
    json!([
        node_str(node, "id"),
        node_str(node, "kind"),
        node_str(node, "name"),
        node_str(node, "context"),
        node_str(node, "module_path").or_else(|| node_str(node, "path")),
        node_str(node, "file"),
        node_value(node, "start_line"),
        node_value(node, "end_line"),
        node_str(node, "visibility"),
        node_value(node, "public"),
        node_str(node, "language"),
        node_str(node, "description"),
        node_str(node, "relation"),
    ])
}

fn compact_edge_rows(edges: &[Value]) -> Vec<Value> {
    edges.iter().map(compact_edge_row).collect()
}

fn compact_edge_row(edge: &Value) -> Value {
    json!([
        node_str(edge, "id"),
        node_str(edge, "relation"),
        node_str(edge, "from"),
        node_str(edge, "to"),
        node_str(edge, "from_kind"),
        node_str(edge, "to_kind"),
        node_str(edge, "file"),
        node_value(edge, "line"),
        node_str(edge, "context"),
        node_str(edge, "edge_type").or_else(|| node_str(edge, "reference_kind")),
    ])
}

fn node_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn node_value(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

fn compact_truth_maintenance_json(report: Value) -> Value {
    let asserted = report.get("implemented").or_else(|| report.get("asserted"));
    let scanned = report.get("scanned");
    json!({
        "schema": "axon.truth_maintenance.v1",
        "format": "schema_rows",
        "cols": ["kind", "ok", "ts", "ctx", "ent", "vo", "svc", "repo", "evt"],
        "rows": [
            compact_truth_row("implemented", asserted),
            compact_truth_row("scanned", scanned),
        ],
        "drift_cols": ["status", "count", "computed_us", "basis_us"],
        "drift_row": compact_drift_row(report.get("drift")),
        "assumptions": report.get("assumptions").cloned().unwrap_or_else(|| json!([])),
    })
}

fn compact_truth_row(kind: &str, section: Option<&Value>) -> Value {
    json!([
        kind,
        section
            .and_then(|value| value["available"].as_bool())
            .unwrap_or(false),
        section
            .and_then(|value| value.get("snapshot_timestamp_us"))
            .cloned()
            .unwrap_or(Value::Null),
        section
            .and_then(|value| value["context_count"].as_u64())
            .unwrap_or(0),
        section
            .and_then(|value| value["entity_count"].as_u64())
            .unwrap_or(0),
        section
            .and_then(|value| value["value_object_count"].as_u64())
            .unwrap_or(0),
        section
            .and_then(|value| value["service_count"].as_u64())
            .unwrap_or(0),
        section
            .and_then(|value| value["repository_count"].as_u64())
            .unwrap_or(0),
        section
            .and_then(|value| value["event_count"].as_u64())
            .unwrap_or(0),
    ])
}

fn compact_drift_row(drift: Option<&Value>) -> Value {
    json!([
        drift
            .and_then(|value| value["status"].as_str())
            .unwrap_or("unknown"),
        drift
            .and_then(|value| value["entry_count"].as_u64())
            .unwrap_or(0),
        drift
            .and_then(|value| value.get("computed_at_us"))
            .cloned()
            .unwrap_or(Value::Null),
        drift
            .and_then(|value| value.get("basis_timestamp_us"))
            .cloned()
            .unwrap_or(Value::Null),
    ])
}

fn truth_maintenance_json(store: &Store, workspace_path: &str) -> Value {
    store
        .truth_maintenance_report(workspace_path)
        .ok()
        .and_then(|report| serde_json::to_value(report).ok())
        .unwrap_or_else(|| {
            json!({
                "implemented": {
                    "knowledge_kind": "implemented",
                    "state": "actual",
                    "available": false,
                    "snapshot_timestamp_us": null,
                    "context_count": 0,
                    "entity_count": 0,
                    "value_object_count": 0,
                    "service_count": 0,
                    "repository_count": 0,
                    "event_count": 0
                },
                "scanned": {
                    "knowledge_kind": "scanned",
                    "state": "actual",
                    "available": false,
                    "snapshot_timestamp_us": null,
                    "context_count": 0,
                    "entity_count": 0,
                    "value_object_count": 0,
                    "service_count": 0,
                    "repository_count": 0,
                    "event_count": 0
                },
                "drift": {
                    "available": false,
                    "status": "unavailable",
                    "computed_at_us": null,
                    "basis_timestamp_us": null,
                    "entry_count": 0
                },
                "assumptions": [
                    "Truth maintenance report could not be loaded from the store."
                ]
            })
        })
}

fn stored_claim_result(
    store: &Store,
    workspace_path: &str,
    claim: &PersistedReasoningClaim,
) -> ToolCallResult {
    let mut envelope = with_reasoning_context(
        claim.payload.clone(),
        claim.proof_json(),
        claim.evidence_json(),
        claim.limitation_texts(),
        serde_json::to_value(&claim.provenance).ok(),
    );

    if let Some(object) = envelope.as_object_mut() {
        object.insert("claim_id".into(), json!(claim.claim_id));
        object.insert("claim_kind".into(), json!(claim.claim_kind));
        object.insert("claim_stale".into(), json!(claim.stale));
        let assumptions = claim.assumption_texts();
        if !assumptions.is_empty() {
            object.insert("assumptions".into(), json!(assumptions));
        }
        object.insert(
            "truth_maintenance".into(),
            truth_maintenance_json(store, workspace_path),
        );
    }

    json_result(envelope)
}

fn compact_architecture_status_payload(mut payload: Value) -> Value {
    if payload["implemented"].is_object() {
        payload["implemented"] = compact_status_model(&payload["implemented"]);
    }
    if payload["current"].is_object() {
        payload["current"] = json!({ "same_as": "implemented" });
    }
    if let Some(object) = payload.as_object_mut() {
        object.insert("detail".into(), json!("summary"));
        object.insert(
            "detail_hint".into(),
            json!("Call rust_status with detail='full' or use architecture/get_model for full fields, methods, and semantic annotations."),
        );
    }
    payload
}

fn compact_status_model(model: &Value) -> Value {
    let contexts = model["bounded_contexts"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|context| {
                    json!({
                        "name": context["name"],
                        "module": context["module"],
                        "depends_on": context["depends_on"],
                        "counts": {
                            "entities": context["entities"].as_array().map(|items| items.len()).unwrap_or(0),
                            "services": context["services"].as_array().map(|items| items.len()).unwrap_or(0),
                            "events": context["events"].as_array().map(|items| items.len()).unwrap_or(0),
                            "value_objects": context["value_objects"].as_array().map(|items| items.len()).unwrap_or(0),
                            "repositories": context["repositories"].as_array().map(|items| items.len()).unwrap_or(0),
                        }
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    json!({
        "project": model["project"],
        "description": model["description"],
        "ontology_contract": model["ontology_contract"],
        "rust_ontology": compact_status_rust_ontology(&model["rust_ontology"]),
        "context_count": contexts.len(),
        "contexts": contexts,
    })
}

fn compact_status_rust_ontology(ontology: &Value) -> Value {
    let module_count = ontology["modules"]
        .as_array()
        .map(|items| items.len())
        .unwrap_or(0);
    json!({
        "available": ontology["available"],
        "contract": ontology["contract"],
        "complete_fact_relations": ontology["complete_fact_relations"],
        "overview_projection": ontology["overview_projection"],
        "counts": ontology["counts"],
        "module_count": module_count,
        "modules": ontology["modules"],
        "query_guidance": ontology["query_guidance"],
        "omitted": ["structs"],
    })
}

/// Build a model overview purely from Datalog relations — replaces DomainRegistry.
pub fn build_model_overview(store: &Store, workspace: &str, state: &str) -> Value {
    let rust_ontology = build_rust_ontology_overview(store, workspace, state);

    // Load project metadata
    let project = store.run_datalog(
        "?[name, description, tech_stack_json, conventions_json, rules_json] := \
            *project{workspace: $ws, name, description, tech_stack_json, conventions_json, rules_json}",
        workspace,
    ).unwrap_or_default();

    // Query all contexts early so we can still build an overview for legacy or
    // actual-only states that have data but no project metadata row yet.
    let contexts = store
        .run_datalog(
            &format!(
                "?[name, description, module_path] := \
            *context{{workspace: $ws, name, description, module_path, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let (proj_name, proj_desc, tech, conventions, rules) = if let Some(row) = project.first() {
        (
            row[0].clone(),
            row[1].clone(),
            serde_json::from_str::<Value>(&row[2]).unwrap_or(json!({})),
            serde_json::from_str::<Value>(&row[3]).unwrap_or(json!({})),
            serde_json::from_str::<Value>(&row[4]).unwrap_or(json!([])),
        )
    } else if contexts.is_empty() && !rust_ontology["available"].as_bool().unwrap_or(false) {
        return json!({});
    } else {
        let fallback_name = std::path::Path::new(workspace)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unnamed".into());
        (
            fallback_name,
            String::new(),
            json!({}),
            json!({}),
            json!([]),
        )
    };

    let context_deps = store
        .run_datalog(
            &format!(
                "?[from_ctx, to_ctx] := \
            *context_dep{{workspace: $ws, from_ctx, to_ctx, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let entities = store.run_datalog(
        &format!("?[ctx, name, description, aggregate_root] := \
            *entity{{workspace: $ws, context: ctx, name, description, aggregate_root, state: '{state}' @ 'NOW'}}"),
        workspace,
    ).unwrap_or_default();

    let services = store.run_datalog(
        &format!("?[ctx, name, description, kind] := \
            *service{{workspace: $ws, context: ctx, name, description, kind, state: '{state}' @ 'NOW'}}"),
        workspace,
    ).unwrap_or_default();

    let events = store.run_datalog(
        &format!("?[ctx, name, description, source] := \
            *event{{workspace: $ws, context: ctx, name, description, source, state: '{state}' @ 'NOW'}}"),
        workspace,
    ).unwrap_or_default();

    let value_objects = store.run_datalog(
        &format!("?[ctx, name, description] := \
            *value_object{{workspace: $ws, context: ctx, name, description, state: '{state}' @ 'NOW'}}"),
        workspace,
    ).unwrap_or_default();

    let repositories = store
        .run_datalog(
            &format!(
                "?[ctx, name, aggregate] := \
            *repository{{workspace: $ws, context: ctx, name, aggregate, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let fields = store.run_datalog(
        &format!("?[ctx, owner_kind, owner, name, field_type, required] := \
            *field{{workspace: $ws, context: ctx, owner_kind, owner, name, field_type, required, state: '{state}' @ 'NOW'}}"),
        workspace,
    ).unwrap_or_default();

    let methods = store.run_datalog(
        &format!("?[ctx, owner_kind, owner, name, description, return_type] := \
            *method{{workspace: $ws, context: ctx, owner_kind, owner, name, description, return_type, state: '{state}' @ 'NOW'}}"),
        workspace,
    ).unwrap_or_default();

    let method_params = store.run_datalog(
        &format!("?[ctx, owner_kind, owner, method, name, param_type, required] := \
            *method_param{{workspace: $ws, context: ctx, owner_kind, owner, method, name, param_type, required, state: '{state}' @ 'NOW'}}"),
        workspace,
    ).unwrap_or_default();

    let invariants = store
        .run_datalog(
            &format!(
                "?[ctx, entity, text] := \
            *invariant{{workspace: $ws, context: ctx, entity, text, state: '{state}' @ 'NOW'}}"
            ),
            workspace,
        )
        .unwrap_or_default();

    let vo_rules = store.run_datalog(
        &format!("?[ctx, vo, text] := \
            *vo_rule{{workspace: $ws, context: ctx, value_object: vo, text, state: '{state}' @ 'NOW'}}"),
        workspace,
    ).unwrap_or_default();

    // Assemble per-context JSON
    let bc_json: Vec<Value> = contexts.iter().map(|ctx_row| {
        let ctx_name = &ctx_row[0];

        let deps: Vec<&str> = context_deps.iter()
            .filter(|r| r[0] == *ctx_name)
            .map(|r| r[1].as_str())
            .collect();

        let ctx_entities: Vec<Value> = entities.iter()
            .filter(|r| r[0] == *ctx_name)
            .map(|e| {
                let ent_name = &e[1];
                let ent_fields: Vec<Value> = fields.iter()
                    .filter(|f| f[0] == *ctx_name && f[1] == "entity" && f[2] == *ent_name)
                    .map(|f| json!({"name": f[3], "type": f[4], "required": f[5] == "true"}))
                    .collect();
                let ent_methods: Vec<Value> = methods.iter()
                    .filter(|m| m[0] == *ctx_name && m[1] == "entity" && m[2] == *ent_name)
                    .map(|m| {
                        let params: Vec<Value> = method_params.iter()
                            .filter(|p| p[0] == *ctx_name && p[1] == "entity" && p[2] == *ent_name && p[3] == m[3])
                            .map(|p| json!({"name": p[4], "type": p[5], "required": p[6] == "true"}))
                            .collect();
                        json!({"name": m[3], "description": m[4], "return_type": m[5], "parameters": params})
                    })
                    .collect();
                let ent_invariants: Vec<&str> = invariants.iter()
                    .filter(|i| i[0] == *ctx_name && i[1] == *ent_name)
                    .map(|i| i[2].as_str())
                    .collect();
                json!({
                    "name": ent_name, "description": e[2],
                    "aggregate_root": e[3] == "true",
                    "fields": ent_fields, "methods": ent_methods,
                    "invariants": ent_invariants,
                })
            })
            .collect();

        let ctx_services: Vec<Value> = services.iter()
            .filter(|r| r[0] == *ctx_name)
            .map(|s| {
                let svc_methods: Vec<Value> = methods.iter()
                    .filter(|m| m[0] == *ctx_name && m[1] == "service" && m[2] == s[1])
                    .map(|m| {
                        let params: Vec<Value> = method_params.iter()
                            .filter(|p| p[0] == *ctx_name && p[1] == "service" && p[2] == s[1] && p[3] == m[3])
                            .map(|p| json!({"name": p[4], "type": p[5], "required": p[6] == "true"}))
                            .collect();
                        json!({"name": m[3], "description": m[4], "return_type": m[5], "parameters": params})
                    })
                    .collect();
                json!({"name": s[1], "description": s[2], "kind": s[3], "methods": svc_methods})
            })
            .collect();

        let ctx_events: Vec<Value> = events.iter()
            .filter(|r| r[0] == *ctx_name)
            .map(|ev| {
                let evt_fields: Vec<Value> = fields.iter()
                    .filter(|f| f[0] == *ctx_name && f[1] == "event" && f[2] == ev[1])
                    .map(|f| json!({"name": f[3], "type": f[4], "required": f[5] == "true"}))
                    .collect();
                json!({"name": ev[1], "description": ev[2], "source": ev[3], "fields": evt_fields})
            })
            .collect();

        let ctx_vos: Vec<Value> = value_objects.iter()
            .filter(|r| r[0] == *ctx_name)
            .map(|vo| {
                let vo_fields: Vec<Value> = fields.iter()
                    .filter(|f| f[0] == *ctx_name && f[1] == "value_object" && f[2] == vo[1])
                    .map(|f| json!({"name": f[3], "type": f[4], "required": f[5] == "true"}))
                    .collect();
                let rules: Vec<&str> = vo_rules.iter()
                    .filter(|r| r[0] == *ctx_name && r[1] == vo[1])
                    .map(|r| r[2].as_str())
                    .collect();
                json!({"name": vo[1], "description": vo[2], "fields": vo_fields, "validation_rules": rules})
            })
            .collect();

        let ctx_repos: Vec<Value> = repositories.iter()
            .filter(|r| r[0] == *ctx_name)
            .map(|repo| {
                let repo_methods: Vec<Value> = methods.iter()
                    .filter(|m| m[0] == *ctx_name && m[1] == "repository" && m[2] == repo[1])
                    .map(|m| {
                        let params: Vec<Value> = method_params.iter()
                            .filter(|p| p[0] == *ctx_name && p[1] == "repository" && p[2] == repo[1] && p[3] == m[3])
                            .map(|p| json!({"name": p[4], "type": p[5], "required": p[6] == "true"}))
                            .collect();
                        json!({"name": m[3], "description": m[4], "return_type": m[5], "parameters": params})
                    })
                    .collect();
                json!({"name": repo[1], "aggregate": repo[2], "methods": repo_methods})
            })
            .collect();

        json!({
            "name": ctx_name, "description": ctx_row[1], "module": ctx_row[2],
            "entities": ctx_entities, "services": ctx_services, "events": ctx_events,
            "value_objects": ctx_vos, "repositories": ctx_repos, "depends_on": deps,
        })
    }).collect();

    json!({
        "project": proj_name,
        "description": proj_desc,
        "ontology_contract": {
            "ground_truth": "rust",
            "primary_nodes": ["workspace", "crate", "module", "submodule", "source_file", "symbol"],
            "overview_nodes": ["crate", "module", "submodule", "struct"],
            "semantic_overlays": ["entity_candidate", "value_object_candidate", "service_candidate", "repository_candidate", "event_candidate"],
            "ui": "The web graph is an overview projection; the stored Rust fact graph remains complete for MCP reasoning."
        },
        "rust_ontology": rust_ontology,
        "tech": tech,
        "bounded_contexts": bc_json,
        "rules": rules,
        "conventions": conventions,
    })
}

fn build_rust_ontology_overview(store: &Store, workspace: &str, state: &str) -> Value {
    let model = match state {
        "actual" | "implemented" | "current" => store.load_actual(workspace),
        _ => store.load_actual(workspace),
    }
    .ok()
    .flatten();

    let Some(model) = model else {
        return json!({
            "available": false,
            "contract": "Rust facts are the ground truth; no persisted model is available yet.",
            "counts": {
                "source_files": 0,
                "symbols": 0,
                "structs": 0,
                "enums": 0,
                "traits": 0,
                "functions": 0,
                "methods": 0,
                "imports": 0,
                "calls": 0
            }
        });
    };

    let mut symbol_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut method_counts_by_owner: BTreeMap<String, usize> = BTreeMap::new();
    for symbol in &model.symbols {
        *symbol_counts.entry(symbol.kind.clone()).or_default() += 1;
        if symbol.kind == "method"
            && let Some((owner, _)) = symbol.name.split_once("::")
        {
            *method_counts_by_owner.entry(owner.to_string()).or_default() += 1;
        }
    }

    let modules = rust_modules_for_overview(&model);
    let module_json: Vec<Value> = modules
        .iter()
        .map(|(path, file_count)| {
            json!({
                "path": path,
                "kind": if path.contains("::") { "submodule" } else { "module" },
                "file_count": file_count,
            })
        })
        .collect();

    let structs: Vec<Value> = model
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == "struct")
        .map(|symbol| {
            json!({
                "name": symbol.name,
                "module": rust_module_path_from_file_path(&symbol.file_path),
                "file_path": symbol.file_path,
                "start_line": symbol.start_line,
                "end_line": symbol.end_line,
                "visibility": symbol.visibility,
                "method_count": method_counts_by_owner.get(&symbol.name).copied().unwrap_or_default(),
            })
        })
        .collect();

    json!({
        "available": true,
        "contract": "Rust facts are the ground truth. DDD and pattern terms are semantic overlays, not primary storage nodes.",
        "complete_fact_relations": ["source_file", "symbol", "import_edge", "calls_symbol", "ast_edge", "reference_edge"],
        "overview_projection": {
            "nodes": ["crate", "module", "submodule", "struct"],
            "edges": ["contains", "declares", "imports", "calls"],
            "purpose": "Human-scale architecture overview; not a loss of stored MCP facts."
        },
        "counts": {
            "source_files": model.source_files.len(),
            "symbols": model.symbols.len(),
            "structs": symbol_counts.get("struct").copied().unwrap_or_default(),
            "enums": symbol_counts.get("enum").copied().unwrap_or_default(),
            "traits": symbol_counts.get("trait").copied().unwrap_or_default(),
            "functions": symbol_counts.get("function").copied().unwrap_or_default(),
            "methods": symbol_counts.get("method").copied().unwrap_or_default(),
            "imports": model.import_edges.len(),
            "calls": model.call_edges.len(),
        },
        "modules": module_json,
        "structs": structs,
        "query_guidance": {
            "overview": "Use architecture/get_model for the Rust ontology summary.",
            "connectivity": "Use impact with dependency_graph, call_graph_callers, call_graph_callees, call_graph_reachability, call_graph_stats, optimization_recommendations, or practice_findings.",
            "deletion": "Use safe_to_delete with module + struct/symbol aliases.",
            "refresh": "Use sync after code changes if the watcher has not already updated the graph."
        }
    })
}

fn rust_modules_for_overview(model: &DomainModel) -> BTreeMap<String, usize> {
    let mut modules = BTreeMap::new();
    for source_file in &model.source_files {
        if let Some(path) = rust_module_path_from_file_path(&source_file.path) {
            *modules.entry(path).or_default() += 1;
        }
    }
    for context in &model.bounded_contexts {
        for module in &context.modules {
            if !module.path.is_empty() {
                modules.entry(module.path.clone()).or_insert(0);
            }
        }
    }
    modules
}

fn rust_module_path_from_file_path(file_path: &str) -> Option<String> {
    let normalized = file_path.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let start = parts
        .iter()
        .position(|part| *part == "src")
        .map_or(0, |index| index + 1);
    let relative = &parts[start..];
    let file_name = relative.last()?;
    if !file_name.ends_with(".rs") {
        return None;
    }

    let mut segments: Vec<String> = relative[..relative.len().saturating_sub(1)]
        .iter()
        .map(|segment| segment.to_string())
        .collect();
    let stem = file_name.trim_end_matches(".rs");
    if stem != "mod" {
        if (stem == "lib" || stem == "main") && segments.is_empty() {
            return None;
        }
        segments.push(stem.to_string());
    }
    (!segments.is_empty()).then(|| segments.join("::"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::*;
    use crate::mcp::protocol::ContentBlock;
    use crate::store::default_layer_constraints;
    use std::env::temp_dir;

    fn test_store() -> Store {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = temp_dir().join(format!("axon_tools_test_{}_{}.db", std::process::id(), id));
        Store::open(&path).unwrap()
    }

    #[test]
    fn test_unknown_tool() {
        let store = test_store();
        let result = call_tool(&store, "/tmp/test-tools", "nonexistent_tool", &json!({}));
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn test_list_tools_count() {
        let tools = list_tools();
        assert_eq!(tools.len(), 9);
        let names: Vec<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
        for expected in [
            "rust_status",
            "rust_graph",
            "rust_readiness",
            "rust_impact",
            "rust_delete_safety",
            "rust_invariants",
            "rust_explain",
            "rust_history",
            "rust_search",
        ] {
            assert!(names.contains(&expected));
        }
        for omitted in ["rust_resolve", "rust_health", "rust_path", "rust_diff"] {
            assert!(!names.contains(&omitted));
        }
        for legacy in [
            "get_model",
            "query_rust_graph",
            "model_health",
            "query_blast_radius",
        ] {
            assert!(!names.contains(&legacy));
        }
        for tool in &tools {
            let properties = tool
                .input_schema
                .get("properties")
                .and_then(|value| value.as_object())
                .unwrap_or_else(|| panic!("{} should have object properties", tool.name));
            for key in ["workspace_path", "file_path", "crate", "crate_name"] {
                assert!(
                    properties.contains_key(key),
                    "{} schema should advertise {key}",
                    tool.name
                );
            }
        }
    }

    #[test]
    fn test_status_includes_health_for_empty_store() {
        let store = test_store();
        let result = call_tool(&store, "/tmp/test-tools", "rust_status", &json!({}));
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["status"], "no_model");
        assert!(parsed["health"]["score"].is_number());
        assert_eq!(parsed["graph_confidence"]["status"], "not_scanned");
        assert_eq!(parsed["readiness_summary"]["status"], "not_scanned");
    }

    #[test]
    fn test_history_latest_diff_compares_recent_snapshots() {
        let store = test_store();
        let ws = format!("/tmp/test-history-latest-diff-{}", std::process::id());
        let mut first = DomainModel::empty("HistoryDiff");
        first.bounded_contexts = vec![BoundedContext {
            name: "Catalog".into(),
            description: "Catalog context".into(),
            module_path: "src/catalog".into(),
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
            api_endpoints: vec![],
        }];
        store.save_actual(&ws, &first).unwrap();

        let mut second = first.clone();
        second.bounded_contexts.push(BoundedContext {
            name: "Billing".into(),
            description: "Billing context".into(),
            module_path: "src/billing".into(),
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
            api_endpoints: vec![],
        });
        store.save_actual(&ws, &second).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_history",
            &json!({ "mode": "latest_diff" }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["status"], "latest_diff");
        assert_eq!(parsed["claim_kind"], "history");
        assert_eq!(parsed["summary"]["additions"], 1);
        assert!(
            parsed["added"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| { change["kind"] == "context" && change["name"] == "Billing" })
        );
    }

    #[test]
    fn test_readiness_reports_runtime_and_environment() {
        let store = test_store();
        let result = call_tool(&store, "/tmp/test-tools", "rust_readiness", &json!({}));
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["status"], "not_scanned");
        assert_eq!(parsed["runtime"]["axon_version"], crate::VERSION);
        assert!(parsed["environment"]["rust_analyzer"].is_object());
        assert!(parsed["environment"]["cargo_metadata"].is_object());
        assert_eq!(parsed["graph_confidence"]["status"], "not_scanned");
        assert!(
            parsed["graph_confidence"]["capabilities"]["semantic_call_graph"]["persisted_fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field == "call_expr")
        );
        assert_eq!(
            parsed["graph_confidence"]["capabilities"]["cfg_feature_awareness"]["persisted_profile_slices"],
            false
        );
        assert_eq!(
            parsed["graph_confidence"]["capabilities"]["generated_code"]["proc_macro_expansion"],
            "disabled"
        );
    }

    #[test]
    fn test_architecture_dispatch() {
        let store = test_store();
        let ws = format!("/tmp/test-architecture-{}", std::process::id());
        let model = DomainModel {
            name: "ArchProject".into(),
            description: "Architecture test".into(),
            bounded_contexts: vec![BoundedContext {
                name: "Billing".into(),
                description: "Billing context".into(),
                module_path: "src/billing".into(),
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
                api_endpoints: vec![],
            }],
            external_systems: vec![],
            architectural_decisions: vec![],
            ownership: Ownership::default(),
            rules: vec![],
            tech_stack: TechStack::default(),
            conventions: Conventions::default(),
            ast_edges: vec![],
            source_files: vec![SourceFile {
                path: "src/billing/service.rs".into(),
                context: "Billing".into(),
                language: "rust".into(),
            }],
            symbols: vec![SymbolDef {
                name: "BillingService".into(),
                kind: "struct".into(),
                context: "Billing".into(),
                file_path: "src/billing/service.rs".into(),
                start_line: 1,
                end_line: 12,
                visibility: "public".into(),
            }],
            import_edges: vec![],
            call_edges: vec![],
            reference_edges: vec![],
        };
        store.save_desired(&ws, &model).unwrap();
        store.save_actual(&ws, &model).unwrap();
        store.compute_drift(&ws).unwrap();

        let result = call_tool(&store, &ws, "rust_status", &json!({}));
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["claim_kind"], "architecture");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["detail"], "summary");
        assert!(parsed["implemented"].is_object());
        assert_eq!(
            parsed["implemented"]["ontology_contract"]["ground_truth"],
            "rust"
        );
        assert_eq!(parsed["implemented"]["context_count"], 1);
        assert!(parsed["implemented"]["contexts"].is_array());
        assert!(parsed["implemented"]["bounded_contexts"].is_null());
        assert_eq!(parsed["implemented"]["rust_ontology"]["available"], true);
        assert_eq!(
            parsed["implemented"]["rust_ontology"]["counts"]["structs"],
            1
        );
        assert_eq!(parsed["health"]["score"], 96);
        assert_eq!(
            parsed["health"]["policy_coverage"]["dependency_constraint_count"],
            default_layer_constraints().len()
        );

        let full_result = call_tool(&store, &ws, "rust_status", &json!({"detail": "full"}));
        assert_eq!(full_result.is_error, None);
        let ContentBlock::Text { text } = &full_result.content[0];
        let full: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(full["implemented"]["bounded_contexts"].is_array());
    }

    #[test]
    fn test_graph_dispatch_exposes_rust_facts() {
        let store = test_store();
        let ws = format!("/tmp/test-graph-{}", std::process::id());
        let model = DomainModel {
            name: "GraphProject".into(),
            description: "Graph test".into(),
            bounded_contexts: vec![BoundedContext {
                name: "Core".into(),
                description: "Core context".into(),
                module_path: "src/core".into(),
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
                api_endpoints: vec![],
            }],
            external_systems: vec![],
            architectural_decisions: vec![],
            ownership: Ownership::default(),
            rules: vec![],
            tech_stack: TechStack::default(),
            conventions: Conventions::default(),
            ast_edges: vec![],
            source_files: vec![SourceFile {
                path: "src/core/lib.rs".into(),
                context: "Core".into(),
                language: "rust".into(),
            }],
            symbols: vec![SymbolDef {
                name: "CoreService".into(),
                kind: "struct".into(),
                context: "Core".into(),
                file_path: "src/core/lib.rs".into(),
                start_line: 3,
                end_line: 12,
                visibility: "public".into(),
            }],
            import_edges: vec![ImportEdge {
                from_file: "src/core/lib.rs".into(),
                to_module: "crate::store::Store".into(),
                context: "Core".into(),
            }],
            call_edges: vec![CallEdge {
                caller: "CoreService::run".into(),
                callee: "Store::load_actual".into(),
                file_path: "src/core/lib.rs".into(),
                line: 9,
                context: "Core".into(),
            }],
            reference_edges: vec![ReferenceEdge {
                from_file: "src/core/lib.rs".into(),
                to_path: "crate::domain::model::DomainModel".into(),
                reference_kind: "type".into(),
                line: 8,
                context: "Core".into(),
            }],
        };
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_graph",
            &json!({
                "view": "neighborhood",
                "symbol": "CoreService",
                "limit": 20
            }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["view"], "neighborhood");
        assert_eq!(parsed["format"], "schema_rows");
        assert_eq!(parsed["schema"], "axon.rust_graph.neighborhood.v1");
        assert_eq!(parsed["provenance"]["state"], "actual");
        assert!(
            parsed["tables"]["nodes"]["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| { row[0] == "symbol:CoreService" && row[1] == "struct" })
        );
        assert!(
            parsed["proof"]["derived_from"]
                .as_array()
                .unwrap()
                .iter()
                .any(|relation| { relation.as_str() == Some("symbol") })
        );

        let edge_result = call_tool(
            &store,
            &ws,
            "rust_graph",
            &json!({
                "view": "edges",
                "relation": "reference_edge",
                "to": "DomainModel",
                "limit": 20
            }),
        );
        assert_eq!(edge_result.is_error, None);
        let ContentBlock::Text { text } = &edge_result.content[0];
        let edge_payload: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(edge_payload["status"], "ok");
        assert_eq!(edge_payload["view"], "edges");
        assert_eq!(edge_payload["format"], "schema_rows");
        assert_eq!(edge_payload["schema"], "axon.rust_graph.edges.v1");
        assert!(edge_payload["rows"].as_array().unwrap().iter().any(|row| {
            row[1] == "reference_edge"
                && row[2] == "src/core/lib.rs"
                && row[3] == "crate::domain::model::DomainModel"
                && row[7] == 8
                && row[8] == "Core"
                && row[9] == "type"
        }));
    }

    #[test]
    fn test_graph_symbol_neighborhood_filters_before_limit() {
        let store = test_store();
        let ws = format!("/tmp/test-graph-symbol-{}", std::process::id());
        let mut model = DomainModel::empty(&ws);
        model.bounded_contexts = (0..30)
            .map(|idx| BoundedContext {
                name: format!("Context{idx:02}"),
                description: "".into(),
                module_path: format!("src/context_{idx:02}"),
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
                api_endpoints: vec![],
            })
            .collect();
        model.call_edges = (0..600)
            .map(|idx| CallEdge {
                caller: format!("A{idx:03}::run"),
                callee: "Irrelevant".into(),
                file_path: format!("src/context_{:02}/lib.rs", idx % 30),
                line: idx,
                context: format!("Context{:02}", idx % 30),
            })
            .chain(std::iter::once(CallEdge {
                caller: "ZCaller::run".into(),
                callee: "TargetSymbol".into(),
                file_path: "src/context_29/lib.rs".into(),
                line: 700,
                context: "Context29".into(),
            }))
            .collect();
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_graph",
            &json!({
                "view": "neighborhood",
                "relation": "calls_symbol",
                "symbol": "TargetSymbol",
                "limit": 5
            }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["status"], "ok");
        assert!(
            parsed["tables"]["nodes"]["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| {
                    row[0] == "symbol:TargetSymbol"
                        && row[2] == "TargetSymbol"
                        && row[12] == "calls_symbol"
                })
        );
        assert!(
            parsed["tables"]["edges"]["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| {
                    row[1] == "calls_symbol" && row[2] == "ZCaller::run" && row[3] == "TargetSymbol"
                })
        );
    }

    #[test]
    fn test_impact_dispatch() {
        let store = test_store();
        let ws = format!("/tmp/test-impact-{}", std::process::id());
        store
            .save_desired(
                &ws,
                &DomainModel {
                    name: "ImpactProject".into(),
                    description: "Impact test".into(),
                    bounded_contexts: vec![
                        BoundedContext {
                            name: "A".into(),
                            description: "A".into(),
                            module_path: "src/a".into(),
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
                            dependencies: vec!["B".into()],
                            api_endpoints: vec![],
                        },
                        BoundedContext {
                            name: "B".into(),
                            description: "B".into(),
                            module_path: "src/b".into(),
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
                            api_endpoints: vec![],
                        },
                    ],
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
                },
            )
            .unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_impact",
            &json!({
                "analysis": "transitive_deps",
                "context": "A"
            }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["claim_kind"], "impact");
        assert_eq!(parsed["analysis"], "transitive_deps");
        assert_eq!(parsed["count"], 1);
    }

    #[test]
    fn test_impact_dispatch_refreshes_call_graph_after_rescan() {
        let store = test_store();
        let ws = format!("/tmp/test-impact-refresh-{}", std::process::id());
        store.save_actual(&ws, &DomainModel::empty(&ws)).unwrap();

        let initial = call_tool(
            &store,
            &ws,
            "rust_impact",
            &json!({"analysis": "call_graph_callers", "symbol": "Store::query_call_paths"}),
        );
        assert_eq!(initial.is_error, None);
        let ContentBlock::Text { text } = &initial.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["result"]["count"], 0);

        let mut model = DomainModel::empty(&ws);
        model.call_edges = vec![CallEdge {
            caller: "ReasoningKernel::how_connected_claim".into(),
            callee: "query_call_paths".into(),
            file_path: "src/reasoning/mod.rs".into(),
            line: 10,
            context: "reasoning".into(),
        }];
        store.save_actual(&ws, &model).unwrap();

        let refreshed = call_tool(
            &store,
            &ws,
            "rust_impact",
            &json!({"analysis": "call_graph_callers", "symbol": "Store::query_call_paths"}),
        );
        assert_eq!(refreshed.is_error, None);
        let ContentBlock::Text { text } = &refreshed.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["result"]["count"], 1);
        assert_eq!(
            parsed["result"]["callers"][0]["caller"],
            "ReasoningKernel::how_connected_claim"
        );
    }

    #[test]
    fn test_impact_call_graph_stats_proof_cites_project_symbols() {
        let store = test_store();
        let ws = format!("/tmp/test-impact-stats-proof-{}", std::process::id());
        let mut model = DomainModel::empty(&ws);
        model.symbols = vec![SymbolDef {
            name: "Store::load_actual".into(),
            kind: "method".into(),
            context: "store".into(),
            file_path: "src/store/cozo.rs".into(),
            start_line: 1,
            end_line: 1,
            visibility: "public".into(),
        }];
        model.call_edges = vec![CallEdge {
            caller: "ReasoningKernel::architecture".into(),
            callee: "load_actual".into(),
            file_path: "src/reasoning/mod.rs".into(),
            line: 10,
            context: "reasoning".into(),
        }];
        store.save_actual(&ws, &model).unwrap();
        store
            .save_resolved_calls(
                &ws,
                &[crate::domain::rust_analyzer::ResolvedCall {
                    caller: "ReasoningKernel::architecture".into(),
                    callee: "Store::load_actual".into(),
                    callee_file: "src/store/cozo.rs".into(),
                    callee_line: 1,
                    ..Default::default()
                }],
            )
            .unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_impact",
            &json!({"analysis": "call_graph_stats"}),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        let derived_from = parsed["proof"]["derived_from"].as_array().unwrap();
        assert!(derived_from.iter().any(|item| item == "calls_symbol"));
        assert!(derived_from.iter().any(|item| item == "resolved_call"));
        assert!(derived_from.iter().any(|item| item == "symbol"));
        assert_eq!(parsed["proof"]["witness_count"], 1);
        assert_eq!(parsed["result"]["project_callee_edges"], 1);
        assert_eq!(parsed["result"]["resolved_project_callee_edges"], 1);
        assert_eq!(
            parsed["result"]["hottest_project_callees"][0]["call_graph_relation"],
            "calls_symbol"
        );
        assert_eq!(
            parsed["result"]["hottest_resolved_project_callees"][0]["callee"],
            "Store::load_actual"
        );
        assert_eq!(
            parsed["result"]["hottest_resolved_project_callees"][0]["call_graph_relation"],
            "resolved_call"
        );
    }

    #[test]
    fn test_impact_optimization_recommendations_dispatch() {
        let store = test_store();
        let ws = format!(
            "/tmp/test-optimization-recommendations-{}",
            std::process::id()
        );
        let mut model = DomainModel::empty(&ws);
        model.source_files = vec![
            SourceFile {
                path: "src/domain/rust_syn.rs".into(),
                context: "domain".into(),
                language: "rust".into(),
            },
            SourceFile {
                path: "src/mcp/tools.rs".into(),
                context: "mcp".into(),
                language: "rust".into(),
            },
            SourceFile {
                path: "src/mcp/resources.rs".into(),
                context: "mcp".into(),
                language: "rust".into(),
            },
            SourceFile {
                path: "src/server/web.rs".into(),
                context: "server".into(),
                language: "rust".into(),
            },
        ];
        model.import_edges = vec![
            ImportEdge {
                from_file: "src/mcp/tools.rs".into(),
                to_module: "crate::domain::rust_syn::RustSynScanner".into(),
                context: "mcp".into(),
            },
            ImportEdge {
                from_file: "src/mcp/resources.rs".into(),
                to_module: "crate::domain::rust_syn::RustSynScanner".into(),
                context: "mcp".into(),
            },
            ImportEdge {
                from_file: "src/server/web.rs".into(),
                to_module: "crate::domain::rust_syn::RustSynScanner".into(),
                context: "server".into(),
            },
        ];
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_impact",
            &json!({"analysis": "optimization_recommendations"}),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        let recommendations = parsed["result"]["recommendations"].as_array().unwrap();
        assert!(
            recommendations
                .iter()
                .any(|recommendation| recommendation["kind"] == "facade"),
            "expected facade recommendation: {parsed}"
        );
        let derived_from = parsed["proof"]["derived_from"].as_array().unwrap();
        assert!(derived_from.iter().any(|item| item == "import_edge"));
        assert!(derived_from.iter().any(|item| item == "symbol"));
    }

    #[test]
    fn test_impact_practice_findings_dispatch() {
        let store = test_store();
        let ws = format!("/tmp/test-practice-findings-{}", std::process::id());
        let mut model = DomainModel::empty(&ws);
        model.call_edges = vec![CallEdge {
            caller: "Store::load".into(),
            callee: "unwrap".into(),
            file_path: "src/store/cozo.rs".into(),
            line: 30,
            context: "store".into(),
        }];
        model.ast_edges = vec![ASTEdge {
            from_node: "Store::legacy".into(),
            to_node: "allow(dead_code)".into(),
            edge_type: "decorators".into(),
            file_path: "src/store/cozo.rs".into(),
            line: 8,
        }];
        model.reference_edges = vec![ReferenceEdge {
            from_file: "src/store/cozo.rs".into(),
            to_path: "panic".into(),
            reference_kind: "macro".into(),
            line: 35,
            context: "store".into(),
        }];
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_impact",
            &json!({"analysis": "practice_findings"}),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        let findings = parsed["result"]["findings"].as_array().unwrap();
        assert!(
            findings
                .iter()
                .any(|finding| finding["kind"] == "panic_macro"),
            "expected panic finding: {parsed}"
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding["kind"] == "unchecked_unwrap"),
            "expected unwrap finding: {parsed}"
        );
        let derived_from = parsed["proof"]["derived_from"].as_array().unwrap();
        assert!(derived_from.iter().any(|item| item == "calls_symbol"));
        assert!(derived_from.iter().any(|item| item == "ast_edge"));
        assert!(derived_from.iter().any(|item| item == "reference_edge"));
    }

    #[test]
    fn test_history_dispatch() {
        let store = test_store();
        let ws = format!("/tmp/test-history-{}", std::process::id());
        store
            .save_desired(
                &ws,
                &DomainModel {
                    name: "HistoryProject".into(),
                    description: "History test".into(),
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
                },
            )
            .unwrap();

        let result = call_tool(&store, &ws, "rust_history", &json!({"state": "planned"}));
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["claim_kind"], "history");
        assert_eq!(parsed["status"], "listing");
        assert_eq!(parsed["count"], 1);
    }

    #[test]
    fn test_search_dispatch() {
        let store = test_store();
        let ws = format!("/tmp/test-search-{}", std::process::id());
        store
            .save_desired(
                &ws,
                &DomainModel {
                    name: "SearchProject".into(),
                    description: "Search test".into(),
                    bounded_contexts: vec![BoundedContext {
                        name: "Identity".into(),
                        description: "User identity context".into(),
                        module_path: "src/identity".into(),
                        ownership: Ownership::default(),
                        aggregates: vec![],
                        policies: vec![],
                        read_models: vec![],
                        entities: vec![Entity {
                            name: "UserAccount".into(),
                            description: "Stores user identity data".into(),
                            aggregate_root: true,
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
                        dependencies: vec![],
                        api_endpoints: vec![],
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
                    reference_edges: vec![],
                },
            )
            .unwrap();

        let result = call_tool(&store, &ws, "rust_search", &json!({"query": "identity"}));
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["claim_kind"], "search");
        assert_eq!(parsed["query"], "identity");
        assert!(parsed["count"].as_u64().unwrap_or(0) >= 1);
    }

    #[test]
    fn test_search_dispatch_includes_policy_sources() {
        let store = test_store();
        let ws = format!("/tmp/test-search-policy-{}", std::process::id());
        store.save_actual(&ws, &DomainModel::empty(&ws)).unwrap();
        store
            .upsert_layer_assignment(&ws, "Domain", "domain")
            .unwrap();
        store
            .upsert_dependency_constraint(&ws, "layer", "domain", "infrastructure", "forbidden")
            .unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_search",
            &json!({"query": "policy dependency constraints"}),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["claim_kind"], "search");
        assert!(parsed["count"].as_u64().unwrap_or(0) >= 1);
        let derived_from = parsed["proof"]["derived_from"].as_array().unwrap();
        assert!(derived_from.iter().any(|item| item == "layer_assignment"));
        assert!(
            derived_from
                .iter()
                .any(|item| item == "dependency_constraint")
        );
    }

    #[test]
    fn test_search_dispatch_finds_rust_symbols() {
        let store = test_store();
        let ws = format!("/tmp/test-search-rust-symbol-{}", std::process::id());
        let mut model = DomainModel::empty(&ws);
        model.symbols = vec![SymbolDef {
            name: "Store::query_call_paths".into(),
            kind: "method".into(),
            context: "store".into(),
            file_path: "src/store/cozo.rs".into(),
            start_line: 10,
            end_line: 20,
            visibility: "public".into(),
        }];
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_search",
            &json!({"query": "query_call_paths"}),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["claim_kind"], "search");
        assert_eq!(parsed["count"], 1);
        assert_eq!(parsed["results"][0]["kind"], "symbol");
        assert_eq!(parsed["results"][0]["search_mode"], "rust_fact_scan");
        let derived_from = parsed["proof"]["derived_from"].as_array().unwrap();
        assert!(derived_from.iter().any(|item| item == "symbol"));
        assert_eq!(parsed["provenance"]["source"], "architecture_search");
    }

    #[test]
    fn test_search_dispatch_limits_rust_fact_scan_results() {
        let store = test_store();
        let ws = format!("/tmp/test-search-limit-{}", std::process::id());
        let mut model = DomainModel::empty(&ws);
        model.call_edges = (0..8)
            .map(|idx| CallEdge {
                caller: "QueryOwner::run".into(),
                callee: format!("Target{idx}"),
                file_path: "src/query.rs".into(),
                line: idx,
                context: "query".into(),
            })
            .collect();
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_search",
            &json!({"query": "QueryOwner", "limit": 3}),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["count"], 3);
        assert_eq!(parsed["total_before_limit"], 8);
        assert_eq!(parsed["truncated"], true);
        assert_eq!(parsed["results"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_safe_to_delete_dispatch() {
        let store = test_store();
        let ws = format!("/tmp/test-can-del-{}", std::process::id());
        store
            .save_desired(
                &ws,
                &DomainModel {
                    name: "P".into(),
                    description: "".into(),
                    bounded_contexts: vec![BoundedContext {
                        name: "Sales".into(),
                        description: "".into(),
                        module_path: "src/sales".into(),
                        ownership: Ownership::default(),
                        aggregates: vec![],
                        policies: vec![],
                        read_models: vec![],
                        entities: vec![Entity {
                            name: "Order".into(),
                            description: "".into(),
                            aggregate_root: true,
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
                        dependencies: vec![],
                        api_endpoints: vec![],
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
                    reference_edges: vec![],
                },
            )
            .unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_delete_safety",
            &json!({
                "context": "Sales",
                "entity": "Order"
            }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        // Order has no references, so it should be deletable
        assert_eq!(parsed["status"], "true");
        assert_eq!(parsed["claim_kind"], "safe_to_delete");
        assert_eq!(parsed["provenance"]["state"], "actual");
        assert!(
            parsed["proof"]["rule"]
                .as_str()
                .unwrap()
                .contains("deletable")
        );
        assert!(!parsed["limitations"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_safe_to_delete_accepts_symbol_without_context() {
        let store = test_store();
        let ws = format!("/tmp/test-can-del-symbol-{}", std::process::id());
        let model = DomainModel {
            name: "P".into(),
            description: "".into(),
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
            call_edges: vec![CallEdge {
                caller: "Caller::run".into(),
                callee: "TargetSymbol".into(),
                file_path: "src/caller.rs".into(),
                line: 7,
                context: "Core".into(),
            }],
            reference_edges: vec![],
        };
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_delete_safety",
            &json!({ "symbol": "TargetSymbol" }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["can_delete"], false);
        assert_eq!(parsed["entity"], "TargetSymbol");
        assert_eq!(
            parsed["result"]["call_references"][0]["caller"],
            "Caller::run"
        );
    }

    #[test]
    fn test_safe_to_delete_counts_type_reference_witnesses() {
        let store = test_store();
        let ws = format!("/tmp/test-can-del-types-{}", std::process::id());
        let mut model = DomainModel::empty(&ws);
        model.bounded_contexts = vec![BoundedContext {
            name: "Store".into(),
            description: "".into(),
            module_path: "src/store".into(),
            ownership: Ownership::default(),
            aggregates: vec![],
            policies: vec![],
            read_models: vec![],
            entities: vec![Entity {
                name: "ModelHealth".into(),
                description: "".into(),
                aggregate_root: false,
                fields: vec![Field {
                    name: "policy_coverage".into(),
                    field_type: "PolicyCoverage".into(),
                    required: true,
                    description: "".into(),
                }],
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
            dependencies: vec![],
            api_endpoints: vec![],
        }];
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_delete_safety",
            &json!({ "symbol": "PolicyCoverage" }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["can_delete"], false);
        assert_eq!(parsed["proof"]["witness_count"], 1);
        assert_eq!(
            parsed["result"]["type_references"]["fields"][0]["owner"],
            "ModelHealth"
        );
        let derived_from = parsed["proof"]["derived_from"].as_array().unwrap();
        assert!(derived_from.iter().any(|item| item == "field"));
    }

    #[test]
    fn test_check_dispatch() {
        let store = test_store();
        let ws = format!("/tmp/test-invariant-{}", std::process::id());
        // No data = no violations
        let result = call_tool(
            &store,
            &ws,
            "rust_invariants",
            &json!({
                "check_name": "circular_deps"
            }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["status"], "true");
        assert_eq!(parsed["count"], 0);
        assert_eq!(parsed["provenance"]["state"], "actual");
        assert_eq!(parsed["proof"]["derived_from"][0], "context_dep");
        assert!(!parsed["limitations"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_check_unknown() {
        let store = test_store();
        let result = call_tool(
            &store,
            "/tmp/test",
            "rust_invariants",
            &json!({
                "check_name": "nonexistent"
            }),
        );
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn test_graph_paths_uses_dependency_relation() {
        let store = test_store();
        let ws = format!("/tmp/test-deppath-{}", std::process::id());
        store
            .save_actual(
                &ws,
                &DomainModel {
                    name: "P".into(),
                    description: "".into(),
                    bounded_contexts: vec![
                        BoundedContext {
                            name: "A".into(),
                            description: "".into(),
                            module_path: "src/a".into(),
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
                            dependencies: vec!["B".into()],
                            api_endpoints: vec![],
                        },
                        BoundedContext {
                            name: "B".into(),
                            description: "".into(),
                            module_path: "src/b".into(),
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
                            api_endpoints: vec![],
                        },
                    ],
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
                },
            )
            .unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_graph",
            &json!({
                "view": "paths",
                "relation": "context_dep",
                "from": "A",
                "to": "B"
            }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["reachable"], true);
        assert_eq!(parsed["view"], "paths");
        assert_eq!(parsed["relation"], "context_dep");
        assert_eq!(parsed["rows"][0][0], json!(["A", "B"]));
        assert_eq!(parsed["provenance"]["state"], "actual");
        assert_eq!(parsed["proof"]["derived_from"][0], "context_dep");
        assert!(!parsed["limitations"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_graph_paths_uses_call_graph_relation() {
        let store = test_store();
        let ws = format!("/tmp/test-callpath-{}", std::process::id());
        let mut model = DomainModel::empty(&ws);
        model.call_edges = vec![CallEdge {
            caller: "Store::refresh_runtime_constraints".into(),
            callee: "seed_default_constraints".into(),
            file_path: "src/store/cozo.rs".into(),
            line: 4102,
            context: "store".into(),
        }];
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_graph",
            &json!({
                "view": "paths",
                "relation": "calls_symbol",
                "from": "Store::refresh_runtime_constraints",
                "to": "seed_default_constraints"
            }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let graph: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(graph["reachable"], true);
        assert_eq!(graph["relation"], "calls_symbol");
        assert_eq!(
            graph["rows"][0][0],
            json!([
                "Store::refresh_runtime_constraints",
                "seed_default_constraints"
            ])
        );
    }

    #[test]
    fn test_why_dispatch_includes_reasoning_context() {
        let store = test_store();
        let ws = format!("/tmp/test-why-{}", std::process::id());

        let result = call_tool(
            &store,
            &ws,
            "rust_explain",
            &json!({
                "violation_type": "layer_violations"
            }),
        );

        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["status"], "true");
        assert_eq!(parsed["provenance"]["state"], "actual");
        assert_eq!(parsed["proof"]["witness_count"], 0);
        assert!(parsed["evidence"].as_array().unwrap().is_empty());
        assert!(!parsed["limitations"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_build_model_overview_for_actual_only_model() {
        let store = test_store();
        let ws = format!("/tmp/test-actual-overview-{}", std::process::id());

        store
            .save_actual(
                &ws,
                &DomainModel {
                    name: "ActualOnly".into(),
                    description: "Actual state only".into(),
                    bounded_contexts: vec![BoundedContext {
                        name: "Orders".into(),
                        description: "Ordering context".into(),
                        module_path: "src/orders".into(),
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
                        api_endpoints: vec![],
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
                    reference_edges: vec![],
                },
            )
            .unwrap();

        let overview = build_model_overview(&store, &ws, "actual");
        assert_eq!(overview["project"], "ActualOnly");
        assert_eq!(overview["bounded_contexts"].as_array().unwrap().len(), 1);
        assert_eq!(overview["bounded_contexts"][0]["name"], "Orders");
    }
}
