use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::domain::model::DomainModel;
use crate::mcp::protocol::*;
use crate::reasoning::ReasoningKernel;
use crate::store::Store;
use crate::store::cozo::PersistedReasoningClaim;

/// Returns the list of tools the Axon server exposes.
pub fn list_tools() -> Vec<ToolDefinition> {
    rust_native_read_tools()
}

fn rust_native_read_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "rust_status".into(),
            description: "Show the current actual-state Rust model: crate inventory, module tree, source files, Rust symbols, imports, calls, semantic annotations, health, and snapshot freshness. Call this first before planning Rust changes.".into(),
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
        ToolDefinition {
            name: "rust_graph".into(),
            description: "Query persisted actual-state Rust facts through bounded graph views: modules, source files, symbols, import edges, call edges, AST edges, neighborhoods, paths, and relation counts. AST edges (`relation=ast_edge`) carry `extends`/`implements` plus compiler directives as `decorators` edges — `to_node` is the directive text (e.g. `allow(dead_code)`, `cfg(feature = \"x\")`, `Debug`) and `from_node` the annotated item (`Owner::method`, `Owner.field`, or a bare name), each with a `file`/`line`. So `view=edges, relation=ast_edge, to=\"dead_code\"` lists every dead-code flag in one call (directives are captured on items/fields of any visibility). Results use compact schema-plus-rows JSON (`schema`, `cols`, `rows`) for repeated facts. Arbitrary Datalog is not exposed.".into(),
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
                        "enum": ["all", "context_dep", "import_edge", "calls_symbol", "ast_edge", "resolved_call"],
                        "description": "Rust relation filter for edges/paths views"
                    },
                    "module": { "type": "string", "description": "Rust module path/name filter" },
                    "file": { "type": "string", "description": "Source file filter" },
                    "symbol": { "type": "string", "description": "Rust symbol filter" },
                    "struct": { "type": "string", "description": "Rust struct-name alias for symbol" },
                    "from": { "type": "string", "description": "Source node for paths or edge filtering" },
                    "to": { "type": "string", "description": "Target node for paths or edge filtering" },
                    "limit": { "type": "integer", "description": "Max returned rows per collection (default: 50, max: 200)", "default": 50 }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "rust_resolve".into(),
            description: "Ingest a compiler-resolved call graph from rust-analyzer to complement the syn scanner. Drives a rust-analyzer LSP session (name resolution + type inference) to resolve each call site to the concrete function it actually targets — which the syn scanner cannot determine — and persists them into the `resolved_call` relation with the callee's definition location. Opt-in and slow: spawns rust-analyzer and waits for it to index the workspace (tens of seconds); needs the `rust-analyzer` component (`rustup component add rust-analyzer`, runs on stable). After it runs, query via `rust_graph(view=edges, relation=resolved_call, from=\"Store::save\")` (what a function actually calls) or `to=\"save_state\"` (who resolves to a callee).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: "rust_health".into(),
            description: "Compute a Datalog-derived health report over actual Rust facts and optional semantic annotations: score, cycles, layer violations, missing invariants, orphan modules/contexts, and graph analytics when available.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: "rust_impact".into(),
            description: "Analyze blast radius in the actual Rust graph. Use module for dependency analysis and symbol or struct for call graph analysis.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "analysis": {
                        "type": "string",
                        "enum": ["transitive_deps", "circular_deps", "layer_violations", "impact_analysis",
                                 "aggregate_quality", "dependency_graph", "field_usage", "method_search",
                                 "shared_fields", "pagerank", "community_detection", "betweenness_centrality",
                                 "degree_centrality", "topological_order",
                                 "call_graph_callers", "call_graph_callees", "call_graph_reachability", "call_graph_stats"],
                        "description": "The specific actual-state Rust analysis to run"
                    },
                    "module": { "type": "string", "description": "Rust module name/path for dependency analyses" },
                    "struct": { "type": "string", "description": "Rust struct name for struct/entity or call graph analyses" },
                    "symbol": { "type": "string", "description": "Rust symbol name" },
                    "field_type": { "type": "string", "description": "Field type to search (required for field_usage)" },
                    "method_name": { "type": "string", "description": "Method name to search (required for method_search)" }
                },
                "required": ["analysis"]
            }),
        },
        ToolDefinition {
            name: "rust_delete_safety".into(),
            description: "Check whether a Rust symbol or struct can be safely deleted. Evaluates inbound call edges, imports, AST references, and semantic annotation dependents. Module is optional; omit it for a workspace-wide symbol check.".into(),
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
        ToolDefinition {
            name: "rust_invariants".into(),
            description: "Check actual Rust graph invariants and configured constraints: circular dependencies, layer violations, missing invariants on annotated core structs, isolated modules, policy violations, and drift freshness. Run without parameters to check everything.".into(),
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
        ToolDefinition {
            name: "rust_path".into(),
            description: "Show how two Rust modules/components or symbols are connected. Returns proof paths over stored dependency facts or call-graph reachability when available.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "Source Rust module or component" },
                    "to": { "type": "string", "description": "Target Rust module or component" },
                    "relation": { "type": "string", "enum": ["context_dep", "calls_symbol"], "description": "Connectivity relation to traverse (default: context_dep)" }
                },
                "required": ["from", "to"]
            }),
        },
        ToolDefinition {
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
        ToolDefinition {
            name: "rust_diff".into(),
            description: "Compare the two most recent actual Rust graph snapshots. Shows added and removed Rust facts over time.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: "rust_history".into(),
            description: "List actual Rust graph snapshots or compare two snapshot timestamps to show what changed between them.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "state": {
                        "type": "string",
                        "enum": ["actual", "implemented", "current", "planned"],
                        "description": "History stream to query (default: actual; implemented/current/planned are accepted aliases)"
                    },
                    "ts_old": {
                        "type": "integer",
                        "description": "Older snapshot timestamp (microseconds). Required for comparison."
                    },
                    "ts_new": {
                        "type": "integer",
                        "description": "Newer snapshot timestamp (microseconds). Omit for latest."
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "rust_search".into(),
            description: "Search actual Rust facts and semantic annotations by keyword. Finds modules, structs, symbols, labels, and decisions across the codebase.".into(),
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

#[allow(dead_code)]
fn legacy_read_tools() -> Vec<ToolDefinition> {
    let mut tools = vec![
        ToolDefinition {
            name: "architecture".into(),
            description: "Show the complete implemented Rust architecture contract: workspace, \
                          crate, modules/submodules, source files, Rust symbols, imports, calls, \
                          and semantic overlays. Includes the compact overview projection used by \
                          the web UI plus health and temporal change status. Call this first before \
                          changing a Rust codebase."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: "graph".into(),
            description: "Query the Rust graph database directly through bounded, structured views. \
                          Exposes source files, symbols, modules, imports, call edges, dependency \
                          edges, AST edges, neighborhoods, paths, and relation counts without \
                          allowing arbitrary Datalog execution. Use this when an AI agent needs \
                          precise graph facts before planning or editing Rust code."
                .into(),
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
                        "enum": ["all", "context", "module", "source_file", "symbol", "struct", "enum", "function", "method"],
                        "description": "Node kind filter for nodes/neighborhood views"
                    },
                    "relation": {
                        "type": "string",
                        "enum": ["all", "context_dep", "import_edge", "calls_symbol", "ast_edge", "resolved_call"],
                        "description": "Edge relation filter for edges/paths views"
                    },
                    "context": { "type": "string", "description": "Context/module filter" },
                    "module": { "type": "string", "description": "Alias for context/module filter" },
                    "file": { "type": "string", "description": "Source file filter" },
                    "symbol": { "type": "string", "description": "Rust symbol filter" },
                    "struct": { "type": "string", "description": "Alias for symbol when targeting a Rust struct" },
                    "from": { "type": "string", "description": "Source node for paths or edge filtering" },
                    "to": { "type": "string", "description": "Target node for paths or edge filtering" },
                    "limit": { "type": "integer", "description": "Max returned rows per collection (default: 50, max: 200)", "default": 50 }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "impact".into(),
            description: "Analyze downstream impact in the implemented Rust graph. Use module/context \
                          for architecture-level dependency analysis and symbol/struct for call graph \
                          analysis.\n\
                          Supports: transitive_deps, circular_deps, layer_violations, impact_analysis, \n\
                          aggregate_quality, dependency_graph, field_usage, method_search, shared_fields, \n\
                          pagerank, community_detection, betweenness_centrality, degree_centrality, \n\
                          topological_order, call_graph_callers, call_graph_callees, \n\
                          call_graph_reachability, call_graph_stats."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "analysis": {
                        "type": "string",
                        "enum": ["transitive_deps", "circular_deps", "layer_violations", "impact_analysis",
                                 "aggregate_quality", "dependency_graph", "field_usage", "method_search",
                                 "shared_fields", "pagerank", "community_detection", "betweenness_centrality",
                                 "degree_centrality", "topological_order",
                                 "call_graph_callers", "call_graph_callees", "call_graph_reachability", "call_graph_stats"],
                        "description": "The specific analysis to run"
                    },
                    "context": { "type": "string", "description": "Compatibility alias for module/context name (required for transitive_deps, impact_analysis)" },
                    "module": { "type": "string", "description": "Rust module name/path alias for context" },
                    "entity": { "type": "string", "description": "Compatibility alias for struct/entity name (required for impact_analysis)" },
                    "struct": { "type": "string", "description": "Rust struct name alias for entity" },
                    "symbol": { "type": "string", "description": "Symbol name (required for call_graph_callers, call_graph_callees, call_graph_reachability)" },
                    "field_type": { "type": "string", "description": "Field type to search (required for field_usage)" },
                    "method_name": { "type": "string", "description": "Method name to search (required for method_search)" }
                },
                "required": ["analysis"]
            }),
        },
        ToolDefinition {
            name: "safe_to_delete".into(),
            description: "Check whether a Rust symbol or struct can be safely deleted. Evaluates \
                          inbound call edges, imports, AST references, and any semantic overlay \
                          dependents. Context/module is optional; provide it to narrow overlay \
                          evidence, or omit it for a workspace-wide Rust symbol check."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "context": { "type": "string", "description": "Optional semantic context/module filter" },
                    "module": { "type": "string", "description": "Optional Rust module/context filter" },
                    "entity": { "type": "string", "description": "Compatibility entity name alias" },
                    "struct": { "type": "string", "description": "Rust struct name alias" },
                    "symbol": { "type": "string", "description": "Rust symbol name" }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "check".into(),
            description: "Check for architectural problems over the implemented Rust graph and semantic overlays: \
                          circular dependencies, layer violations, missing business rules on core structs, \
                          isolated modules, or policy violations. \
                          Run without parameters to check everything at once."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "check_name": {
                        "type": "string",
                        "enum": ["layer_violations", "circular_deps", "aggregate_quality", "orphan_contexts", "policy_violations", "drift"],
                        "description": "Specific check to run (default: runs all checks)"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "how_connected".into(),
            description: "Show how two Rust modules/components are connected. Returns proof paths \
                          over stored dependency facts when available."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "Source module or component" },
                    "to": { "type": "string", "description": "Target module or component" }
                },
                "required": ["from", "to"]
            }),
        },
        ToolDefinition {
            name: "why".into(),
            description: "Explain why something is flagged as a problem. Returns evidence-backed \
                          explanations with specific references and remediation suggestions."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "violation_type": {
                        "type": "string",
                        "enum": ["layer_violations", "circular_deps", "policy_violations", "aggregate_quality", "orphan_contexts"],
                        "description": "The type of problem to explain"
                    }
                },
                "required": ["violation_type"]
            }),
        },
        ToolDefinition {
            name: "drift".into(),
            description: "Compare the two most recent implemented architecture snapshots. Shows what was \
                          added or removed in the actual graph over time."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
            }),
        },
        ToolDefinition {
            name: "history".into(),
            description: "View architecture change history. Without timestamps, lists available \
                          snapshots. With timestamps, compares two points in time to show \
                          what changed between them."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "state": {
                        "type": "string",
                        "enum": ["actual", "implemented", "current", "planned"],
                        "description": "Which history stream to query (default: actual; planned/current are compatibility aliases)"
                    },
                    "ts_old": {
                        "type": "integer",
                        "description": "Older snapshot timestamp (microseconds). Required for comparison."
                    },
                    "ts_new": {
                        "type": "integer",
                        "description": "Newer snapshot timestamp (microseconds). Omit for latest."
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "search".into(),
            description: "Search the architecture by keyword. Finds matching Rust modules, structs, \
                          semantic labels, services/events overlays, and decisions across the codebase."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search keywords" },
                    "limit": { "type": "integer", "description": "Max results (default: 20)", "default": 20 }
                },
                "required": ["query"]
            }),
        },
    ];

    add_tool_alias(
        &mut tools,
        "architecture",
        "get_model",
        "Alias for architecture. Returns the implemented Rust ontology contract with health and temporal change status.",
    );
    add_tool_alias(
        &mut tools,
        "graph",
        "query_rust_graph",
        "Alias for graph. Returns bounded Rust graph database views over source files, symbols, modules, imports, calls, and dependencies.",
    );
    add_tool_alias(
        &mut tools,
        "impact",
        "query_blast_radius",
        "Alias for impact. Runs dependency, impact, graph, field, method, and call-graph analyses.",
    );
    add_tool_alias(
        &mut tools,
        "safe_to_delete",
        "can_delete_symbol",
        "Alias for safe_to_delete. Checks whether a symbol can be deleted and returns inbound-reference witnesses.",
    );
    add_tool_alias(
        &mut tools,
        "check",
        "check_architectural_invariant",
        "Alias for check. Evaluates named architectural invariants and returns proof evidence.",
    );
    add_tool_alias(
        &mut tools,
        "how_connected",
        "query_dependency_path",
        "Alias for how_connected. Returns proof paths between two Rust modules/components.",
    );
    add_tool_alias(
        &mut tools,
        "why",
        "explain_violation",
        "Alias for why. Explains architectural violations with evidence and limitations.",
    );
    add_tool_alias(
        &mut tools,
        "drift",
        "diff_models",
        "Alias for drift. Compares recent implemented architecture snapshots.",
    );
    add_tool_alias(
        &mut tools,
        "search",
        "search_architecture",
        "Alias for search. Runs full-text search over stored architecture facts.",
    );
    tools.push(ToolDefinition {
        name: "model_health".into(),
        description: "Compute a structured Datalog-derived health report over implemented Rust facts and semantic overlays: score, cycles, layer violations, aggregate quality, orphan modules/contexts, and graph analytics when available.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
    });

    tools
}

