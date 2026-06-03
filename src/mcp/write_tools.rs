use serde_json::{Value, json};

use crate::domain::model::*;
use crate::domain::to_snake;
use crate::mcp::protocol::*;
use crate::reasoning::ReasoningKernel;
use crate::store::Store;
use crate::store::cozo::{PersistedReasoningClaim, ReasoningFactRef};

/// Returns the list of write tools the Axon server exposes.
pub fn list_write_tools() -> Vec<ToolDefinition> {
    rust_native_write_tools()
}

fn rust_native_write_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "rust_scan".into(),
            description: "Scan the workspace source code through the unified Rust fact pipeline and refresh the actual graph from implementation ground truth. Extracts modules, structs, functions, methods, imports, code references, syntactic calls, and compiler-resolved calls when rust-analyzer can resolve the workspace.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: "rust_annotations".into(),
            description: "Add, update, or remove semantic annotations layered on top of extracted Rust facts: module labels, ownership, policies, decisions, invariants, service roles, and rationale. Pass kind/action/name plus module/context when scoped; put kind-specific fields either top-level or under data. This does not mutate source-extracted Rust facts; use rust_scan to refresh implementation ground truth.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["bounded_context", "aggregate", "entity", "policy", "read_model", "service", "event", "value_object", "repository", "module", "external_system", "architectural_decision"],
                        "description": "Semantic annotation kind to upsert/remove"
                    },
                    "action": {
                        "type": "string",
                        "enum": ["upsert", "remove"],
                        "description": "Whether to upsert or remove the annotation (default: upsert)"
                    },
                    "module": { "type": "string", "description": "Rust module target for the annotation" },
                    "context": { "type": "string", "description": "Compatibility alias for module/context annotation target" },
                    "name": { "type": "string", "description": "Annotation name" },
                    "data": {
                        "type": "object",
                        "description": "Kind-specific fields such as description, module_path, ownership, dependencies, fields, methods, invariants, policy metadata, decision metadata, or aggregate links."
                    }
                },
                "additionalProperties": true,
                "required": ["kind", "name"]
            }),
        },
        ToolDefinition {
            name: "rust_diagnose".into(),
            description: "Diagnose the actual Rust architecture and report temporal graph changes. Actions: diagnose runs the full analysis pipeline, plan summarizes current changes, and accept/reset remain compatibility no-ops in actual-first mode.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["diagnose", "plan", "accept", "reset"],
                        "description": "Diagnostic action (default: plan)"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: "rust_constraints".into(),
            description: "Declare and evaluate constraints over Rust modules and optional semantic annotations: layer assignments, allowed or forbidden dependencies, list current constraints, or evaluate violations.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["assign_layer", "remove_layer", "add_constraint", "remove_constraint", "list", "evaluate"],
                        "description": "Constraint action to perform"
                    },
                    "module": { "type": "string", "description": "Rust module or semantic context target" },
                    "context": { "type": "string", "description": "Compatibility alias for module" },
                    "layer": { "type": "string", "description": "Layer name, e.g. domain, application, infrastructure" },
                    "constraint_kind": {
                        "type": "string",
                        "enum": ["layer", "context"],
                        "description": "Whether the constraint applies to layers or module/context names"
                    },
                    "source": { "type": "string", "description": "Source layer or module/context name" },
                    "target": { "type": "string", "description": "Target layer or module/context name" },
                    "rule": {
                        "type": "string",
                        "enum": ["forbidden", "allowed"],
                        "description": "Whether the dependency is forbidden or explicitly allowed (default: forbidden)"
                    }
                },
                "required": ["action"]
            }),
        },
    ]
}

pub fn call_write_tool(
    workspace_path: &str,
    store: &Store,
    name: &str,
    args: &Value,
) -> ToolCallResult {
    dispatch_write_tool(workspace_path, store, name, args)
}

