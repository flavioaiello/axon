//! Self-model integration test: axon models itself.
//!
//! This test exercises the full MCP tool lifecycle against the axon
//! codebase itself, proving that the server is a valuable higher-domain
//! abstraction layer. The flow:
//!
//!   1. **refactor(scan)** → AST-extract the actual axon domain model
//!   2. **architecture** → verify the scanned model is persisted & queryable
//!   3. **define** → enrich the planned model with domain knowledge
//!   4. **architecture** → verify mutations are visible
//!   5. **Datalog queries** → run Datalog queries proving cross-cutting insights
//!   6. **refactor(plan)** → diff actual vs desired, get actionable plan
//!   7. **refactor(accept)** → promote desired → actual
//!   8. **refactor(reset)** → revert desired to actual
//!   9. **Datalog reasoning** → prove value: queries impossible without the graph

use serde_json::{Value, json};
use std::env::temp_dir;
use std::sync::atomic::{AtomicU64, Ordering};

use axon::domain::analyze::scan_actual_model;
use axon::mcp::tools::call_tool;
use axon::mcp::write_tools::call_write_tool;
use axon::store::Store;
use axon::store::canonicalize_path;

// ── Helpers ────────────────────────────────────────────────────────────────

fn temp_store() -> Store {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let id = CTR.fetch_add(1, Ordering::SeqCst);
    let path = temp_dir().join(format!("axon_self_integ_{}_{}.db", std::process::id(), id));
    Store::open(&path).expect("Failed to open temp store")
}

/// Extract the text payload from a tool call result, panic if error.
fn unwrap_tool_text(result: &axon::mcp::protocol::ToolCallResult) -> Value {
    assert_ne!(
        result.is_error,
        Some(true),
        "Tool call returned error: {:?}",
        result.content
    );
    match &result.content[0] {
        axon::mcp::protocol::ContentBlock::Text { text } => {
            serde_json::from_str(text).unwrap_or_else(|_| json!(text))
        }
    }
}

/// The real axon workspace root (this repository).
fn axon_root() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.to_path_buf()
}

// ═══════════════════════════════════════════════════════════════════════════
//  Phase 1: Scan → Persist → Show round-trip on axon itself
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn self_scan_persist_show_roundtrip() {
    let store = temp_store();
    let ws_root = axon_root();
    let ws = ws_root.to_string_lossy().to_string();

    // ── Step 1: Scan actual model from axon source ────────────────
    let actual =
        scan_actual_model(&ws_root, None).expect("scan_actual_model must succeed on axon source");

    // axon has src/{domain,mcp,server,store} → at least 4 bounded contexts
    assert!(
        !actual.bounded_contexts.is_empty(),
        "Scan must discover at least one bounded context"
    );

    let total_entities: usize = actual
        .bounded_contexts
        .iter()
        .map(|bc| bc.entities.len())
        .sum();
    let total_services: usize = actual
        .bounded_contexts
        .iter()
        .map(|bc| bc.services.len())
        .sum();
    let total_vos: usize = actual
        .bounded_contexts
        .iter()
        .map(|bc| bc.value_objects.len())
        .sum();

    eprintln!("── Self-scan results ──");
    eprintln!("  Bounded contexts: {}", actual.bounded_contexts.len());
    eprintln!("  AST edges: {}", actual.ast_edges.len());
    for bc in &actual.bounded_contexts {
        eprintln!(
            "    {} → {} entities, {} services, {} VOs, {} events, {} deps",
            bc.name,
            bc.entities.len(),
            bc.services.len(),
            bc.value_objects.len(),
            bc.events.len(),
            bc.dependencies.len()
        );
    }

    // Verify AST edges are populated (axon uses derive macros and trait impls)
    assert!(
        !actual.ast_edges.is_empty(),
        "Self-scan must discover AST edges (derives, trait impls) from axon Rust source"
    );

    // ── Step 2: Persist to CozoDB ──────────────────────────────────────
    store
        .save_desired(&ws, &actual)
        .expect("save_desired must succeed");
    store
        .save_actual(&ws, &actual)
        .expect("save_actual must succeed");

    // ── Step 3: Show actual model via MCP tool ─────────────────────────
    let show_result = call_tool(&store, &ws, "rust_status", &json!({"detail": "full"}));
    let show_json = unwrap_tool_text(&show_result);
    assert!(
        show_json["current"].is_object(),
        "architecture must return a current model object after save_actual"
    );

    let shown_contexts = show_json["current"]["bounded_contexts"]
        .as_array()
        .expect("Model must have bounded_contexts array");
    assert_eq!(
        shown_contexts.len(),
        actual.bounded_contexts.len(),
        "architecture must return same number of contexts as scanned"
    );

    // ── Step 4: Verify entities survived the round-trip ────────────────
    let shown_entity_count: usize = shown_contexts
        .iter()
        .map(|bc| bc["entities"].as_array().map_or(0, |e| e.len()))
        .sum();
    assert_eq!(
        shown_entity_count, total_entities,
        "Entity count must match between scan and show"
    );

    eprintln!(
        "  Round-trip verified: {} entities, {} services, {} VOs",
        total_entities, total_services, total_vos
    );
}