fn add_tool_alias(
    tools: &mut Vec<ToolDefinition>,
    source_name: &str,
    alias_name: &str,
    description: &str,
) {
    if let Some(source) = tools.iter().find(|tool| tool.name == source_name) {
        tools.push(ToolDefinition {
            name: alias_name.into(),
            description: description.into(),
            input_schema: source.input_schema.clone(),
        });
    }
}

/// Dispatches a tool call and returns the result.
pub fn call_tool(store: &Store, workspace_path: &str, name: &str, args: &Value) -> ToolCallResult {
    match name {
        "rust_status" => {
            let kernel = ReasoningKernel::new(store);
            match kernel.architecture(workspace_path) {
                Ok(mut claim) => {
                    if args["detail"].as_str().unwrap_or("summary") != "full" {
                        claim.payload = compact_architecture_status_payload(claim.payload);
                    }
                    stored_claim_result(store, workspace_path, &claim)
                }
                Err(e) => error_result(format!("rust_status failed: {e}")),
            }
        }

        "architecture" | "get_model" => {
            let kernel = ReasoningKernel::new(store);
            match kernel.architecture(workspace_path) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!("architecture failed: {e}")),
            }
        }

        "rust_graph" | "graph" | "query_rust_graph" => {
            match store.query_rust_graph(workspace_path, args) {
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
                    ],
                    Some(json!({"source": "rust_graph_query", "state": "actual"})),
                );
                    if let Some(object) = envelope.as_object_mut() {
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
            }
        }

        "rust_resolve" => {
            let crate_dir = std::path::Path::new(workspace_path);
            match crate::domain::rust_analyzer::resolve_calls(crate_dir) {
                Ok(calls) => match store.save_resolved_calls(workspace_path, &calls) {
                    Ok(count) => {
                        let sample: Vec<String> = calls
                            .iter()
                            .take(10)
                            .map(|c| format!("{} → {} [{}:{}]", c.caller, c.callee, c.callee_file, c.callee_line))
                            .collect();
                        json_result(json!({
                            "status": "ok",
                            "resolved_calls": count,
                            "sample": sample,
                            "note": "Query with rust_graph(view=edges, relation=resolved_call). \
                                     Each edge resolves a call site to the concrete function it targets.",
                        }))
                    }
                    Err(e) => error_result(format!("rust_resolve: failed to persist: {e}")),
                },
                Err(e) => error_result(format!(
                    "rust_resolve: rust-analyzer call resolution failed \
                     (needs the rust-analyzer component — `rustup component add rust-analyzer`): {e}"
                )),
            }
        }

        "rust_health" | "model_health" => match store.model_health(workspace_path) {
            Ok(health) => {
                let policy_gap_count = if health.policy_coverage.context_count == 0 {
                    0
                } else {
                    health.policy_coverage.missing_layer_assignments.len()
                        + usize::from(health.policy_coverage.dependency_constraint_count == 0)
                };
                json_result(with_reasoning_context(
                        json!({
                            "status": "ok",
                            "model_health": health,
                        }),
                        Some(json!({
                            "rule": "model health is computed from persisted architecture relations",
                            "derived_from": [
                                "context_dep",
                                "service",
                                "entity",
                                "invariant",
                                "event",
                                "context",
                                "layer_assignment",
                                "dependency_constraint"
                            ],
                            "witness_count": health.circular_deps.len()
                                + health.layer_violations.len()
                                + health.missing_invariants.len()
                                + health.orphan_contexts.len()
                                + policy_gap_count,
                        })),
                        Some(json!({
                            "score": health.score,
                            "circular_dependency_count": health.circular_deps.len(),
                            "layer_violation_count": health.layer_violations.len(),
                            "missing_invariant_count": health.missing_invariants.len(),
                            "orphan_context_count": health.orphan_contexts.len(),
                            "policy_gap_count": policy_gap_count,
                            "policy_coverage": health.policy_coverage,
                        })),
                        vec![
                            "Model health uses the latest persisted implemented architecture facts.".into(),
                            "Graph analytics are empty when the active Cozo runtime does not provide the required fixed rules.".into(),
                        ],
                        Some(json!({"source": "model_health", "state": "actual"})),
                    ))
            }
            Err(e) => error_result(format!("model_health failed: {e}")),
        },

        "rust_impact" | "impact" | "query_blast_radius" => {
            let kernel = ReasoningKernel::new(store);
            match kernel.impact(workspace_path, args) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!("rust_impact failed: {e}")),
            }
        }

        "rust_delete_safety" | "safe_to_delete" | "can_delete_symbol" => {
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

        "rust_invariants" | "check" | "check_architectural_invariant" => {
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

        "rust_path" | "how_connected" | "query_dependency_path" => {
            let from = args["from"]
                .as_str()
                .or_else(|| args["from_context"].as_str());
            let from = match from {
                Some(f) => f,
                None => return error_result("'from' parameter is required".into()),
            };
            let to = args["to"].as_str().or_else(|| args["to_context"].as_str());
            let to = match to {
                Some(t) => t,
                None => return error_result("'to' parameter is required".into()),
            };
            let relation = args["relation"].as_str().unwrap_or("context_dep");
            let kernel = ReasoningKernel::new(store);
            match kernel.how_connected_with_relation(workspace_path, relation, from, to) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!("rust_path failed: {e}")),
            }
        }

        "rust_explain" | "why" | "explain_violation" => {
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

        "rust_diff" | "drift" | "diff_models" => {
            let kernel = ReasoningKernel::new(store);
            match kernel.drift(workspace_path) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!("rust_diff failed: {e}")),
            }
        }

        "rust_history" | "history" => {
            let kernel = ReasoningKernel::new(store);
            match kernel.history(workspace_path, args) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!("history failed: {e}")),
            }
        }

        "rust_search" | "search" | "search_architecture" => {
            let query = match args["query"].as_str() {
                Some(q) => q,
                None => return error_result("'query' parameter is required".into()),
            };
            let limit = args["limit"].as_u64().unwrap_or(20) as usize;
            let kernel = ReasoningKernel::new(store);
            match kernel.search(workspace_path, query, limit) {
                Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                Err(e) => error_result(format!("rust_search failed: {e}")),
            }
        }

        _ => error_result(format!("Unknown tool: {}", name)),
    }
}