fn dispatch_write_tool(
    workspace_path: &str,
    store: &Store,
    name: &str,
    args: &Value,
) -> ToolCallResult {
    // Only the canonical rust-native write tools are accepted.
    if !matches!(
        name,
        "rust_scan" | "rust_annotations" | "rust_diagnose" | "rust_constraints"
    ) {
        return error_result(format!("Unknown write tool: {name}"));
    }
    let canonical_name = canonical_write_tool_name(name);
    let mut normalized_args = args.clone();
    let mut use_normalized_args = false;
    if matches!(name, "rust_annotations" | "rust_constraints") {
        normalize_rust_native_write_args(&mut normalized_args);
        use_normalized_args = true;
    }
    let args = if use_normalized_args {
        &normalized_args
    } else {
        args
    };

    match canonical_name {
        "define" => {
            let kind = arg_str(args, "kind");
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("upsert");

            let result = match (kind.as_str(), action) {
                ("bounded_context", "upsert") => {
                    upsert_bounded_context(store, workspace_path, args)
                }
                ("bounded_context", "remove") => {
                    remove_bounded_context(store, workspace_path, args)
                }
                ("entity", "upsert") => upsert_entity(store, workspace_path, args),
                ("entity", "remove") => remove_entity(store, workspace_path, args),
                ("service", "upsert") => upsert_service(store, workspace_path, args),
                ("service", "remove") => remove_service(store, workspace_path, args),
                ("event", "upsert") => upsert_event(store, workspace_path, args),
                ("event", "remove") => remove_event(store, workspace_path, args),
                ("value_object", "upsert") => upsert_value_object(store, workspace_path, args),
                ("value_object", "remove") => remove_value_object(store, workspace_path, args),
                ("repository", "upsert") => upsert_repository(store, workspace_path, args),
                ("repository", "remove") => remove_repository(store, workspace_path, args),
                ("aggregate", "upsert") => upsert_aggregate(store, workspace_path, args),
                ("aggregate", "remove") => remove_aggregate(store, workspace_path, args),
                ("policy", "upsert") => upsert_policy(store, workspace_path, args),
                ("policy", "remove") => remove_policy(store, workspace_path, args),
                ("read_model", "upsert") => upsert_read_model(store, workspace_path, args),
                ("read_model", "remove") => remove_read_model(store, workspace_path, args),
                ("external_system", "upsert") => {
                    upsert_external_system(store, workspace_path, args)
                }
                ("external_system", "remove") => {
                    remove_external_system(store, workspace_path, args)
                }
                ("architectural_decision", "upsert") => {
                    upsert_architectural_decision(store, workspace_path, args)
                }
                ("architectural_decision", "remove") => {
                    remove_architectural_decision(store, workspace_path, args)
                }
                ("module", "upsert") => upsert_module(store, workspace_path, args),
                ("module", "remove") => remove_module_handler(store, workspace_path, args),
                ("", _) => error_result("'kind' is required"),
                (_, action) => error_result(format!("Unknown action '{action}' for kind '{kind}'")),
            };

            if result.is_error.unwrap_or(false) {
                result
            } else {
                if let Err(e) = store.record_actual_snapshot(workspace_path) {
                    return error_result(format!(
                        "Model mutation succeeded but snapshot recording failed: {e}"
                    ));
                }
                let fact_refs = reasoning_fact_refs_for_define(args);
                let invalidate_result = if fact_refs.is_empty() {
                    invalidate_and_refresh_dependency(store, workspace_path, "actual")
                } else {
                    invalidate_and_refresh_facts(store, workspace_path, &fact_refs)
                };

                match invalidate_result {
                    Ok(()) => result,
                    Err(e) => error_result(format!("Model mutation succeeded but {e}")),
                }
            }
        }

        "sync" => {
            use crate::domain::analyze::{SemanticResolution, scan_actual_graph};

            let workspace_root = std::path::Path::new(workspace_path);
            let previous = store.load_actual(workspace_path).ok().flatten();

            match scan_actual_graph(workspace_root, previous.as_ref()) {
                Ok(scan) => {
                    let actual = &scan.model;
                    let entity_count: usize = actual
                        .bounded_contexts
                        .iter()
                        .map(|bc| bc.entities.len())
                        .sum();
                    let vo_count: usize = actual
                        .bounded_contexts
                        .iter()
                        .map(|bc| bc.value_objects.len())
                        .sum();
                    let svc_count: usize = actual
                        .bounded_contexts
                        .iter()
                        .map(|bc| bc.services.len())
                        .sum();
                    let repo_count: usize = actual
                        .bounded_contexts
                        .iter()
                        .map(|bc| bc.repositories.len())
                        .sum();
                    let event_count: usize = actual
                        .bounded_contexts
                        .iter()
                        .map(|bc| bc.events.len())
                        .sum();
                    let mut counts = SyncCounts {
                        contexts_scanned: actual.bounded_contexts.len(),
                        entities: entity_count,
                        value_objects: vo_count,
                        services: svc_count,
                        repositories: repo_count,
                        events: event_count,
                        source_files: actual.source_files.len(),
                        symbols: actual.symbols.len(),
                        import_edges: actual.import_edges.len(),
                        persisted_import_edges: None,
                        reference_edges: actual.reference_edges.len(),
                        persisted_reference_edges: None,
                        call_edges: actual.call_edges.len(),
                        persisted_call_edges: None,
                        resolved_call_edges: scan.resolved_calls.len(),
                        persisted_resolved_call_edges: None,
                        semantic_resolution_succeeded: matches!(
                            scan.semantic_resolution,
                            SemanticResolution::Resolved
                        ),
                    };

                    match store.save_actual_scan_and_compute_drift(workspace_path, &scan) {
                        Ok(drift_count) => {
                            if let Ok(relation_counts) =
                                store.rust_graph_relation_counts(workspace_path)
                            {
                                counts.persisted_import_edges =
                                    relation_counts.get("import_edge").copied();
                                counts.persisted_reference_edges =
                                    relation_counts.get("reference_edge").copied();
                                counts.persisted_call_edges =
                                    relation_counts.get("calls_symbol").copied();
                                counts.persisted_resolved_call_edges =
                                    relation_counts.get("resolved_call").copied();
                            }
                            let had_previous = previous.is_some();
                            let semantic_resolution_error = if let SemanticResolution::Failed {
                                error,
                            } = &scan.semantic_resolution
                            {
                                Some(format!(
                                    "rust-analyzer semantic resolution failed; resolved_call edges cleared: {error}"
                                ))
                            } else {
                                None
                            };
                            let mut follow_on_failures = Vec::new();

                            if let Err(e) = store.reload_persisted_policy(workspace_path) {
                                follow_on_failures
                                    .push(format!("policy reload after scan failed: {e}"));
                            }

                            let drift_entry_count = Some(drift_count);

                            let refresh_states = vec!["actual", "drift"];
                            if let Err(e) =
                                eager_refresh_dependencies(store, workspace_path, &refresh_states)
                            {
                                follow_on_failures.push(e);
                            }

                            let mut payload = build_sync_report(
                                counts,
                                had_previous,
                                drift_entry_count,
                                &follow_on_failures,
                            );
                            if let Some(error) = &semantic_resolution_error {
                                payload["semantic_resolution_error"] = json!(error);
                            }
                            crate::mcp::tools::attach_graph_confidence(
                                &mut payload,
                                store,
                                workspace_path,
                            );
                            let proof = json!({
                                "rule": "sync succeeds when scan extraction, actual persistence, and drift recomputation complete; semantic enrichment is recorded separately when unavailable",
                                "derived_from": ["scan_actual_graph", "save_actual_scan", "compute_drift", "resolve_calls"],
                                "witness_count": counts.contexts_scanned,
                            });
                            let evidence = json!({
                                "counts": {
                                    "contexts_scanned": counts.contexts_scanned,
                                    "entities": counts.entities,
                                    "value_objects": counts.value_objects,
                                    "services": counts.services,
                                    "repositories": counts.repositories,
                                    "events": counts.events,
                                    "source_files": counts.source_files,
                                    "symbols": counts.symbols,
                                    "import_edges": counts.persisted_import_edges.unwrap_or(counts.import_edges),
                                    "extracted_import_edges": counts.import_edges,
                                    "persisted_import_edges": counts.persisted_import_edges,
                                    "reference_edges": counts.persisted_reference_edges.unwrap_or(counts.reference_edges),
                                    "extracted_reference_edges": counts.reference_edges,
                                    "persisted_reference_edges": counts.persisted_reference_edges,
                                    "call_edges": counts.persisted_call_edges.unwrap_or(counts.call_edges),
                                    "extracted_call_edges": counts.call_edges,
                                    "persisted_call_edges": counts.persisted_call_edges,
                                    "resolved_call_edges": counts.persisted_resolved_call_edges.unwrap_or(counts.resolved_call_edges),
                                    "extracted_resolved_call_edges": counts.resolved_call_edges,
                                    "persisted_resolved_call_edges": counts.persisted_resolved_call_edges,
                                    "semantic_resolution": if counts.semantic_resolution_succeeded { "resolved" } else { "failed" },
                                },
                                "follow_on_failures": follow_on_failures,
                                "semantic_resolution_error": semantic_resolution_error,
                            });
                            let limitations = sync_limitations(
                                drift_entry_count.is_none(),
                                !counts.semantic_resolution_succeeded,
                            );

                            if payload["status"] == "partial_failure" {
                                reasoning_error_result(
                                    store,
                                    workspace_path,
                                    payload,
                                    Some(proof),
                                    Some(evidence),
                                    limitations,
                                    json!({"source": "scan_pipeline", "state": "actual"}),
                                )
                            } else {
                                reasoning_result(
                                    store,
                                    workspace_path,
                                    payload,
                                    Some(proof),
                                    Some(evidence),
                                    limitations,
                                    json!({"source": "scan_pipeline", "state": "actual"}),
                                )
                            }
                        }
                        Err(e) => {
                            error_result(format!("Scan succeeded but save/drift failed: {e}"))
                        }
                    }
                }
                Err(e) => error_result(format!("Scan failed: {e}")),
            }
        }

        "refactor" => {
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("plan");

            match action {
                "diagnose" => diagnose_pipeline(store, workspace_path),

                "plan" => {
                    let kernel = ReasoningKernel::new(store);
                    match kernel.refactor_plan(workspace_path) {
                        Ok(claim) => stored_claim_result(store, workspace_path, &claim),
                        Err(e) => error_result(format!("Refactor plan failed: {e}")),
                    }
                }

                "accept" => match store.accept(workspace_path) {
                    Ok(()) => {
                        let mut follow_on_failures = Vec::new();
                        let drift_entry_count = match store.compute_drift(workspace_path) {
                            Ok(count) => Some(count),
                            Err(e) => {
                                follow_on_failures.push(format!("drift recomputation failed: {e}"));
                                None
                            }
                        };
                        let mut refresh_states = vec!["actual"];
                        if drift_entry_count.is_some() {
                            refresh_states.push("drift");
                        }
                        if let Err(e) =
                            eager_refresh_dependencies(store, workspace_path, &refresh_states)
                        {
                            follow_on_failures.push(e);
                        }
                        let payload = json!({
                            "status": if follow_on_failures.is_empty() { "actual_first_noop" } else { "partial_failure" },
                            "message": if follow_on_failures.is_empty() {
                                "Actual-first mode has no planned graph to promote; implemented graph left unchanged."
                            } else {
                                "Actual-first accept compatibility path completed, but follow-on synchronization is incomplete."
                            },
                            "actual_promoted_from_desired": false,
                            "drift_recomputed": drift_entry_count.is_some(),
                            "drift_entry_count": drift_entry_count,
                            "follow_on_failures": follow_on_failures,
                        });
                        let proof = json!({
                            "rule": "accept is a compatibility no-op because the stored graph is already actual-first",
                            "derived_from": ["accept", "compute_drift"],
                            "witness_count": drift_entry_count.unwrap_or(0),
                        });
                        let evidence = json!({
                            "actual_promoted_from_desired": false,
                            "drift_entry_count": drift_entry_count,
                        });
                        let limitations = vec![
                                "Accept is retained for compatibility; run sync to update the implemented graph from source.".into(),
                                "Dynamic runtime behavior and unmodeled code paths remain outside the stored architecture graph.".into(),
                                if drift_entry_count.is_none() {
                                    "Temporal change entries may be stale until drift recomputation succeeds.".into()
                                } else {
                                    String::new()
                                },
                            ]
                            .into_iter()
                            .filter(|item| !item.is_empty())
                            .collect::<Vec<_>>();

                        if payload["status"] == "partial_failure" {
                            reasoning_error_result(
                                store,
                                workspace_path,
                                payload,
                                Some(proof),
                                Some(evidence),
                                limitations,
                                json!({"source": "refactor_lifecycle", "state": "actual"}),
                            )
                        } else {
                            reasoning_result(
                                store,
                                workspace_path,
                                payload,
                                Some(proof),
                                Some(evidence),
                                limitations,
                                json!({"source": "refactor_lifecycle", "state": "actual"}),
                            )
                        }
                    }
                    Err(e) => error_result(format!("Failed to accept: {e}")),
                },

                "reset" => match store.reset(workspace_path) {
                    Ok(Some(_)) => {
                        let mut follow_on_failures = Vec::new();
                        let drift_entry_count = match store.compute_drift(workspace_path) {
                            Ok(count) => Some(count),
                            Err(e) => {
                                follow_on_failures.push(format!("drift recomputation failed: {e}"));
                                None
                            }
                        };
                        let mut refresh_states = vec!["actual"];
                        if drift_entry_count.is_some() {
                            refresh_states.push("drift");
                        }
                        if let Err(e) =
                            eager_refresh_dependencies(store, workspace_path, &refresh_states)
                        {
                            follow_on_failures.push(e);
                        }
                        let payload = json!({
                            "status": if follow_on_failures.is_empty() { "actual_first_noop" } else { "partial_failure" },
                            "message": if follow_on_failures.is_empty() {
                                "Actual-first mode has no planned graph to reset; implemented graph left unchanged."
                            } else {
                                "Actual-first reset compatibility path completed, but follow-on synchronization is incomplete."
                            },
                            "desired_reset_from_actual": false,
                            "drift_recomputed": drift_entry_count.is_some(),
                            "drift_entry_count": drift_entry_count,
                            "follow_on_failures": follow_on_failures,
                        });
                        let proof = json!({
                            "rule": "reset is a compatibility no-op because the stored graph is already actual-first",
                            "derived_from": ["reset", "compute_drift"],
                            "witness_count": drift_entry_count.unwrap_or(0),
                        });
                        let evidence = json!({
                            "desired_reset_from_actual": false,
                            "drift_entry_count": drift_entry_count,
                        });
                        let limitations = vec![
                                "Reset is retained for compatibility; temporal history remains available through the history tool.".into(),
                                "Runtime-only behavior and unmodeled dependencies remain outside the stored architecture graph.".into(),
                                if drift_entry_count.is_none() {
                                    "Temporal change entries may be stale until drift recomputation succeeds.".into()
                                } else {
                                    String::new()
                                },
                            ]
                            .into_iter()
                            .filter(|item| !item.is_empty())
                            .collect::<Vec<_>>();

                        if payload["status"] == "partial_failure" {
                            reasoning_error_result(
                                store,
                                workspace_path,
                                payload,
                                Some(proof),
                                Some(evidence),
                                limitations,
                                json!({"source": "refactor_lifecycle", "state": "actual"}),
                            )
                        } else {
                            reasoning_result(
                                store,
                                workspace_path,
                                payload,
                                Some(proof),
                                Some(evidence),
                                limitations,
                                json!({"source": "refactor_lifecycle", "state": "actual"}),
                            )
                        }
                    }
                    Ok(None) => reasoning_error_result(
                        store,
                        workspace_path,
                        json!({
                            "status": "reset_unavailable",
                            "message": "No implemented architecture graph is available."
                        }),
                        Some(json!({
                            "rule": "reset requires an existing implemented graph",
                            "derived_from": ["load_actual"],
                            "witness_count": 0,
                        })),
                        Some(json!({
                            "has_actual_model": false,
                        })),
                        vec!["Run sync before requesting reset compatibility behavior.".into()],
                        json!({"source": "refactor_lifecycle", "state": "actual"}),
                    ),
                    Err(e) => error_result(format!("Failed to reset: {e}")),
                },

                _ => error_result(format!(
                    "Unknown action '{action}'. Use 'diagnose', 'plan', 'accept', or 'reset'."
                )),
            }
        }

        "constrain" => {
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("list");

            match action {
                "assign_layer" => {
                    let context = arg_str(args, "context");
                    let layer = arg_str(args, "layer");
                    if context.is_empty() {
                        return error_result("'context' is required for assign_layer");
                    }
                    if layer.is_empty() {
                        return error_result("'layer' is required for assign_layer");
                    }
                    match store.upsert_layer_assignment(workspace_path, &context, &layer) {
                        Ok(()) => match invalidate_and_refresh_policy(store, workspace_path) {
                            Ok(()) => text_result(json!({
                                "message": format!("Assigned context '{}' to layer '{}'", context, layer),
                            }).to_string()),
                            Err(e) => error_result(format!(
                                "Layer assignment succeeded but {e}"
                            )),
                        },
                        Err(e) => error_result(format!("Failed to assign layer: {e}")),
                    }
                }

                "remove_layer" => {
                    let context = arg_str(args, "context");
                    if context.is_empty() {
                        return error_result("'context' is required for remove_layer");
                    }
                    match store.remove_layer_assignment(workspace_path, &context) {
                        Ok(true) => match invalidate_and_refresh_policy(store, workspace_path) {
                            Ok(()) => text_result(format!(
                                "Removed layer assignment for context '{context}'"
                            )),
                            Err(e) => error_result(format!("Layer removal succeeded but {e}")),
                        },
                        Ok(false) => error_result(format!(
                            "No layer assignment found for context '{context}'"
                        )),
                        Err(e) => error_result(format!("Failed to remove layer assignment: {e}")),
                    }
                }

                "add_constraint" => {
                    let constraint_kind = arg_str(args, "constraint_kind");
                    let source = arg_str(args, "source");
                    let target = arg_str(args, "target");
                    let rule = args
                        .get("rule")
                        .and_then(|v| v.as_str())
                        .unwrap_or("forbidden");
                    if constraint_kind.is_empty() || source.is_empty() || target.is_empty() {
                        return error_result(
                            "'constraint_kind', 'source', and 'target' are required for add_constraint",
                        );
                    }
                    if constraint_kind != "layer" && constraint_kind != "context" {
                        return error_result("'constraint_kind' must be 'layer' or 'context'");
                    }
                    if rule != "forbidden" && rule != "allowed" {
                        return error_result("'rule' must be 'forbidden' or 'allowed'");
                    }
                    match store.upsert_dependency_constraint(workspace_path, &constraint_kind, &source, &target, rule) {
                        Ok(()) => match invalidate_and_refresh_policy(store, workspace_path) {
                            Ok(()) => text_result(json!({
                                "message": format!("{} dependency: {} → {} ({})", rule, source, target, constraint_kind),
                            }).to_string()),
                            Err(e) => error_result(format!(
                                "Constraint update succeeded but {e}"
                            )),
                        },
                        Err(e) => error_result(format!("Failed to add constraint: {e}")),
                    }
                }

                "remove_constraint" => {
                    let constraint_kind = arg_str(args, "constraint_kind");
                    let source = arg_str(args, "source");
                    let target = arg_str(args, "target");
                    if constraint_kind.is_empty() || source.is_empty() || target.is_empty() {
                        return error_result(
                            "'constraint_kind', 'source', and 'target' are required for remove_constraint",
                        );
                    }
                    match store.remove_dependency_constraint(
                        workspace_path,
                        &constraint_kind,
                        &source,
                        &target,
                    ) {
                        Ok(true) => match invalidate_and_refresh_policy(store, workspace_path) {
                            Ok(()) => text_result(format!(
                                "Removed {} constraint {} -> {}",
                                constraint_kind, source, target
                            )),
                            Err(e) => error_result(format!("Constraint removal succeeded but {e}")),
                        },
                        Ok(false) => error_result(format!(
                            "No {} constraint found for {} -> {}",
                            constraint_kind, source, target
                        )),
                        Err(e) => error_result(format!("Failed to remove constraint: {e}")),
                    }
                }

                "list" => {
                    if let Err(e) = store.reload_persisted_policy(workspace_path) {
                        return error_result(format!("Policy reload failed: {e}"));
                    }
                    let layers = store
                        .list_layer_assignments(workspace_path)
                        .unwrap_or_default();
                    let constraints = store
                        .list_dependency_constraints(workspace_path)
                        .unwrap_or_default();

                    let layer_items: Vec<Value> = layers
                        .iter()
                        .map(|(ctx, layer)| json!({"context": ctx, "layer": layer}))
                        .collect();
                    let constraint_items: Vec<Value> = constraints
                        .iter()
                        .map(|(kind, src, tgt, rule)| {
                            json!({
                                "constraint_kind": kind,
                                "source": src,
                                "target": tgt,
                                "rule": rule,
                            })
                        })
                        .collect();

                    text_result(
                        json!({
                            "layer_assignments": layer_items,
                            "dependency_constraints": constraint_items,
                        })
                        .to_string(),
                    )
                }

                "evaluate" => match store.evaluate_policy_violations(workspace_path) {
                    Ok(result) => text_result(result.to_string()),
                    Err(e) => error_result(format!("Policy evaluation failed: {e}")),
                },

                _ => error_result(format!(
                    "Unknown action '{action}'. Use 'assign_layer', 'remove_layer', 'add_constraint', 'remove_constraint', 'list', or 'evaluate'."
                )),
            }
        }

        _ => error_result(format!("Unknown write tool: {name}")),
    }
}