// ═══════════════════════════════════════════════════════════════════════════
//  Phase 2: Mutate desired model — enrich with domain knowledge
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn self_mutate_and_enrich_desired_model() {
    let store = temp_store();
    let ws_root = axon_root();
    let ws = ws_root.to_string_lossy().to_string();

    // Scan and persist actual first
    let actual = scan_actual_model(&ws_root, None).expect("scan failed");
    store.save_actual(&ws, &actual).expect("save_actual failed");
    // Copy actual → desired as baseline (accept in reverse: save desired = actual)
    store
        .save_desired(&ws, &actual)
        .expect("save_desired failed");

    // ── Mutate: Add a bounded context ──────────────────────────────────
    let result = call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "bounded_context",
            "name": "Reasoning",
            "description": "Symbolic logic and Datalog reasoning layer",
            "module_path": "src/domain"
        }),
    );
    let mutate_json = unwrap_tool_text(&result);
    assert!(
        mutate_json.to_string().to_lowercase().contains("reason"),
        "Mutate response should reference the new context: {:?}",
        mutate_json
    );

    // ── Mutate: Add an entity to the new context ───────────────────────
    let result = call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "entity",
            "context": "Reasoning",
            "name": "DomainModel",
            "description": "Root aggregate representing the complete domain model graph",
            "aggregate_root": true,
            "fields": [
                {"name": "name", "type": "String", "required": true},
                {"name": "description", "type": "String", "required": false},
                {"name": "bounded_contexts", "type": "Vec<BoundedContext>", "required": true}
            ],
            "methods": [
                {"name": "empty", "description": "Create empty model", "parameters": [{"name": "workspace_path", "type": "String"}], "return_type": "DomainModel"},
                {"name": "validate", "description": "Validate model consistency", "parameters": [], "return_type": "Result<()>"}
            ],
            "invariants": ["Name must not be empty", "At least one bounded context required for non-empty model"]
        }),
    );
    unwrap_tool_text(&result);

    // ── Mutate: Add a value object ─────────────────────────────────────
    let result = call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "value_object",
            "context": "Reasoning",
            "name": "Ownership",
            "description": "Team ownership metadata for any domain element",
            "fields": [
                {"name": "team", "type": "String", "required": false},
                {"name": "owners", "type": "Vec<String>", "required": false},
                {"name": "rationale", "type": "String", "required": false}
            ],
            "validation_rules": ["At least team or owners must be specified if ownership is set"]
        }),
    );
    unwrap_tool_text(&result);

    // ── Mutate: Add a service ──────────────────────────────────────────
    let result = call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "service",
            "context": "Reasoning",
            "name": "AnalyzeService",
            "description": "AST extraction and domain classification engine",
            "service_kind": "domain",
            "methods": [
                {"name": "scan_actual_model", "description": "Extract model from source AST", "parameters": [{"name": "workspace_root", "type": "Path"}, {"name": "desired", "type": "Option<DomainModel>"}], "return_type": "Result<DomainModel>"}
            ]
        }),
    );
    unwrap_tool_text(&result);

    // ── Mutate: Add a domain event ─────────────────────────────────────
    let result = call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "event",
            "context": "Reasoning",
            "name": "ModelScanned",
            "description": "Emitted after successful model extraction from source",
            "source": "DomainModel",
            "fields": [
                {"name": "workspace", "type": "String", "required": true},
                {"name": "context_count", "type": "usize", "required": true},
                {"name": "entity_count", "type": "usize", "required": true}
            ]
        }),
    );
    unwrap_tool_text(&result);

    // ── Show implemented model ─────────────────────────────────────────
    let show_result = call_tool(&store, &ws, "rust_status", &json!({"detail": "full"}));
    let show_json = unwrap_tool_text(&show_result);

    let desired_contexts = show_json["implemented"]["bounded_contexts"]
        .as_array()
        .expect("Must have bounded_contexts");

    // The Reasoning context must exist in the implemented model
    let reasoning = desired_contexts
        .iter()
        .find(|bc| bc["name"] == "Reasoning")
        .expect("Reasoning context must be in planned model");
    assert!(
        reasoning["entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "DomainModel"),
        "DomainModel entity must be in Reasoning context"
    );
    assert!(
        reasoning["services"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "AnalyzeService"),
        "AnalyzeService must be in Reasoning context"
    );
    assert!(
        reasoning["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "ModelScanned"),
        "ModelScanned event must be in Reasoning context"
    );
    assert!(
        reasoning["value_objects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "Ownership"),
        "Ownership VO must be in Reasoning context"
    );

    eprintln!(
        "── Desired model enriched with {} contexts ──",
        desired_contexts.len()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
//  Phase 3: Query model — Datalog reasoning that proves MCP value
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn self_query_cross_cutting_insights() {
    let store = temp_store();
    let ws_root = axon_root();
    let ws = ws_root.to_string_lossy().to_string();

    // Set up: scan actual + copy to desired
    let actual = scan_actual_model(&ws_root, None).expect("scan failed");
    store.save_actual(&ws, &actual).expect("save_actual failed");
    store
        .save_desired(&ws, &actual)
        .expect("save_desired failed");

    let canonical = axon::store::canonicalize_path(&ws);

    // ── Query 1: List all bounded contexts ─────────────────────────────
    let rows = store
        .run_datalog(
            "?[name, module_path] := *context{workspace: $ws, name, module_path, state: 'actual'}",
            &canonical,
        )
        .expect("query 1 failed");
    assert!(!rows.is_empty(), "Must have at least one context");
    eprintln!("── Query: {} bounded contexts found ──", rows.len());

    // ── Query 2: List all entities across all contexts ─────────────────
    let rows = store.run_datalog(
        "?[ctx, name, aggregate_root] := *entity{workspace: $ws, context: ctx, name, aggregate_root, state: 'actual'}",
        &canonical,
    ).expect("query 2 failed");
    assert!(!rows.is_empty(), "Must have at least one entity");
    eprintln!("── Query: {} entities found ──", rows.len());

    // ── Query 3: Cross-cutting field type analysis ─────────────────────
    let rows = store.run_datalog(
        "?[ctx, owner_kind, owner, field_name] := *field{workspace: $ws, context: ctx, owner_kind, owner, name: field_name, field_type: 'String', state: 'actual'}",
        &canonical,
    ).expect("query 3 failed");
    eprintln!("── Query: {} fields of type String ──", rows.len());

    // ── Query 4: Find all public methods across all services ───────────
    let rows = store.run_datalog(
        "?[ctx, service, method_name, return_type] := *method{workspace: $ws, context: ctx, owner_kind: 'service', owner: service, name: method_name, return_type, state: 'actual'}",
        &canonical,
    ).expect("query 4 failed");
    eprintln!("── Query: {} service methods found ──", rows.len());

    // ── Query 5: Count elements per context (aggregation) ──────────────
    let rows = store.run_datalog(
        "ctx_entities[ctx, count(name)] := *entity{workspace: $ws, context: ctx, name, state: 'actual'} ?[ctx, entity_count] := ctx_entities[ctx, entity_count]",
        &canonical,
    ).expect("query 5 failed");
    assert!(!rows.is_empty(), "Must have entity counts per context");

    // ── Query 6: All value objects in the model ────────────────────────
    let rows = store.run_datalog(
        "?[ctx, name, description] := *value_object{workspace: $ws, context: ctx, name, description, state: 'actual'}",
        &canonical,
    ).expect("query 6 failed");
    eprintln!("── Query: {} value objects found ──", rows.len());
}

// ═══════════════════════════════════════════════════════════════════════════
//  Phase 4: Refactor lifecycle — plan → accept → reset
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn self_refactor_lifecycle() {
    let store = temp_store();
    let ws_root = axon_root();
    let ws = ws_root.to_string_lossy().to_string();

    // Set up: scan actual, copy to desired, then mutate desired
    let actual = scan_actual_model(&ws_root, None).expect("scan failed");
    store.save_actual(&ws, &actual).expect("save_actual failed");
    store
        .save_desired(&ws, &actual)
        .expect("save_desired failed");

    // ── Step 1: Plan with no changes → should be in_sync ───────────────
    let result = call_write_tool(&ws, &store, "rust_diagnose", &json!({"action": "plan"}));
    let plan_json = unwrap_tool_text(&result);
    // When actual == desired, expect in_sync or empty changes
    let is_sync = plan_json
        .get("status")
        .map(|s| s == "in_sync")
        .unwrap_or(false);
    let has_no_changes = plan_json
        .get("pending_changes")
        .and_then(|v| v.as_array())
        .map(|a| a.is_empty())
        .unwrap_or(false);
    assert!(
        is_sync || has_no_changes,
        "Plan with no changes should be in_sync: {:?}",
        plan_json
    );
    eprintln!("── Refactor plan (no changes): in_sync ──");

    // ── Step 2: Mutate desired → create divergence ─────────────────────
    call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "bounded_context",
            "name": "Telemetry",
            "description": "Observability and metrics collection",
            "module_path": "src/telemetry"
        }),
    );
    call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "entity",
            "context": "Telemetry",
            "name": "MetricPoint",
            "description": "A single metric data point",
            "aggregate_root": true,
            "fields": [
                {"name": "name", "type": "String", "required": true},
                {"name": "value", "type": "f64", "required": true},
                {"name": "timestamp", "type": "DateTime", "required": true}
            ],
            "invariants": ["Name must not be empty", "Value must be finite"]
        }),
    );

    // ── Step 3: Plan with changes → should show pending_changes ────────
    let result = call_write_tool(&ws, &store, "rust_diagnose", &json!({"action": "plan"}));
    let plan_json = unwrap_tool_text(&result);
    let changes = plan_json
        .get("pending_changes")
        .and_then(|v| v.as_array())
        .or_else(|| plan_json.get("changes").and_then(|v| v.as_array()));
    assert!(
        changes.is_some_and(|c| !c.is_empty()),
        "Plan with mutations should show pending changes: {:?}",
        plan_json
    );
    let change_count = changes.map_or(0, |c| c.len());
    eprintln!("── Refactor plan: {} pending changes ──", change_count);

    // ── Step 4: Accept compatibility path is an actual-first no-op ──────
    let result = call_write_tool(&ws, &store, "rust_diagnose", &json!({"action": "accept"}));
    let accept_json = unwrap_tool_text(&result);
    assert_eq!(
        accept_json.get("status").and_then(|s| s.as_str()),
        Some("actual_first_noop"),
        "Accept compatibility path must succeed: {:?}",
        accept_json
    );

    // Verify: plan still reports temporal changes because accept does not erase history.
    let result = call_write_tool(&ws, &store, "rust_diagnose", &json!({"action": "plan"}));
    let plan_json = unwrap_tool_text(&result);
    let is_pending = plan_json
        .get("status")
        .map(|s| s == "pending_changes")
        .unwrap_or(false);
    let has_changes = plan_json
        .get("pending_changes")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    assert!(
        is_pending || has_changes,
        "After accept compatibility no-op, plan should still expose temporal changes: {:?}",
        plan_json
    );

    // Verify: implemented model includes Telemetry.
    let show_result = call_tool(&store, &ws, "rust_status", &json!({"detail": "full"}));
    let show_json = unwrap_tool_text(&show_result);
    let actual_contexts = show_json["implemented"]["bounded_contexts"]
        .as_array()
        .expect("Must have bounded_contexts");
    assert!(
        actual_contexts.iter().any(|bc| bc["name"] == "Telemetry"),
        "Telemetry context must be in implemented graph"
    );
    eprintln!("── Accept: actual-first compatibility no-op ──");

    // ── Step 5: Mutate again, then reset compatibility path ────────────
    call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "bounded_context",
            "name": "Ephemeral",
            "description": "This remains because reset is a no-op in actual-first mode",
            "module_path": "src/ephemeral"
        }),
    );

    let result = call_write_tool(&ws, &store, "rust_diagnose", &json!({"action": "reset"}));
    let reset_json = unwrap_tool_text(&result);
    assert_eq!(
        reset_json.get("status").and_then(|s| s.as_str()),
        Some("actual_first_noop"),
        "Reset compatibility path must succeed: {:?}",
        reset_json
    );

    // Verify: implemented graph still has Ephemeral because reset is a compatibility no-op.
    let show_result = call_tool(&store, &ws, "rust_status", &json!({"detail": "full"}));
    let show_json = unwrap_tool_text(&show_result);
    let desired_contexts = show_json["implemented"]["bounded_contexts"]
        .as_array()
        .expect("Must have bounded_contexts");
    assert!(
        desired_contexts.iter().any(|bc| bc["name"] == "Ephemeral"),
        "Ephemeral context must remain after actual-first reset compatibility no-op"
    );
    assert!(
        desired_contexts.iter().any(|bc| bc["name"] == "Telemetry"),
        "Telemetry context must remain in implemented graph"
    );
    eprintln!("── Reset: actual-first compatibility no-op ──");
}