fn error_result(msg: String) -> ToolCallResult {
    error_tool_result(msg)
}

fn json_result(payload: Value) -> ToolCallResult {
    json_tool_result(payload)
}

fn graph_witness_count(payload: &Value) -> usize {
    payload["count"]
        .as_u64()
        .or_else(|| payload["rows"].as_array().map(|rows| rows.len() as u64))
        .or_else(|| payload["summary"]["node_count"].as_u64())
        .or_else(|| payload["summary"]["edge_count"].as_u64())
        .or_else(|| payload["summary"]["relation_count"].as_u64())
        .unwrap_or(0) as usize
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
        node_str(edge, "edge_type"),
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
        _ => store.load_desired(workspace),
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
        "complete_fact_relations": ["source_file", "symbol", "import_edge", "calls_symbol", "ast_edge"],
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
            "connectivity": "Use impact with dependency_graph, call_graph_callers, call_graph_callees, call_graph_reachability, or call_graph_stats.",
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
        assert_eq!(tools.len(), 12);
        let names: Vec<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
        for expected in [
            "rust_status",
            "rust_graph",
            "rust_resolve",
            "rust_health",
            "rust_impact",
            "rust_delete_safety",
            "rust_invariants",
            "rust_path",
            "rust_explain",
            "rust_diff",
            "rust_history",
            "rust_search",
        ] {
            assert!(names.contains(&expected));
        }
        for legacy in [
            "get_model",
            "query_rust_graph",
            "model_health",
            "query_blast_radius",
        ] {
            assert!(!names.contains(&legacy));
        }
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
        assert_eq!(parsed["health"]["score"], 94);
        assert_eq!(
            parsed["health"]["policy_coverage"]["dependency_constraint_count"],
            0
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
        };
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "graph",
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
                },
            )
            .unwrap();

        let result = call_tool(
            &store,
            &ws,
            "impact",
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
            "impact",
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
            "impact",
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

        let result = call_tool(
            &store,
            &ws,
            "impact",
            &json!({"analysis": "call_graph_stats"}),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        let derived_from = parsed["proof"]["derived_from"].as_array().unwrap();
        assert!(derived_from.iter().any(|item| item == "calls_symbol"));
        assert!(derived_from.iter().any(|item| item == "symbol"));
        assert_eq!(parsed["proof"]["witness_count"], 1);
        assert_eq!(parsed["result"]["project_callee_edges"], 1);
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
                },
            )
            .unwrap();

        let result = call_tool(
            &store,
            &ws,
            "safe_to_delete",
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
        };
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "safe_to_delete",
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
            "check",
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
            "check",
            &json!({
                "check_name": "nonexistent"
            }),
        );
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn test_how_connected_dispatch() {
        let store = test_store();
        let ws = format!("/tmp/test-deppath-{}", std::process::id());
        store
            .save_desired(
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
                },
            )
            .unwrap();

        let result = call_tool(
            &store,
            &ws,
            "how_connected",
            &json!({
                "from": "A",
                "to": "B"
            }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["reachable"], true);
        assert_eq!(parsed["claim_kind"], "how_connected");
        assert_eq!(parsed["provenance"]["state"], "actual");
        assert_eq!(parsed["proof"]["derived_from"][0], "context_dep");
        assert!(!parsed["limitations"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_how_connected_dispatch_uses_call_graph_relation() {
        let store = test_store();
        let ws = format!("/tmp/test-callpath-{}", std::process::id());
        let mut model = DomainModel::empty(&ws);
        model.call_edges = vec![CallEdge {
            caller: "Store::upsert_layer_assignment".into(),
            callee: "persist_policy".into(),
            file_path: "src/store/cozo.rs".into(),
            line: 4102,
            context: "store".into(),
        }];
        store.save_actual(&ws, &model).unwrap();

        let result = call_tool(
            &store,
            &ws,
            "rust_path",
            &json!({
                "relation": "calls_symbol",
                "from": "Store::upsert_layer_assignment",
                "to": "persist_policy"
            }),
        );
        assert_eq!(result.is_error, None);
        let ContentBlock::Text { text } = &result.content[0];
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["reachable"], true);
        assert_eq!(parsed["relation"], "calls_symbol");
        assert_eq!(
            parsed["paths"][0],
            json!(["Store::upsert_layer_assignment", "persist_policy"])
        );
        assert_eq!(parsed["provenance"]["source"], "call_graph_reachability");
        assert_eq!(parsed["proof"]["derived_from"][0], "calls_symbol");
        assert_eq!(parsed["proof"]["witness_count"], 1);

        let graph_result = call_tool(
            &store,
            &ws,
            "rust_graph",
            &json!({
                "view": "paths",
                "relation": "calls_symbol",
                "from": "Store::upsert_layer_assignment",
                "to": "persist_policy"
            }),
        );
        assert_eq!(graph_result.is_error, None);
        let ContentBlock::Text { text } = &graph_result.content[0];
        let graph: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(graph["reachable"], true);
        assert_eq!(
            graph["rows"][0][0],
            json!(["Store::upsert_layer_assignment", "persist_policy"])
        );
    }

    #[test]
    fn test_why_dispatch_includes_reasoning_context() {
        let store = test_store();
        let ws = format!("/tmp/test-why-{}", std::process::id());

        let result = call_tool(
            &store,
            &ws,
            "why",
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
                },
            )
            .unwrap();

        let overview = build_model_overview(&store, &ws, "actual");
        assert_eq!(overview["project"], "ActualOnly");
        assert_eq!(overview["bounded_contexts"].as_array().unwrap().len(), 1);
        assert_eq!(overview["bounded_contexts"][0]["name"], "Orders");
    }
}