/// Map a canonical rust-native write tool to its internal handler key.
fn canonical_write_tool_name(name: &str) -> &str {
    match name {
        "rust_annotations" => "define",
        "rust_scan" => "sync",
        "rust_diagnose" => "refactor",
        "rust_constraints" => "constrain",
        other => other,
    }
}

fn normalize_rust_native_write_args(args: &mut Value) {
    let Some(object) = args.as_object_mut() else {
        return;
    };
    if let Some(data) = object.remove("data") {
        if let Value::Object(data_object) = data {
            for (key, value) in data_object {
                object.entry(key).or_insert(value);
            }
        } else {
            object.insert("data".into(), data);
        }
    }
    if object.contains_key("context") {
        return;
    }
    if let Some(module) = object.get("module").cloned() {
        object.insert("context".into(), module);
    }
}

// ─── Kind handlers ─────────────────────────────────────────────────────────

fn upsert_bounded_context(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "name");
    if ctx_name.is_empty() {
        return error_result("'name' is required");
    }
    let existing = store.load_desired(workspace_path).ok().flatten();
    let current = existing.as_ref().and_then(|m| {
        m.bounded_contexts
            .iter()
            .find(|bc| bc.name.eq_ignore_ascii_case(&ctx_name))
    });
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| current.map(|bc| bc.description.clone()))
        .unwrap_or_default();
    let module_path = args
        .get("module_path")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| current.map(|bc| bc.module_path.clone()))
        .unwrap_or_default();
    let ownership = if args.get("ownership").is_some() {
        parse_ownership(args.get("ownership"))
    } else {
        current.map(|bc| bc.ownership.clone()).unwrap_or_default()
    };
    let dependencies = args
        .get("dependencies")
        .map(|v| parse_string_array(Some(v)))
        .or_else(|| current.map(|bc| bc.dependencies.clone()))
        .unwrap_or_default();
    if let Err(e) = store.upsert_context(
        workspace_path,
        &ctx_name,
        &description,
        &module_path,
        &dependencies,
        &ownership,
    ) {
        return error_result(format!("Failed to upsert bounded context: {e}"));
    }
    let action = if current.is_some() {
        "updated"
    } else {
        "created"
    };

    let all_ctx_names: Vec<String> = store
        .load_desired(workspace_path)
        .ok()
        .flatten()
        .map(|m| m.bounded_contexts.into_iter().map(|c| c.name).collect())
        .unwrap_or_default();
    let unknown_deps: Vec<&str> = dependencies
        .iter()
        .filter(|d| !all_ctx_names.iter().any(|c| c.eq_ignore_ascii_case(d)))
        .map(|d| d.as_str())
        .collect();

    let mut result = json!({
        "message": format!("{} bounded context '{}'", if action == "created" { "Created" } else { "Updated" }, ctx_name),
    });

    if !unknown_deps.is_empty() {
        result["dependency_warnings"] = json!(
            unknown_deps
                .iter()
                .map(|d| format!("Dependency '{}' references an undefined bounded context", d))
                .collect::<Vec<_>>()
        );
    }

    text_result(result.to_string())
}

fn remove_bounded_context(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "name");
    match store.remove_context(workspace_path, &ctx_name) {
        Ok(true) => text_result(format!("Removed bounded context '{ctx_name}'")),
        Ok(false) => error_result(format!("Bounded context '{ctx_name}' not found")),
        Err(e) => error_result(format!("Failed to remove bounded context: {e}")),
    }
}

fn upsert_entity(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let entity_name = arg_str(args, "name");
    let existing = match require_context(store, workspace_path, &ctx_name) {
        Ok(()) => store.query_entity(workspace_path, &ctx_name, &entity_name),
        Err(result) => return result,
    };
    let mut entity = existing.clone().unwrap_or(Entity {
        name: entity_name.clone(),
        description: String::new(),
        aggregate_root: false,
        fields: vec![],
        methods: vec![],
        invariants: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    });
    if let Some(desc) = args.get("description").and_then(|v| v.as_str()) {
        entity.description = desc.to_string();
    }
    if let Some(agg) = args.get("aggregate_root").and_then(|v| v.as_bool()) {
        entity.aggregate_root = agg;
    }
    if let Some(fields) = args.get("fields").and_then(|v| v.as_array()) {
        merge_fields(&mut entity.fields, fields);
    }
    if let Some(methods) = args.get("methods").and_then(|v| v.as_array()) {
        merge_methods(&mut entity.methods, methods);
    }
    if let Some(invariants) = args.get("invariants").and_then(|v| v.as_array()) {
        for inv in invariants {
            if let Some(s) = inv.as_str()
                && !entity.invariants.iter().any(|i| i == s)
            {
                entity.invariants.push(s.to_string());
            }
        }
    }
    if let Err(e) = store.upsert_entity(workspace_path, &ctx_name, &entity) {
        return error_result(format!("Failed to upsert entity: {e}"));
    }
    text_result(json!({
        "message": format!("{} entity '{}' in '{}'", if existing.is_some() { "Updated" } else { "Created" }, entity_name, ctx_name),
        "suggested_path": suggested_path_for(store, workspace_path, &ctx_name, "entity", &entity_name)
    }).to_string())
}

fn remove_entity(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let entity_name = arg_str(args, "name");
    match store.remove_entity(workspace_path, &ctx_name, &entity_name) {
        Ok(true) => text_result(format!("Removed entity '{entity_name}' from '{ctx_name}'")),
        Ok(false) => error_result(format!("Entity '{entity_name}' not found in '{ctx_name}'")),
        Err(e) => error_result(format!("Failed to remove entity: {e}")),
    }
}

fn upsert_service(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let svc_name = arg_str(args, "name");
    let existing = match require_context(store, workspace_path, &ctx_name) {
        Ok(()) => store.query_service(workspace_path, &ctx_name, &svc_name),
        Err(result) => return result,
    };
    let mut service = existing.clone().unwrap_or(Service {
        name: svc_name.clone(),
        description: String::new(),
        kind: ServiceKind::Domain,
        methods: vec![],
        dependencies: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    });
    if let Some(desc) = args.get("description").and_then(|v| v.as_str()) {
        service.description = desc.to_string();
    }
    if args.get("service_kind").is_some() {
        service.kind = parse_service_kind(&arg_str(args, "service_kind"));
    }
    if let Some(deps) = args.get("dependencies").and_then(|v| v.as_array()) {
        service.dependencies = deps
            .iter()
            .filter_map(|d| d.as_str().map(String::from))
            .collect();
    }
    if let Some(methods) = args.get("methods").and_then(|v| v.as_array()) {
        merge_methods(&mut service.methods, methods);
    }
    if let Err(e) = store.upsert_service(workspace_path, &ctx_name, &service) {
        return error_result(format!("Failed to upsert service: {e}"));
    }
    text_result(json!({
        "message": format!("{} service '{}' in '{}'", if existing.is_some() { "Updated" } else { "Created" }, svc_name, ctx_name),
        "suggested_path": suggested_path_for(store, workspace_path, &ctx_name, "service", &svc_name)
    }).to_string())
}

fn remove_service(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let svc_name = arg_str(args, "name");
    match store.remove_service(workspace_path, &ctx_name, &svc_name) {
        Ok(true) => text_result(format!("Removed service '{svc_name}' from '{ctx_name}'")),
        Ok(false) => error_result(format!("Service '{svc_name}' not found in '{ctx_name}'")),
        Err(e) => error_result(format!("Failed to remove service: {e}")),
    }
}

fn upsert_event(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let event_name = arg_str(args, "name");

    let existing = match require_context(store, workspace_path, &ctx_name) {
        Ok(()) => store.query_event(workspace_path, &ctx_name, &event_name),
        Err(result) => return result,
    };
    let mut event = existing.clone().unwrap_or(DomainEvent {
        name: event_name.clone(),
        description: String::new(),
        fields: vec![],
        source: String::new(),
        file_path: None,
        start_line: None,
        end_line: None,
    });
    if let Some(desc) = args.get("description").and_then(|v| v.as_str()) {
        event.description = desc.to_string();
    }
    if let Some(src) = args.get("source").and_then(|v| v.as_str()) {
        event.source = src.to_string();
    }
    if let Some(fields) = args.get("fields").and_then(|v| v.as_array()) {
        merge_fields(&mut event.fields, fields);
    }
    if let Err(e) = store.upsert_event(workspace_path, &ctx_name, &event) {
        return error_result(format!("Failed to upsert event: {e}"));
    }
    text_result(json!({
        "message": format!("{} event '{}' in '{}'", if existing.is_some() { "Updated" } else { "Created" }, event_name, ctx_name),
        "suggested_path": suggested_path_for(store, workspace_path, &ctx_name, "event", &event_name)
    }).to_string())
}