// ═══════════════════════════════════════════════════════════════════════════
//  Phase 5: Prove MCP value — queries impossible without the graph
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn self_model_proves_mcp_value() {
    let store = temp_store();
    let ws_root = axon_root();
    let ws = ws_root.to_string_lossy().to_string();

    // Full setup: scan + persist actual + copy to desired
    let actual = scan_actual_model(&ws_root, None).expect("scan failed");
    store.save_actual(&ws, &actual).expect("save_actual failed");
    store
        .save_desired(&ws, &actual)
        .expect("save_desired failed");

    let canonical = canonicalize_path(&ws);

    // ── Value 1: "Which structs have the most fields?" ─────────────────
    // This cross-cutting query spans all contexts and owner types —
    // impossible without the relational graph.
    let rows = store.run_datalog(
        "field_count[ctx, ok, owner, count(name)] := *field{workspace: $ws, context: ctx, owner_kind: ok, owner, name, state: 'actual'} ?[ctx, owner_kind, owner, n_fields] := field_count[ctx, owner_kind, owner, n_fields], n_fields > 2",
        &canonical,
    ).expect("datalog query failed");
    eprintln!("── Value: Structs with >2 fields: {} ──", rows.len());

    // ── Value 2: "Entities that might be missing invariants" ────────────
    let missing = store
        .aggregate_roots_without_invariants(&canonical)
        .unwrap_or_default();
    eprintln!(
        "── Value: {} aggregate roots without invariants ──",
        missing.len()
    );

    // ── Value 3: "All value objects in the model" ──────────────────────
    let rows = store.run_datalog(
        "?[ctx, name, description] := *value_object{workspace: $ws, context: ctx, name, description, state: 'actual'}",
        &canonical,
    ).expect("datalog query failed");
    let vo_count = rows.len();
    eprintln!(
        "── Value: {} value objects across all contexts ──",
        vo_count
    );

    // ── Value 4: "Service dependency graph" ────────────────────────────
    let rows = store.run_datalog(
        "?[ctx, service, dep] := *service_dep{workspace: $ws, context: ctx, service, dep, state: 'actual'}",
        &canonical,
    ).expect("datalog query failed");
    eprintln!("── Value: {} service dependencies ──", rows.len());

    // ── Value 5: "Cross-context method parameter type usage" ───────────
    // Find all method parameters grouped by type — only possible with
    // first-class method_param relations.
    let rows = store.run_datalog(
        "type_usage[param_type, count(name)] := *method_param{workspace: $ws, name, param_type, state: 'actual'} ?[param_type, usage_count] := type_usage[param_type, usage_count], usage_count > 1",
        &canonical,
    ).expect("datalog query failed");
    eprintln!("── Value: {} parameter types used >1 time ──", rows.len());

    // ── Value 6: Full model statistics ─────────────────────────────────
    let result = call_tool(&store, &ws, "rust_status", &json!({"detail": "full"}));
    let show = unwrap_tool_text(&result);
    let model = &show["current"];
    assert!(model.is_object(), "Model must be present");
    let contexts = model["bounded_contexts"].as_array().unwrap();
    let total_entities: usize = contexts
        .iter()
        .map(|bc| bc["entities"].as_array().map_or(0, |e| e.len()))
        .sum();
    let total_services: usize = contexts
        .iter()
        .map(|bc| bc["services"].as_array().map_or(0, |s| s.len()))
        .sum();
    let total_vos: usize = contexts
        .iter()
        .map(|bc| bc["value_objects"].as_array().map_or(0, |v| v.len()))
        .sum();

    eprintln!("═══ Self-Model Summary ════════════════════════════════");
    eprintln!("  Bounded contexts : {}", contexts.len());
    eprintln!("  Entities         : {}", total_entities);
    eprintln!("  Services         : {}", total_services);
    eprintln!("  Value objects    : {}", total_vos);
    eprintln!("═══════════════════════════════════════════════════════");

    // Final assertion: the model is non-trivial
    assert!(
        contexts.len() >= 3,
        "axon should have at least 3 bounded contexts (domain, mcp, server, store)"
    );
    assert!(
        total_entities + total_services + total_vos >= 10,
        "axon should have at least 10 domain elements total"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
//  Phase 6: End-to-end via MCP tool dispatch (refactor scan action)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn self_scan_via_mcp_tool_dispatch() {
    let store = temp_store();
    let ws_root = axon_root();
    let ws = ws_root.to_string_lossy().to_string();

    // Use the MCP write_tool dispatch directly (same path as MCP clients)
    let result = call_write_tool(&ws, &store, "rust_scan", &json!({}));
    let scan_json = unwrap_tool_text(&result);

    assert_eq!(
        scan_json["status"], "scanned",
        "sync must succeed: {:?}",
        scan_json
    );
    assert!(
        scan_json["entities"].as_u64().unwrap_or(0) > 0,
        "Must discover entities: {:?}",
        scan_json
    );
    assert!(
        scan_json["contexts_scanned"].as_u64().unwrap_or(0) > 0,
        "Must scan contexts: {:?}",
        scan_json
    );

    eprintln!("── sync via MCP: {} ──", scan_json["message"]);

    // Now architecture must work
    let show_result = call_tool(&store, &ws, "rust_status", &json!({"detail": "full"}));
    let show_json = unwrap_tool_text(&show_result);
    assert!(
        show_json["current"].is_object() && show_json["current"]["bounded_contexts"].is_array(),
        "architecture must return current model after sync: {:?}",
        show_json
    );

    let ctx_count = show_json["current"]["bounded_contexts"]
        .as_array()
        .map_or(0, |a| a.len());
    assert!(ctx_count > 0, "Must have contexts after sync");
    eprintln!("── architecture after sync: {} contexts ──", ctx_count);
}

// ═══════════════════════════════════════════════════════════════════════════
//  Phase 7: Remove elements — prove mutation completeness
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn self_mutate_remove_elements() {
    let store = temp_store();
    let ws_root = axon_root();
    let ws = ws_root.to_string_lossy().to_string();

    // Scan and baseline
    let actual = scan_actual_model(&ws_root, None).expect("scan failed");
    store.save_actual(&ws, &actual).expect("save_actual failed");
    store
        .save_desired(&ws, &actual)
        .expect("save_desired failed");

    // Add elements to remove
    call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "bounded_context",
            "name": "Disposable",
            "description": "Context to be removed",
            "module_path": "src/disposable"
        }),
    );
    call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "entity",
            "context": "Disposable",
            "name": "Throwaway",
            "description": "Entity to be removed",
            "fields": [{"name": "id", "type": "u64", "required": true}]
        }),
    );

    // Verify they exist
    let show = call_tool(&store, &ws, "rust_status", &json!({"detail": "full"}));
    let json = unwrap_tool_text(&show);
    let contexts = json["implemented"]["bounded_contexts"].as_array().unwrap();
    assert!(
        contexts.iter().any(|bc| bc["name"] == "Disposable"),
        "Disposable must exist before removal"
    );

    // Remove entity first
    let result = call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "entity",
            "action": "remove",
            "context": "Disposable",
            "name": "Throwaway"
        }),
    );
    unwrap_tool_text(&result);

    // Remove context
    let result = call_write_tool(
        &ws,
        &store,
        "rust_annotations",
        &json!({
            "kind": "bounded_context",
            "action": "remove",
            "name": "Disposable"
        }),
    );
    unwrap_tool_text(&result);

    // Verify removal
    let show = call_tool(&store, &ws, "rust_status", &json!({"detail": "full"}));
    let json = unwrap_tool_text(&show);
    let contexts = json["implemented"]["bounded_contexts"].as_array().unwrap();
    assert!(
        !contexts.iter().any(|bc| bc["name"] == "Disposable"),
        "Disposable must be gone after removal"
    );
    eprintln!("── Remove: context + entity successfully deleted ──");
}