fn remove_event(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let event_name = arg_str(args, "name");
    match store.remove_event(workspace_path, &ctx_name, &event_name) {
        Ok(true) => text_result(format!("Removed event '{event_name}' from '{ctx_name}'")),
        Ok(false) => error_result(format!("Event '{event_name}' not found in '{ctx_name}'")),
        Err(e) => error_result(format!("Failed to remove event: {e}")),
    }
}

fn upsert_value_object(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let vo_name = arg_str(args, "name");

    let existing = match require_context(store, workspace_path, &ctx_name) {
        Ok(()) => store.query_value_object(workspace_path, &ctx_name, &vo_name),
        Err(result) => return result,
    };
    let mut value_object = existing.clone().unwrap_or(ValueObject {
        name: vo_name.clone(),
        description: String::new(),
        fields: vec![],
        validation_rules: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    });
    if let Some(desc) = args.get("description").and_then(|v| v.as_str()) {
        value_object.description = desc.to_string();
    }
    if let Some(fields) = args.get("fields").and_then(|v| v.as_array()) {
        merge_fields(&mut value_object.fields, fields);
    }
    if let Some(rules) = args.get("validation_rules").and_then(|v| v.as_array()) {
        for rule in rules {
            if let Some(s) = rule.as_str()
                && !value_object.validation_rules.iter().any(|r| r == s)
            {
                value_object.validation_rules.push(s.to_string());
            }
        }
    }
    if let Err(e) = store.upsert_value_object(workspace_path, &ctx_name, &value_object) {
        return error_result(format!("Failed to upsert value object: {e}"));
    }
    text_result(json!({
        "message": format!("{} value object '{}' in '{}'", if existing.is_some() { "Updated" } else { "Created" }, vo_name, ctx_name),
        "suggested_path": suggested_path_for(store, workspace_path, &ctx_name, "value_object", &vo_name)
    }).to_string())
}

fn remove_value_object(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let vo_name = arg_str(args, "name");
    match store.remove_value_object(workspace_path, &ctx_name, &vo_name) {
        Ok(true) => text_result(format!(
            "Removed value object '{vo_name}' from '{ctx_name}'"
        )),
        Ok(false) => error_result(format!(
            "Value object '{vo_name}' not found in '{ctx_name}'"
        )),
        Err(e) => error_result(format!("Failed to remove value object: {e}")),
    }
}

fn upsert_repository(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let repo_name = arg_str(args, "name");

    let existing = match require_context(store, workspace_path, &ctx_name) {
        Ok(()) => store.query_repository(workspace_path, &ctx_name, &repo_name),
        Err(result) => return result,
    };
    let mut repository = existing.clone().unwrap_or(Repository {
        name: repo_name.clone(),
        aggregate: String::new(),
        methods: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    });
    if let Some(agg) = args.get("aggregate").and_then(|v| v.as_str()) {
        repository.aggregate = agg.to_string();
    }
    if let Some(methods) = args.get("methods").and_then(|v| v.as_array()) {
        merge_methods(&mut repository.methods, methods);
    }
    if let Err(e) = store.upsert_repository(workspace_path, &ctx_name, &repository) {
        return error_result(format!("Failed to upsert repository: {e}"));
    }
    text_result(json!({
        "message": format!("{} repository '{}' in '{}'", if existing.is_some() { "Updated" } else { "Created" }, repo_name, ctx_name),
        "suggested_path": suggested_path_for(store, workspace_path, &ctx_name, "repository", &repo_name)
    }).to_string())
}

fn remove_repository(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let repo_name = arg_str(args, "name");
    match store.remove_repository(workspace_path, &ctx_name, &repo_name) {
        Ok(true) => text_result(format!(
            "Removed repository '{repo_name}' from '{ctx_name}'"
        )),
        Ok(false) => error_result(format!(
            "Repository '{repo_name}' not found in '{ctx_name}'"
        )),
        Err(e) => error_result(format!("Failed to remove repository: {e}")),
    }
}

fn upsert_module(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let mod_name = arg_str(args, "name");
    let existing = match require_context(store, workspace_path, &ctx_name) {
        Ok(()) => store.query_module(workspace_path, &ctx_name, &mod_name),
        Err(result) => return result,
    };
    let mut module = existing.clone().unwrap_or(Module {
        name: mod_name.clone(),
        path: String::new(),
        public: true,
        file_path: String::new(),
        description: String::new(),
    });
    if let Some(desc) = args.get("description").and_then(|v| v.as_str()) {
        module.description = desc.to_string();
    }
    if let Some(path) = args.get("module_path").and_then(|v| v.as_str()) {
        module.path = path.to_string();
    }
    if let Some(public) = args.get("public").and_then(|v| v.as_bool()) {
        module.public = public;
    }
    if let Err(e) = store.upsert_module(workspace_path, &ctx_name, &module) {
        return error_result(format!("Failed to upsert module: {e}"));
    }
    text_result(json!({
        "message": format!("{} module '{}' in '{}'", if existing.is_some() { "Updated" } else { "Created" }, mod_name, ctx_name),
    }).to_string())
}

fn remove_module_handler(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let mod_name = arg_str(args, "name");
    match store.remove_module(workspace_path, &ctx_name, &mod_name) {
        Ok(true) => text_result(format!("Removed module '{mod_name}' from '{ctx_name}'")),
        Ok(false) => error_result(format!("Module '{mod_name}' not found in '{ctx_name}'")),
        Err(e) => error_result(format!("Failed to remove module: {e}")),
    }
}

fn upsert_aggregate(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let aggregate_name = arg_str(args, "name");

    let existing = match require_context(store, workspace_path, &ctx_name) {
        Ok(()) => store.query_aggregate(workspace_path, &ctx_name, &aggregate_name),
        Err(result) => return result,
    };

    let mut aggregate = existing.clone().unwrap_or(Aggregate {
        name: aggregate_name.clone(),
        description: String::new(),
        root_entity: String::new(),
        entities: vec![],
        value_objects: vec![],
        ownership: Ownership::default(),
    });
    if let Some(description) = args.get("description").and_then(|v| v.as_str()) {
        aggregate.description = description.to_string();
    }
    if let Some(root_entity) = args.get("root_entity").and_then(|v| v.as_str()) {
        aggregate.root_entity = root_entity.to_string();
    }
    if args.get("entities").is_some() {
        aggregate.entities = parse_string_array(args.get("entities"));
    }
    if args.get("value_objects").is_some() {
        aggregate.value_objects = parse_string_array(args.get("value_objects"));
    }
    if args.get("ownership").is_some() {
        aggregate.ownership = parse_ownership(args.get("ownership"));
    }
    if let Err(e) = store.upsert_aggregate(workspace_path, &ctx_name, &aggregate) {
        return error_result(format!("Failed to upsert aggregate: {e}"));
    }
    text_result(json!({"message": format!("{} aggregate '{}' in '{}'", if existing.is_some() { "Updated" } else { "Created" }, aggregate_name, ctx_name)}).to_string())
}

fn remove_aggregate(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let aggregate_name = arg_str(args, "name");
    match store.remove_aggregate(workspace_path, &ctx_name, &aggregate_name) {
        Ok(true) => text_result(format!(
            "Removed aggregate '{aggregate_name}' from '{ctx_name}'"
        )),
        Ok(false) => error_result(format!(
            "Aggregate '{aggregate_name}' not found in '{ctx_name}'"
        )),
        Err(e) => error_result(format!("Failed to remove aggregate: {e}")),
    }
}

fn upsert_policy(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let policy_name = arg_str(args, "name");

    let existing = match require_context(store, workspace_path, &ctx_name) {
        Ok(()) => store.query_policy(workspace_path, &ctx_name, &policy_name),
        Err(result) => return result,
    };
    let mut policy = existing.clone().unwrap_or(Policy {
        name: policy_name.clone(),
        description: String::new(),
        kind: PolicyKind::Domain,
        triggers: vec![],
        commands: vec![],
        ownership: Ownership::default(),
    });
    if let Some(description) = args.get("description").and_then(|v| v.as_str()) {
        policy.description = description.to_string();
    }
    if args.get("policy_kind").is_some() {
        policy.kind = parse_policy_kind(&arg_str(args, "policy_kind"));
    }
    if args.get("triggers").is_some() {
        policy.triggers = parse_string_array(args.get("triggers"));
    }
    if args.get("commands").is_some() {
        policy.commands = parse_string_array(args.get("commands"));
    }
    if args.get("ownership").is_some() {
        policy.ownership = parse_ownership(args.get("ownership"));
    }
    if let Err(e) = store.upsert_policy(workspace_path, &ctx_name, &policy) {
        return error_result(format!("Failed to upsert policy: {e}"));
    }
    text_result(json!({"message": format!("{} policy '{}' in '{}'", if existing.is_some() { "Updated" } else { "Created" }, policy_name, ctx_name)}).to_string())
}

fn remove_policy(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let policy_name = arg_str(args, "name");
    match store.remove_policy(workspace_path, &ctx_name, &policy_name) {
        Ok(true) => text_result(format!("Removed policy '{policy_name}' from '{ctx_name}'")),
        Ok(false) => error_result(format!("Policy '{policy_name}' not found in '{ctx_name}'")),
        Err(e) => error_result(format!("Failed to remove policy: {e}")),
    }
}

fn upsert_read_model(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let read_model_name = arg_str(args, "name");

    let existing = match require_context(store, workspace_path, &ctx_name) {
        Ok(()) => store.query_read_model(workspace_path, &ctx_name, &read_model_name),
        Err(result) => return result,
    };
    let mut read_model = existing.clone().unwrap_or(ReadModel {
        name: read_model_name.clone(),
        description: String::new(),
        source: String::new(),
        fields: vec![],
        ownership: Ownership::default(),
    });
    if let Some(description) = args.get("description").and_then(|v| v.as_str()) {
        read_model.description = description.to_string();
    }
    if let Some(source) = args.get("source").and_then(|v| v.as_str()) {
        read_model.source = source.to_string();
    }
    if let Some(fields) = args.get("fields").and_then(|v| v.as_array()) {
        merge_fields(&mut read_model.fields, fields);
    }
    if args.get("ownership").is_some() {
        read_model.ownership = parse_ownership(args.get("ownership"));
    }
    if let Err(e) = store.upsert_read_model(workspace_path, &ctx_name, &read_model) {
        return error_result(format!("Failed to upsert read model: {e}"));
    }
    text_result(json!({"message": format!("{} read model '{}' in '{}'", if existing.is_some() { "Updated" } else { "Created" }, read_model_name, ctx_name)}).to_string())
}

fn remove_read_model(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let ctx_name = arg_str(args, "context");
    let read_model_name = arg_str(args, "name");
    match store.remove_read_model(workspace_path, &ctx_name, &read_model_name) {
        Ok(true) => text_result(format!(
            "Removed read model '{read_model_name}' from '{ctx_name}'"
        )),
        Ok(false) => error_result(format!(
            "Read model '{read_model_name}' not found in '{ctx_name}'"
        )),
        Err(e) => error_result(format!("Failed to remove read model: {e}")),
    }
}