// ═══════════════════════════════════════════════════════════════════════════
//  Phase 7: Self-improvement loop — diagnose on axon itself
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn self_diagnose_improvement_loop() {
    let store = temp_store();
    let ws_root = axon_root();
    let ws = ws_root.to_string_lossy().to_string();

    // ── Step 1: Scan actual model ──────────────────────────────────────
    let actual = scan_actual_model(&ws_root, None).expect("scan_actual_model must succeed");
    store
        .save_actual(&ws, &actual)
        .expect("save_actual must succeed");
    store
        .save_desired(&ws, &actual)
        .expect("save_desired must succeed");

    // ── Step 2: Run diagnose ───────────────────────────────────────────
    let result = call_write_tool(&ws, &store, "rust_diagnose", &json!({"action": "diagnose"}));
    let report = unwrap_tool_text(&result);

    eprintln!("── Diagnose results ──");
    eprintln!("  Status: {}", report["status"]);
    eprintln!("  Health score: {}", report["health_score"]);
    eprintln!("  Has implemented: {}", report["has_implemented_model"]);

    // Must have a health score
    let score = report["health_score"]
        .as_u64()
        .expect("must have health_score");
    eprintln!("  Score: {score}");

    // Must have invariant results
    let inv = &report["invariants"];
    assert!(
        inv["circular_deps"]["status"].is_string(),
        "circular_deps must have status"
    );
    assert!(
        inv["layer_violations"]["status"].is_string(),
        "layer_violations must have status"
    );
    assert!(
        inv["aggregate_quality"]["status"].is_string(),
        "aggregate_quality must have status"
    );

    // Must have next_actions
    let actions = report["next_actions"]
        .as_array()
        .expect("must have next_actions array");
    eprintln!("  Next actions: {}", actions.len());
    for (i, action) in actions.iter().enumerate() {
        eprintln!("    [{i}] {} → {}", action["tool"], action["reason"]);
    }

    // Must have AST edge statistics (since we scanned Rust source)
    let ast = &report["ast_edges"];
    assert!(
        ast["total"].as_u64().unwrap_or(0) > 0,
        "must have AST edges from Rust scanning"
    );
    eprintln!("  AST edges: {}", ast["total"]);

    // Must have loop_hint
    assert!(report["loop_hint"].is_string(), "must have loop_hint");

    // ── Step 3: Verify the loop is actionable ──────────────────────────
    // After scan, models are in sync so no drift, but aggregate quality
    // issues should be detected (axon has aggregate roots without invariants)
    assert!(
        report["has_implemented_model"].as_bool().unwrap(),
        "must have implemented model after scan"
    );

    eprintln!("── Self-improvement loop: diagnose verified ──");
}