fn upsert_external_system(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let system_name = arg_str(args, "name");
    let existing = store.query_external_system(workspace_path, &system_name);
    let mut system = existing.clone().unwrap_or(ExternalSystem {
        name: system_name.clone(),
        description: String::new(),
        kind: String::new(),
        consumed_by_contexts: vec![],
        rationale: String::new(),
        ownership: Ownership::default(),
    });
    if let Some(description) = args.get("description").and_then(|v| v.as_str()) {
        system.description = description.to_string();
    }
    if let Some(kind) = args.get("kind_label").and_then(|v| v.as_str()) {
        system.kind = kind.to_string();
    }
    if let Some(rationale) = args.get("rationale").and_then(|v| v.as_str()) {
        system.rationale = rationale.to_string();
    }
    if args.get("consumed_by_contexts").is_some() {
        system.consumed_by_contexts = parse_string_array(args.get("consumed_by_contexts"));
    }
    if args.get("ownership").is_some() {
        system.ownership = parse_ownership(args.get("ownership"));
    }
    if let Err(e) = store.upsert_external_system(workspace_path, &system) {
        return error_result(format!("Failed to upsert external system: {e}"));
    }
    text_result(json!({"message": format!("{} external system '{}'", if existing.is_some() { "Updated" } else { "Created" }, system_name)}).to_string())
}

fn remove_external_system(store: &Store, workspace_path: &str, args: &Value) -> ToolCallResult {
    let system_name = arg_str(args, "name");
    match store.remove_external_system(workspace_path, &system_name) {
        Ok(true) => text_result(format!("Removed external system '{system_name}'")),
        Ok(false) => error_result(format!("External system '{system_name}' not found")),
        Err(e) => error_result(format!("Failed to remove external system: {e}")),
    }
}

fn upsert_architectural_decision(
    store: &Store,
    workspace_path: &str,
    args: &Value,
) -> ToolCallResult {
    let decision_id = arg_str(args, "name");
    let existing = store.query_architectural_decision(workspace_path, &decision_id);
    let mut decision = existing.clone().unwrap_or(ArchitecturalDecision {
        id: decision_id.clone(),
        title: String::new(),
        status: DecisionStatus::Proposed,
        scope: String::new(),
        date: String::new(),
        rationale: String::new(),
        consequences: vec![],
        contexts: vec![],
        ownership: Ownership::default(),
    });
    if let Some(title) = args.get("title").and_then(|v| v.as_str()) {
        decision.title = title.to_string();
    }
    if args.get("status").is_some() {
        decision.status = parse_decision_status(&arg_str(args, "status"));
    }
    if let Some(scope) = args.get("scope").and_then(|v| v.as_str()) {
        decision.scope = scope.to_string();
    }
    if let Some(date) = args.get("date").and_then(|v| v.as_str()) {
        decision.date = date.to_string();
    }
    if let Some(rationale) = args.get("rationale").and_then(|v| v.as_str()) {
        decision.rationale = rationale.to_string();
    }
    if args.get("contexts").is_some() {
        decision.contexts = parse_string_array(args.get("contexts"));
    }
    if args.get("consequences").is_some() {
        decision.consequences = parse_string_array(args.get("consequences"));
    }
    if args.get("ownership").is_some() {
        decision.ownership = parse_ownership(args.get("ownership"));
    }
    if let Err(e) = store.upsert_architectural_decision(workspace_path, &decision) {
        return error_result(format!("Failed to upsert architectural decision: {e}"));
    }
    text_result(json!({"message": format!("{} architectural decision '{}'", if existing.is_some() { "Updated" } else { "Created" }, decision_id)}).to_string())
}