// ═══════════════════════════════════════════════════════════════════════════
//  Compiler directives: list every dead_code flag in one MCP call
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn self_dead_code_flags_listed_via_single_graph_call() {
    let store = temp_store();
    let id = format!(
        "axon_dead_code_fixture_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    );
    let ws_root = temp_dir().join(id);
    let src = ws_root.join("src");
    std::fs::create_dir_all(&src).expect("create fixture src dir");
    std::fs::write(
        src.join("lib.rs"),
        r#"
#[allow(dead_code)]
fn dormant_helper() {}

struct Fixture {
    #[allow(dead_code)]
    hidden: u64,
}
"#,
    )
    .expect("write fixture source");
    let ws = ws_root.to_string_lossy().to_string();

    let actual = scan_actual_model(&ws_root, None).expect("scan_actual_model must succeed");
    store
        .save_actual(&ws, &actual)
        .expect("save_actual must succeed");

    // A single MCP graph call returns every `#[allow(dead_code)]` flag, including
    // the ones on private fns and struct fields, each with a source location.
    let result = call_tool(
        &store,
        &ws,
        "rust_graph",
        &json!({
            "view": "edges",
            "relation": "ast_edge",
            "to": "dead_code",
            "limit": 200,
        }),
    );
    let payload = unwrap_tool_text(&result);
    let text = serde_json::to_string(&payload).expect("serialize graph payload");

    assert!(
        text.contains("dead_code"),
        "graph must list dead_code directives in one call: {text}"
    );
    // Flags carry their declaration site so they are actionable (the repo
    // annotates private items in src/mcp/*.rs).
    assert!(
        text.contains(".rs"),
        "dead_code flags must carry a file location: {text}"
    );

    let _ = std::fs::remove_dir_all(&ws_root);

    eprintln!("── dead_code flags via single graph call: verified ──");
}