fn remove_architectural_decision(
    store: &Store,
    workspace_path: &str,
    args: &Value,
) -> ToolCallResult {
    let decision_id = arg_str(args, "name");
    match store.remove_architectural_decision(workspace_path, &decision_id) {
        Ok(true) => text_result(format!("Removed architectural decision '{decision_id}'")),
        Ok(false) => error_result(format!("Architectural decision '{decision_id}' not found")),
        Err(e) => error_result(format!("Failed to remove architectural decision: {e}")),
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

fn text_result(text: impl Into<String>) -> ToolCallResult {
    text_tool_result(text)
}

fn error_result(msg: impl Into<String>) -> ToolCallResult {
    error_tool_result(msg)
}

fn json_result(payload: Value) -> ToolCallResult {
    json_tool_result(payload)
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

fn reasoning_result(
    store: &Store,
    workspace_path: &str,
    payload: Value,
    proof: Option<Value>,
    evidence: Option<Value>,
    limitations: Vec<String>,
    provenance: Value,
) -> ToolCallResult {
    let mut envelope =
        with_reasoning_context(payload, proof, evidence, limitations, Some(provenance));
    if let Some(object) = envelope.as_object_mut() {
        object.insert(
            "truth_maintenance".into(),
            truth_maintenance_json(store, workspace_path),
        );
    }
    json_result(envelope)
}

fn reasoning_error_result(
    store: &Store,
    workspace_path: &str,
    payload: Value,
    proof: Option<Value>,
    evidence: Option<Value>,
    limitations: Vec<String>,
    provenance: Value,
) -> ToolCallResult {
    let mut envelope =
        with_reasoning_context(payload, proof, evidence, limitations, Some(provenance));
    if let Some(object) = envelope.as_object_mut() {
        object.insert(
            "truth_maintenance".into(),
            truth_maintenance_json(store, workspace_path),
        );
    }
    json_error_tool_result(envelope)
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

fn invalidate_and_refresh_dependency(
    store: &Store,
    workspace_path: &str,
    dependency_state: &str,
) -> Result<(), String> {
    store
        .invalidate_reasoning_claims_for_dependency(workspace_path, dependency_state)
        .map_err(|e| format!("reasoning invalidation failed: {e}"))?;
    eager_refresh_dependencies(store, workspace_path, &[dependency_state])
}

fn invalidate_and_refresh_facts(
    store: &Store,
    workspace_path: &str,
    fact_refs: &[ReasoningFactRef],
) -> Result<(), String> {
    store
        .invalidate_reasoning_claims_for_facts(workspace_path, fact_refs)
        .map_err(|e| format!("reasoning fact invalidation failed: {e}"))?;
    let kernel = ReasoningKernel::new(store);
    kernel
        .eager_refresh_stale_claims(workspace_path)
        .map_err(|e| format!("reasoning eager refresh failed after fact invalidation: {e}"))?;
    Ok(())
}

fn invalidate_and_refresh_policy(store: &Store, workspace_path: &str) -> Result<(), String> {
    store
        .invalidate_reasoning_claims_for_facts(
            workspace_path,
            &[
                fact_ref("layer_assignment", "*", "actual"),
                fact_ref("dependency_constraint", "*", "actual"),
            ],
        )
        .map_err(|e| format!("reasoning policy fact invalidation failed: {e}"))?;
    store
        .invalidate_reasoning_claims_for_dependency(workspace_path, "actual")
        .map_err(|e| format!("reasoning policy dependency invalidation failed: {e}"))?;
    let kernel = ReasoningKernel::new(store);
    kernel
        .eager_refresh_stale_claims(workspace_path)
        .map_err(|e| format!("reasoning eager refresh failed after policy invalidation: {e}"))?;
    Ok(())
}

fn eager_refresh_dependencies(
    store: &Store,
    workspace_path: &str,
    dependency_states: &[&str],
) -> Result<(), String> {
    let kernel = ReasoningKernel::new(store);
    for dependency_state in dependency_states {
        kernel
            .eager_refresh_for_dependency(workspace_path, dependency_state)
            .map_err(|e| {
                format!("reasoning eager refresh failed for dependency '{dependency_state}': {e}")
            })?;
    }
    Ok(())
}

fn arg_str(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn fact_ref(fact_kind: &str, fact_key: &str, fact_state: &str) -> ReasoningFactRef {
    ReasoningFactRef {
        fact_kind: fact_kind.into(),
        fact_key: fact_key.into(),
        fact_state: fact_state.into(),
    }
}

fn reasoning_fact_refs_for_define(args: &Value) -> Vec<ReasoningFactRef> {
    let kind = arg_str(args, "kind");
    let context = arg_str(args, "context");
    let name = arg_str(args, "name");
    let mut refs = Vec::new();
    let fact_state = "actual";

    match kind.as_str() {
        "bounded_context" => {
            if !name.is_empty() {
                refs.push(fact_ref("context", &name, fact_state));
            }
            refs.push(fact_ref("context_dep", "*", fact_state));
        }
        "entity" => {
            if !context.is_empty() && !name.is_empty() {
                refs.push(fact_ref("entity", &format!("{context}/{name}"), fact_state));
            }
            refs.push(fact_ref("field", "*", fact_state));
            refs.push(fact_ref("method", "*", fact_state));
            refs.push(fact_ref("invariant", "*", fact_state));
        }
        "service" => {
            if !context.is_empty() && !name.is_empty() {
                refs.push(fact_ref(
                    "service",
                    &format!("{context}/{name}"),
                    fact_state,
                ));
            }
            refs.push(fact_ref("service_dep", "*", fact_state));
            refs.push(fact_ref("method", "*", fact_state));
        }
        "event" => {
            if !context.is_empty() && !name.is_empty() {
                refs.push(fact_ref("event", &format!("{context}/{name}"), fact_state));
            }
            refs.push(fact_ref("field", "*", fact_state));
        }
        "value_object" => {
            if !context.is_empty() && !name.is_empty() {
                refs.push(fact_ref(
                    "value_object",
                    &format!("{context}/{name}"),
                    fact_state,
                ));
            }
            refs.push(fact_ref("field", "*", fact_state));
            refs.push(fact_ref("vo_rule", "*", fact_state));
        }
        "repository" => {
            if !context.is_empty() && !name.is_empty() {
                refs.push(fact_ref(
                    "repository",
                    &format!("{context}/{name}"),
                    fact_state,
                ));
            }
            refs.push(fact_ref("method", "*", fact_state));
        }
        "aggregate" => {
            if !context.is_empty() && !name.is_empty() {
                refs.push(fact_ref(
                    "aggregate",
                    &format!("{context}/{name}"),
                    fact_state,
                ));
            }
            refs.push(fact_ref("aggregate_member", "*", fact_state));
        }
        "policy" => {
            if !context.is_empty() && !name.is_empty() {
                refs.push(fact_ref("policy", &format!("{context}/{name}"), fact_state));
            }
            refs.push(fact_ref("policy_link", "*", fact_state));
        }
        "read_model" => {
            if !context.is_empty() && !name.is_empty() {
                refs.push(fact_ref(
                    "read_model",
                    &format!("{context}/{name}"),
                    fact_state,
                ));
            }
            refs.push(fact_ref("field", "*", fact_state));
        }
        "module" if !context.is_empty() && !name.is_empty() => {
            refs.push(fact_ref("module", &format!("{context}/{name}"), fact_state));
        }
        "external_system" => {
            if !name.is_empty() {
                refs.push(fact_ref("external_system", &name, fact_state));
            }
            refs.push(fact_ref("external_system_context", "*", fact_state));
        }
        "architectural_decision" => {
            if !name.is_empty() {
                refs.push(fact_ref("architectural_decision", &name, fact_state));
            }
            refs.push(fact_ref("decision_context", "*", fact_state));
            refs.push(fact_ref("decision_consequence", "*", fact_state));
        }
        _ => {}
    }

    refs
}

fn parse_ownership(val: Option<&Value>) -> Ownership {
    let Some(obj) = val else {
        return Ownership::default();
    };
    Ownership {
        team: obj
            .get("team")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        owners: obj
            .get("owners")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        rationale: obj
            .get("rationale")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    }
}

fn parse_string_array(val: Option<&Value>) -> Vec<String> {
    val.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_policy_kind(kind: &str) -> PolicyKind {
    match kind {
        "process_manager" => PolicyKind::ProcessManager,
        "integration" => PolicyKind::Integration,
        _ => PolicyKind::Domain,
    }
}

fn parse_service_kind(kind: &str) -> ServiceKind {
    match kind {
        "application" => ServiceKind::Application,
        "infrastructure" => ServiceKind::Infrastructure,
        _ => ServiceKind::Domain,
    }
}

fn parse_decision_status(status: &str) -> DecisionStatus {
    match status {
        "accepted" => DecisionStatus::Accepted,
        "superseded" => DecisionStatus::Superseded,
        "deprecated" => DecisionStatus::Deprecated,
        _ => DecisionStatus::Proposed,
    }
}

fn require_context(
    store: &Store,
    workspace_path: &str,
    ctx_name: &str,
) -> Result<(), ToolCallResult> {
    let exists = store
        .load_desired(workspace_path)
        .ok()
        .flatten()
        .map(|model| {
            model
                .bounded_contexts
                .iter()
                .any(|bc| bc.name.eq_ignore_ascii_case(ctx_name))
        })
        .unwrap_or(false);
    if exists {
        Ok(())
    } else {
        Err(error_result(format!(
            "Bounded context '{ctx_name}' not found"
        )))
    }
}

fn suggested_path_for(
    store: &Store,
    workspace_path: &str,
    context: &str,
    kind: &str,
    name: &str,
) -> String {
    let pattern = store
        .load_desired(workspace_path)
        .ok()
        .flatten()
        .map(|model| model.conventions.file_structure.pattern)
        .unwrap_or_default();
    suggest_path(&pattern, context, kind, name)
}

/// Compute the suggested file path for a domain artifact, using project conventions.
/// This replaces the standalone `suggest_file_path` tool — now implicit in every
/// `rust_annotations` response for artifacts that live in files (entity, service, event).
fn suggest_path(pattern: &str, context: &str, kind: &str, name: &str) -> String {
    let layer = match kind {
        "entity" | "value_object" | "event" => "domain",
        "service" => "application",
        "repository" => "infrastructure",
        other => other,
    };
    if pattern.is_empty() {
        return format!("src/{}/{}/{}.rs", to_snake(context), layer, to_snake(name));
    }
    pattern
        .replace("{context}", &to_snake(context))
        .replace("{layer}", layer)
        .replace("{type}", &to_snake(name))
}

fn parse_fields(val: Option<&Value>) -> Vec<Field> {
    val.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    Some(Field {
                        name: f.get("name")?.as_str()?.to_string(),
                        field_type: f.get("type")?.as_str()?.to_string(),
                        required: f.get("required").and_then(|v| v.as_bool()).unwrap_or(false),
                        description: f
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_methods(val: Option<&Value>) -> Vec<Method> {
    val.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    Some(Method {
                        name: m.get("name")?.as_str()?.to_string(),
                        description: m
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        parameters: parse_fields(m.get("parameters")),
                        return_type: m
                            .get("return_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn merge_fields(existing: &mut Vec<Field>, new_fields: &[Value]) {
    for f in new_fields {
        let name = match f.get("name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => continue,
        };
        if let Some(existing_f) = existing.iter_mut().find(|ef| ef.name == name) {
            if let Some(t) = f.get("type").and_then(|v| v.as_str()) {
                existing_f.field_type = t.to_string();
            }
            if let Some(r) = f.get("required").and_then(|v| v.as_bool()) {
                existing_f.required = r;
            }
            if let Some(d) = f.get("description").and_then(|v| v.as_str()) {
                existing_f.description = d.to_string();
            }
        } else if let Some(field) = parse_fields(Some(&json!([f]))).into_iter().next() {
            existing.push(field);
        }
    }
}

fn merge_methods(existing: &mut Vec<Method>, new_methods: &[Value]) {
    for m in new_methods {
        let name = match m.get("name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => continue,
        };
        if let Some(existing_m) = existing.iter_mut().find(|em| em.name == name) {
            if let Some(d) = m.get("description").and_then(|v| v.as_str()) {
                existing_m.description = d.to_string();
            }
            if let Some(rt) = m.get("return_type").and_then(|v| v.as_str()) {
                existing_m.return_type = rt.to_string();
            }
        } else if let Some(method) = parse_methods(Some(&json!([m]))).into_iter().next() {
            existing.push(method);
        }
    }
}

#[derive(Clone, Copy)]
struct SyncCounts {
    contexts_scanned: usize,
    entities: usize,
    value_objects: usize,
    services: usize,
    repositories: usize,
    events: usize,
    source_files: usize,
    symbols: usize,
    import_edges: usize,
    persisted_import_edges: Option<usize>,
    reference_edges: usize,
    persisted_reference_edges: Option<usize>,
    call_edges: usize,
    persisted_call_edges: Option<usize>,
    resolved_call_edges: usize,
    persisted_resolved_call_edges: Option<usize>,
    semantic_resolution_succeeded: bool,
}

fn build_sync_report(
    counts: SyncCounts,
    had_previous: bool,
    drift_entry_count: Option<usize>,
    follow_on_failures: &[String],
) -> Value {
    let status = if follow_on_failures.is_empty() {
        "scanned"
    } else {
        "partial_failure"
    };
    let drift_recomputed = drift_entry_count.is_some();
    let persisted_import_edges = counts.persisted_import_edges.unwrap_or(counts.import_edges);
    let persisted_reference_edges = counts
        .persisted_reference_edges
        .unwrap_or(counts.reference_edges);
    let persisted_call_edges = counts.persisted_call_edges.unwrap_or(counts.call_edges);
    let persisted_resolved_call_edges = counts
        .persisted_resolved_call_edges
        .unwrap_or(counts.resolved_call_edges);
    let message = if follow_on_failures.is_empty() {
        if had_previous {
            format!(
                "Scanned {} contexts -> {} entities, {} VOs, {} services, {} repos, {} events. Implemented model updated and temporal changes recomputed.",
                counts.contexts_scanned,
                counts.entities,
                counts.value_objects,
                counts.services,
                counts.repositories,
                counts.events
            )
        } else {
            format!(
                "Scanned {} contexts -> {} entities, {} VOs, {} services, {} repos, {} events. Initial implemented model stored.",
                counts.contexts_scanned,
                counts.entities,
                counts.value_objects,
                counts.services,
                counts.repositories,
                counts.events
            )
        }
    } else {
        format!(
            "Scanned {} contexts -> {} entities, {} VOs, {} services, {} repos, {} events. Implemented model updated, but {} follow-on synchronization step(s) failed.",
            counts.contexts_scanned,
            counts.entities,
            counts.value_objects,
            counts.services,
            counts.repositories,
            counts.events,
            follow_on_failures.len()
        )
    };

    json!({
        "status": status,
        "message": message,
        "contexts_scanned": counts.contexts_scanned,
        "entities": counts.entities,
        "value_objects": counts.value_objects,
        "services": counts.services,
        "repositories": counts.repositories,
        "events": counts.events,
        "source_files": counts.source_files,
        "symbols": counts.symbols,
        "import_edges": persisted_import_edges,
        "extracted_import_edges": counts.import_edges,
        "persisted_import_edges": counts.persisted_import_edges,
        "reference_edges": persisted_reference_edges,
        "extracted_reference_edges": counts.reference_edges,
        "persisted_reference_edges": counts.persisted_reference_edges,
        "call_edges": persisted_call_edges,
        "extracted_call_edges": counts.call_edges,
        "persisted_call_edges": counts.persisted_call_edges,
        "resolved_call_edges": persisted_resolved_call_edges,
        "extracted_resolved_call_edges": counts.resolved_call_edges,
        "persisted_resolved_call_edges": counts.persisted_resolved_call_edges,
        "semantic_resolution": if counts.semantic_resolution_succeeded { "resolved" } else { "failed" },
        "implemented_state_saved": true,
        "had_previous_snapshot": had_previous,
        "drift_recomputed": drift_recomputed,
        "drift_entry_count": drift_entry_count,
        "follow_on_failures": follow_on_failures,
    })
}

fn sync_limitations(drift_failed: bool, semantic_resolution_failed: bool) -> Vec<String> {
    let mut limitations = vec![
        "Scan coverage is limited to statically extracted facts from supported languages.".into(),
        "Dynamic dispatch, reflection, generated code, and runtime-only dependencies are not modeled.".into(),
    ];

    if semantic_resolution_failed {
        limitations.push(
            "Compiler-resolved call edges are unavailable until rust-analyzer resolution succeeds."
                .into(),
        );
    }

    if drift_failed {
        limitations.push(
            "Temporal change entries may be stale until drift recomputation succeeds.".into(),
        );
    }

    limitations
}

fn diagnose_pipeline(store: &Store, workspace_path: &str) -> ToolCallResult {
    let kernel = ReasoningKernel::new(store);
    match kernel.diagnose(workspace_path) {
        Ok(mut claim) => {
            attach_diagnose_readiness(&mut claim.payload, store, workspace_path);
            stored_claim_result(store, workspace_path, &claim)
        }
        Err(e) => error_result(format!("diagnose failed: {e}")),
    }
}

fn attach_diagnose_readiness(payload: &mut Value, store: &Store, workspace_path: &str) {
    let confidence = crate::mcp::tools::build_graph_confidence_report(store, workspace_path);
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    object.insert(
        "graph_confidence".into(),
        confidence["graph_confidence"].clone(),
    );
    object.insert("readiness_summary".into(), confidence["summary"].clone());

    let graph_status = confidence["summary"]["status"]
        .as_str()
        .unwrap_or("unknown");
    if graph_status == "ready" {
        return;
    }

    if object["status"].as_str() == Some("healthy") {
        object.insert("status".into(), json!("healthy_with_readiness_warnings"));
    }

    object.insert(
        "readiness_warnings".into(),
        confidence["graph_confidence"]["warnings"].clone(),
    );

    let mut next_actions = object
        .remove("next_actions")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    if let Some(actions) = confidence["graph_confidence"]["next_actions"].as_array() {
        let base_priority = next_actions.len();
        for (offset, action) in actions.iter().enumerate() {
            next_actions.push(json!({
                "priority": base_priority + offset,
                "tool": "rust_readiness",
                "reason": action,
            }));
        }
    }
    object.insert("next_action_count".into(), json!(next_actions.len()));
    object.insert("next_actions".into(), Value::Array(next_actions));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::env::temp_dir;

    fn test_store() -> Store {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = temp_dir().join(format!("axon_wt_test_{}_{}.db", std::process::id(), id));
        Store::open(&path).unwrap()
    }

    fn test_model() -> DomainModel {
        DomainModel {
            name: "TestProject".into(),
            description: "Test".into(),
            bounded_contexts: vec![BoundedContext {
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
            reference_edges: vec![],
        }
    }

    /// Save initial model and return (store, workspace).
    fn setup(ws: &str) -> Store {
        let store = test_store();
        store.save_desired(ws, &test_model()).unwrap();
        store
    }

    #[test]
    fn test_list_write_tools_count() {
        let tools = list_write_tools();
        assert_eq!(tools.len(), 4);
        let names: Vec<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert!(names.contains(&"rust_scan"));
        assert!(names.contains(&"rust_annotations"));
        assert!(names.contains(&"rust_diagnose"));
        assert!(names.contains(&"rust_constraints"));
        assert!(!names.contains(&"set_model"));
        assert!(!names.contains(&"scan_model"));
        assert!(!names.contains(&"refactor_model"));
        assert!(!names.contains(&"assert_model"));
    }

    #[test]
    fn test_build_sync_report_success_after_initial_scan() {
        let report = build_sync_report(
            SyncCounts {
                contexts_scanned: 2,
                entities: 4,
                value_objects: 3,
                services: 1,
                repositories: 1,
                events: 2,
                source_files: 6,
                symbols: 9,
                import_edges: 4,
                persisted_import_edges: Some(3),
                reference_edges: 6,
                persisted_reference_edges: Some(6),
                call_edges: 11,
                persisted_call_edges: Some(7),
                resolved_call_edges: 5,
                persisted_resolved_call_edges: Some(5),
                semantic_resolution_succeeded: true,
            },
            false,
            Some(5),
            &[],
        );

        assert_eq!(report["status"], "scanned");
        assert_eq!(report["implemented_state_saved"], true);
        assert_eq!(report["had_previous_snapshot"], false);
        assert_eq!(report["drift_recomputed"], true);
        assert_eq!(report["drift_entry_count"], 5);
        assert_eq!(report["source_files"], 6);
        assert_eq!(report["symbols"], 9);
        assert_eq!(report["import_edges"], 3);
        assert_eq!(report["extracted_import_edges"], 4);
        assert_eq!(report["persisted_import_edges"], 3);
        assert_eq!(report["reference_edges"], 6);
        assert_eq!(report["extracted_reference_edges"], 6);
        assert_eq!(report["call_edges"], 7);
        assert_eq!(report["extracted_call_edges"], 11);
        assert_eq!(report["persisted_call_edges"], 7);
        assert_eq!(report["resolved_call_edges"], 5);
        assert_eq!(report["semantic_resolution"], "resolved");
        assert!(report["follow_on_failures"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_diagnose_does_not_promote_test_only_practice_findings() {
        let ws = "/tmp/test-diagnose-test-only-practice";
        let store = setup(ws);
        let mut model = store.load_actual(ws).unwrap().unwrap();
        model.call_edges = vec![CallEdge {
            caller: "test_load_fixture".into(),
            callee: "unwrap".into(),
            file_path: "src/store/cozo_tests.rs".into(),
            line: 31,
            context: "store".into(),
        }];
        store.save_actual(ws, &model).unwrap();

        let result = call_write_tool(ws, &store, "rust_diagnose", &json!({"action": "diagnose"}));
        assert!(result.is_error.is_none() || result.is_error == Some(false));

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
        };
        let report: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            report["practice_findings"]["summary"]["actionable_count"],
            0
        );
        assert_eq!(report["practice_findings"]["summary"]["test_count"], 1);
        assert!(
            report["practice_findings"]["top_actionable"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let actions = report["next_actions"].as_array().unwrap();
        assert!(actions.iter().all(|action| {
            action["tool"] != "rust_impact" || action["action"] != "practice_findings"
        }));
    }

    #[test]
    fn test_diagnose_readiness_degrades_healthy_payload() {
        let store = test_store();
        let ws = format!("/tmp/test-diagnose-readiness-{}", std::process::id());
        let mut payload = json!({
            "status": "healthy",
            "next_actions": [],
        });

        attach_diagnose_readiness(&mut payload, &store, &ws);

        assert_eq!(payload["status"], "healthy_with_readiness_warnings");
        assert_eq!(payload["graph_confidence"]["status"], "not_scanned");
        assert!(payload["readiness_warnings"].as_array().unwrap().len() >= 1);
        assert!(payload["next_action_count"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn test_build_sync_report_marks_follow_on_failures() {
        let failures = vec!["drift recomputation failed: stale state".to_string()];
        let report = build_sync_report(
            SyncCounts {
                contexts_scanned: 1,
                entities: 1,
                value_objects: 0,
                services: 0,
                repositories: 0,
                events: 0,
                source_files: 1,
                symbols: 1,
                import_edges: 0,
                persisted_import_edges: None,
                reference_edges: 0,
                persisted_reference_edges: None,
                call_edges: 0,
                persisted_call_edges: None,
                resolved_call_edges: 0,
                persisted_resolved_call_edges: None,
                semantic_resolution_succeeded: false,
            },
            false,
            None,
            &failures,
        );

        assert_eq!(report["status"], "partial_failure");
        assert_eq!(report["implemented_state_saved"], true);
        assert_eq!(report["had_previous_snapshot"], false);
        assert_eq!(report["drift_recomputed"], false);
        assert_eq!(report["drift_entry_count"], serde_json::Value::Null);
        assert_eq!(report["follow_on_failures"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_build_sync_report_treats_semantic_resolution_failure_as_degraded_success() {
        let report = build_sync_report(
            SyncCounts {
                contexts_scanned: 1,
                entities: 1,
                value_objects: 0,
                services: 0,
                repositories: 0,
                events: 0,
                source_files: 1,
                symbols: 1,
                import_edges: 1,
                persisted_import_edges: Some(1),
                reference_edges: 1,
                persisted_reference_edges: Some(1),
                call_edges: 1,
                persisted_call_edges: Some(1),
                resolved_call_edges: 0,
                persisted_resolved_call_edges: Some(0),
                semantic_resolution_succeeded: false,
            },
            false,
            Some(0),
            &[],
        );

        assert_eq!(report["status"], "scanned");
        assert_eq!(report["implemented_state_saved"], true);
        assert_eq!(report["semantic_resolution"], "failed");
        assert_eq!(report["resolved_call_edges"], 0);
        assert!(report["follow_on_failures"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_update_entity_add_field() {
        let ws = "/tmp/test-add-field";
        let store = setup(ws);
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "entity", "context": "Identity", "name": "User",
                "fields": [{"name": "email", "type": "String", "required": true}]
            }),
        );
        assert!(result.is_error.is_none());
        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        let user = identity.entities.iter().find(|e| e.name == "User").unwrap();
        assert_eq!(user.fields.len(), 2);
        assert!(user.fields.iter().any(|f| f.name == "email"));
    }

    #[test]
    fn test_update_entity_merge_existing_field() {
        let ws = "/tmp/test-merge-field";
        let store = setup(ws);
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "entity", "context": "Identity", "name": "User",
                "fields": [{"name": "id", "type": "Uuid"}]
            }),
        );
        assert!(result.is_error.is_none());
        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        let user = identity.entities.iter().find(|e| e.name == "User").unwrap();
        assert_eq!(user.fields.len(), 1);
        assert_eq!(user.fields[0].field_type, "Uuid");
    }

    #[test]
    fn test_create_new_entity() {
        let ws = "/tmp/test-new-entity";
        let store = setup(ws);
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "entity",
                "module": "Identity",
                "name": "Role",
                "data": {
                    "description": "A role assignment",
                    "aggregate_root": false,
                    "fields": [{"name": "name", "type": "String"}]
                }
            }),
        );
        assert!(result.is_error.is_none());
        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        assert_eq!(identity.entities.len(), 2);
        assert!(identity.entities.iter().any(|e| e.name == "Role"));
    }

    #[test]
    fn test_update_entity_context_not_found() {
        let ws = "/tmp/test-ctx-notfound";
        let store = setup(ws);
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({"kind": "entity", "context": "Nonexistent", "name": "Foo"}),
        );
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn test_create_bounded_context() {
        let ws = "/tmp/test-create-bc";
        let store = setup(ws);
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "bounded_context", "name": "Billing",
                "description": "Billing context", "module_path": "src/billing",
                "dependencies": ["Identity"]
            }),
        );
        assert!(result.is_error.is_none());
        let loaded = store.load_desired(ws).unwrap().unwrap();
        assert_eq!(loaded.bounded_contexts.len(), 2);
        let billing = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Billing")
            .unwrap();
        assert_eq!(billing.dependencies, vec!["Identity"]);
    }

    #[test]
    fn test_update_existing_bounded_context() {
        let ws = "/tmp/test-update-bc";
        let store = setup(ws);
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "bounded_context", "name": "Identity",
                "description": "Updated description"
            }),
        );
        assert!(result.is_error.is_none());
        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        assert_eq!(identity.description, "Updated description");
    }

    #[test]
    fn test_remove_entity() {
        let ws = "/tmp/test-rm-entity";
        let store = setup(ws);
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({"kind": "entity", "action": "remove", "context": "Identity", "name": "User"}),
        );
        assert!(result.is_error.is_none());
        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        assert_eq!(identity.entities.len(), 0);
    }

    #[test]
    fn test_remove_entity_not_found() {
        let ws = "/tmp/test-rm-entity-nf";
        let store = setup(ws);
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({"kind": "entity", "action": "remove", "context": "Identity", "name": "NotHere"}),
        );
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn test_update_service() {
        let ws = "/tmp/test-upd-svc";
        let store = setup(ws);
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "service", "context": "Identity", "name": "AuthService",
                "service_kind": "application", "description": "Handles authentication"
            }),
        );
        assert!(result.is_error.is_none());
        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        assert_eq!(identity.services.len(), 1);
        assert_eq!(identity.services[0].description, "Handles authentication");
    }

    #[test]
    fn test_update_event() {
        let ws = "/tmp/test-upd-evt";
        let store = setup(ws);
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "event", "context": "Identity", "name": "UserRegistered",
                "source": "User", "fields": [{"name": "user_id", "type": "UserId"}]
            }),
        );
        assert!(result.is_error.is_none());
        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        assert_eq!(identity.events.len(), 1);
        assert_eq!(identity.events[0].name, "UserRegistered");
    }

    #[test]
    fn test_upsert_aggregate_persists_members_and_ownership() {
        let ws = "/tmp/test-upsert-aggregate";
        let store = setup(ws);

        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "aggregate",
                "context": "Identity",
                "name": "UserAggregate",
                "description": "User consistency boundary",
                "root_entity": "User",
                "entities": ["User"],
                "value_objects": ["EmailAddress"],
                "ownership": {
                    "team": "Identity Team",
                    "owners": ["alice"],
                    "rationale": "Owns authentication"
                }
            }),
        );

        assert!(result.is_error.is_none());
        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        let aggregate = identity
            .aggregates
            .iter()
            .find(|a| a.name == "UserAggregate")
            .unwrap();
        assert_eq!(aggregate.root_entity, "User");
        assert_eq!(aggregate.entities, vec!["User"]);
        assert_eq!(aggregate.value_objects, vec!["EmailAddress"]);
        assert_eq!(aggregate.ownership.team, "Identity Team");
    }

    #[test]
    fn test_upsert_policy_merges_links() {
        let ws = "/tmp/test-upsert-policy";
        let store = setup(ws);

        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "policy",
                "context": "Identity",
                "name": "WelcomePolicy",
                "policy_kind": "process_manager",
                "triggers": ["UserRegistered"],
                "commands": ["SendWelcomeEmail"]
            }),
        );

        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "policy",
                "context": "Identity",
                "name": "WelcomePolicy",
                "commands": ["SendWelcomeEmail", "CreateAuditEntry"]
            }),
        );

        assert!(result.is_error.is_none());
        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        let policy = identity
            .policies
            .iter()
            .find(|p| p.name == "WelcomePolicy")
            .unwrap();
        assert!(matches!(policy.kind, PolicyKind::ProcessManager));
        assert_eq!(policy.triggers, vec!["UserRegistered"]);
        assert_eq!(
            policy.commands,
            vec!["SendWelcomeEmail", "CreateAuditEntry"]
        );
    }

    #[test]
    fn test_upsert_read_model_merges_fields() {
        let ws = "/tmp/test-upsert-read-model";
        let store = setup(ws);

        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "read_model",
                "context": "Identity",
                "name": "UserProfileView",
                "source": "User",
                "fields": [{"name": "id", "type": "UserId", "required": true}]
            }),
        );

        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "read_model",
                "context": "Identity",
                "name": "UserProfileView",
                "fields": [{"name": "email", "type": "String", "required": true}]
            }),
        );

        assert!(result.is_error.is_none());
        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        let read_model = identity
            .read_models
            .iter()
            .find(|rm| rm.name == "UserProfileView")
            .unwrap();
        assert_eq!(read_model.fields.len(), 2);
        assert!(read_model.fields.iter().any(|f| f.name == "id"));
        assert!(read_model.fields.iter().any(|f| f.name == "email"));
    }

    #[test]
    fn test_upsert_external_system_and_decision() {
        let ws = "/tmp/test-upsert-boundaries";
        let store = setup(ws);

        let system_result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "external_system",
                "name": "Stripe",
                "description": "Payment processor",
                "kind_label": "saas",
                "consumed_by_contexts": ["Identity"],
                "rationale": "Delegates payments"
            }),
        );
        assert!(system_result.is_error.is_none());

        let decision_result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "architectural_decision",
                "name": "ADR-001",
                "title": "Use Stripe for payments",
                "status": "accepted",
                "scope": "payments",
                "date": "2026-03-06",
                "rationale": "Reduce PCI burden",
                "contexts": ["Identity"],
                "consequences": ["External dependency introduced"]
            }),
        );
        assert!(decision_result.is_error.is_none());

        let loaded = store.load_desired(ws).unwrap().unwrap();
        assert_eq!(loaded.external_systems.len(), 1);
        assert_eq!(loaded.external_systems[0].name, "Stripe");
        assert_eq!(
            loaded.external_systems[0].consumed_by_contexts,
            vec!["Identity"]
        );
        assert_eq!(loaded.architectural_decisions.len(), 1);
        assert!(matches!(
            loaded.architectural_decisions[0].status,
            DecisionStatus::Accepted
        ));
        assert_eq!(loaded.architectural_decisions[0].contexts, vec!["Identity"]);
    }

    #[test]
    fn test_remove_expressive_elements() {
        let ws = "/tmp/test-remove-expressive";
        let store = setup(ws);

        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "aggregate", "context": "Identity", "name": "UserAggregate", "root_entity": "User"
            }),
        );
        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "external_system", "name": "Stripe"
            }),
        );

        let rm_aggregate = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "aggregate", "action": "remove", "context": "Identity", "name": "UserAggregate"
            }),
        );
        let rm_system = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "external_system", "action": "remove", "name": "Stripe"
            }),
        );

        assert!(rm_aggregate.is_error.is_none());
        assert!(rm_system.is_error.is_none());

        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        assert!(identity.aggregates.is_empty());
        assert!(loaded.external_systems.is_empty());
    }

    #[test]
    fn test_unknown_write_tool() {
        let store = test_store();
        let result = call_write_tool("/tmp/test-ws", &store, "nonexistent", &json!({}));
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn test_auto_save_on_mutation() {
        let ws = "/tmp/test-autosave";
        let store = setup(ws);
        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({"kind": "bounded_context", "name": "Billing", "description": "Billing context"}),
        );
        let loaded = store.load_desired(ws).unwrap().unwrap();
        assert_eq!(loaded.bounded_contexts.len(), 2);
    }

    #[test]
    fn test_auto_save_not_on_error() {
        let store = test_store();
        let ws = "/tmp/test-autosave-err";
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({"kind": "entity", "context": "Nonexistent", "name": "Foo"}),
        );
        assert_eq!(result.is_error, Some(true));
        assert!(store.load_desired(ws).unwrap().is_none());
    }

    #[test]
    fn test_draft_refactoring_plan_uses_baseline() {
        let ws = "/tmp/test-baseline";
        let store = setup(ws);
        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({"kind": "bounded_context", "name": "Billing", "description": "Billing context"}),
        );
        let result = call_write_tool(ws, &store, "rust_diagnose", &json!({}));
        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
        };
        let report: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(report["status"], "pending_changes");
        assert_eq!(report["provenance"]["source"], "refactor_lifecycle");
        assert_eq!(report["provenance"]["state"], "actual_history");
        assert!(report.get("proof").is_some());
        assert!(report.get("truth_maintenance").is_some());
    }

    #[test]
    fn test_draft_plan_does_not_auto_advance() {
        let ws = "/tmp/test-no-advance";
        let store = setup(ws);
        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({"kind": "bounded_context", "name": "Billing", "description": "Billing context"}),
        );
        let result = call_write_tool(ws, &store, "rust_diagnose", &json!({}));
        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
        };
        let first: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(first["status"], "pending_changes");
        let result = call_write_tool(ws, &store, "rust_diagnose", &json!({}));
        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
        };
        let second: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(second["status"], "pending_changes");
        assert_eq!(first["change_count"], second["change_count"]);
    }

    #[test]
    fn test_accept_is_actual_first_noop() {
        let ws = "/tmp/test-accept-sync";
        let store = setup(ws);
        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({"kind": "bounded_context", "name": "Billing", "description": "Billing context"}),
        );
        let result = call_write_tool(ws, &store, "rust_diagnose", &json!({"action": "accept"}));
        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
        };
        let accepted: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(accepted["status"], "actual_first_noop");
        assert_eq!(accepted["provenance"]["source"], "refactor_lifecycle");
        assert_eq!(accepted["provenance"]["state"], "actual");
        assert!(accepted.get("truth_maintenance").is_some());
        let result = call_write_tool(ws, &store, "rust_diagnose", &json!({}));
        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
        };
        let plan: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(plan["status"], "pending_changes");
        assert!(plan.get("truth_maintenance").is_some());
    }

    #[test]
    fn test_reset_is_actual_first_noop() {
        let ws = "/tmp/test-reset-wt";
        let store = setup(ws);
        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({"kind": "bounded_context", "name": "Identity", "description": "Original"}),
        );
        call_write_tool(ws, &store, "rust_diagnose", &json!({"action": "accept"}));
        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({"kind": "bounded_context", "name": "Billing", "description": "New context"}),
        );
        let desired = store.load_desired(ws).unwrap().unwrap();
        assert_eq!(desired.bounded_contexts.len(), 2);
        let result = call_write_tool(ws, &store, "rust_diagnose", &json!({"action": "reset"}));
        let text = match &result.content[0] {
            ContentBlock::Text { text } => text,
        };
        let report: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(report["status"], "actual_first_noop");
        assert_eq!(report["provenance"]["source"], "refactor_lifecycle");
        assert_eq!(report["provenance"]["state"], "actual");
        assert!(report.get("truth_maintenance").is_some());
        let reset = store.load_desired(ws).unwrap().unwrap();
        assert_eq!(reset.bounded_contexts.len(), 2);
    }

    #[test]
    fn test_update_service_merges_methods() {
        let ws = "/tmp/test-merge-methods";
        let store = setup(ws);
        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "service", "context": "Identity", "name": "AuthService",
                "service_kind": "application",
                "methods": [{"name": "login", "return_type": "Token"}]
            }),
        );
        call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({
                "kind": "service", "context": "Identity", "name": "AuthService",
                "methods": [{"name": "logout", "return_type": "void"}]
            }),
        );
        let loaded = store.load_desired(ws).unwrap().unwrap();
        let identity = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == "Identity")
            .unwrap();
        let svc = identity
            .services
            .iter()
            .find(|s| s.name == "AuthService")
            .unwrap();
        assert_eq!(svc.methods.len(), 2);
    }

    #[test]
    fn test_remove_bounded_context() {
        let ws = "/tmp/test-rm-bc";
        let store = setup(ws);
        let result = call_write_tool(
            ws,
            &store,
            "rust_annotations",
            &json!({"kind": "bounded_context", "action": "remove", "name": "Identity"}),
        );
        assert!(result.is_error.is_none());
        // After removing the only context, the model exists but has 0 contexts
        let loaded = store.load_desired(ws).unwrap().unwrap();
        assert_eq!(loaded.bounded_contexts.len(), 0);
    }

    #[test]
    fn test_missing_kind() {
        let store = test_store();
        let result = call_write_tool(
            "/tmp/test-ws",
            &store,
            "rust_annotations",
            &json!({"name": "Foo"}),
        );
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn test_diagnose_returns_structured_report() {
        let ws = "/tmp/test-diagnose";
        let store = setup(ws);

        // diagnose on a model with data
        let result = call_write_tool(ws, &store, "rust_diagnose", &json!({"action": "diagnose"}));
        assert!(result.is_error.is_none() || result.is_error == Some(false));

        let text = match &result.content[0] {
            crate::mcp::protocol::ContentBlock::Text { text } => text.clone(),
        };
        let report: serde_json::Value =
            serde_json::from_str(&text).expect("diagnose must return valid JSON");

        // Must have required top-level fields
        assert!(
            report.get("health_score").is_some(),
            "must have health_score"
        );
        assert!(report.get("invariants").is_some(), "must have invariants");
        assert!(
            report.get("practice_findings").is_some(),
            "must have practice_findings"
        );
        assert!(
            report.get("next_actions").is_some(),
            "must have next_actions"
        );
        assert!(
            report.get("has_implemented_model").is_some(),
            "must have has_implemented_model"
        );
        assert!(report.get("loop_hint").is_some(), "must have loop_hint");
        assert!(report.get("proof").is_some(), "must have proof");
        assert!(report.get("evidence").is_some(), "must have evidence");
        assert!(report.get("provenance").is_some(), "must have provenance");
        assert!(
            report.get("truth_maintenance").is_some(),
            "must have truth_maintenance"
        );

        // Invariants must have all 4 checks
        let inv = &report["invariants"];
        assert!(inv.get("circular_deps").is_some());
        assert!(inv.get("layer_violations").is_some());
        assert!(inv.get("aggregate_quality").is_some());
        assert!(inv.get("policy_violations").is_some());

        // next_actions must be an array with at least one action
        let actions = report["next_actions"]
            .as_array()
            .expect("next_actions must be array");
        assert!(
            !actions.is_empty(),
            "diagnose must suggest at least one next action"
        );

        // Each action must have priority, tool, and reason
        for action in actions {
            assert!(
                action.get("priority").is_some(),
                "action must have priority"
            );
            assert!(action.get("tool").is_some(), "action must have tool");
            assert!(action.get("reason").is_some(), "action must have reason");
        }

        assert_eq!(report["provenance"]["source"], "analysis_pipeline");
        assert_eq!(report["provenance"]["state"], "actual_history");
    }

    #[test]
    fn test_diagnose_on_empty_store() {
        let store = test_store();
        let result = call_write_tool(
            "/tmp/test-diagnose-empty",
            &store,
            "rust_diagnose",
            &json!({"action": "diagnose"}),
        );
        assert!(result.is_error.is_none() || result.is_error == Some(false));

        let text = match &result.content[0] {
            crate::mcp::protocol::ContentBlock::Text { text } => text.clone(),
        };
        let report: serde_json::Value =
            serde_json::from_str(&text).expect("diagnose must return valid JSON");

        // No current model → should suggest sync
        assert_eq!(report["has_implemented_model"], false);
        assert!(report.get("truth_maintenance").is_some());
        let actions = report["next_actions"].as_array().unwrap();
        let first = &actions[0];
        assert_eq!(
            first["tool"], "sync",
            "first action on empty store should be sync"
        );
    }
}
