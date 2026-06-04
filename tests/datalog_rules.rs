//! Unit-level tests for Datalog reasoning rules.
//!
//! These tests exercise the core graph-analytic and constraint-checking
//! Datalog queries in isolation: transitive closure, cycle detection,
//! layer violations, policy violations, deletion safety, impact analysis,
//! graph algorithms (PageRank, community detection, betweenness centrality,
//! degree centrality, topological order), orphan/god context detection,
//! dependency paths, model health, drift computation, and search.

use std::env::temp_dir;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use axon::domain::model::*;
use axon::domain::rust_analyzer::ResolvedCall;
use axon::store::Store;
use axon::store::{canonicalize_path, default_layer_constraints};

// ── Helpers ────────────────────────────────────────────────────────────────

fn temp_store() -> Store {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let id = CTR.fetch_add(1, Ordering::SeqCst);
    let path = temp_dir().join(format!("axon_datalog_{}_{}.db", std::process::id(), id));
    Store::open(&path).unwrap()
}

fn ws() -> String {
    format!("/tmp/datalog-{}", std::process::id())
}

fn empty_ctx(name: &str) -> BoundedContext {
    BoundedContext {
        name: name.into(),
        description: "".into(),
        module_path: format!("src/{}", name.to_lowercase()),
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
    }
}

fn empty_model() -> DomainModel {
    DomainModel {
        name: "Test".into(),
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
        call_edges: vec![],
        reference_edges: vec![],
    }
}

fn canonicalize(path: &str) -> String {
    canonicalize_path(path)
}

/// Create a unique temp directory that looks like a crate root (has a
/// `Cargo.toml`), so `Store::open` enables `.axon/policy.json` persistence and
/// keys all operations under the same canonical workspace.
fn unique_crate_root(prefix: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), unique));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"axon-policy-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    root
}

// ═══════════════════════════════════════════════════════════════════════════
//  1. Transitive dependency closure
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn transitive_deps_linear_chain() {
    // A → B → C → D: transitive_deps(A) = {B, C, D}
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into()];
    let mut b = empty_ctx("B");
    b.dependencies = vec!["C".into()];
    let mut c = empty_ctx("C");
    c.dependencies = vec!["D".into()];
    let d = empty_ctx("D");
    model.bounded_contexts = vec![a, b, c, d];
    store.save_desired(&ws, &model).unwrap();

    let canon = canonicalize(&ws);
    let deps = store.transitive_deps(&canon, "A").unwrap();
    assert_eq!(deps.len(), 3, "A should transitively reach B, C, D");
    assert!(deps.contains(&"B".to_string()));
    assert!(deps.contains(&"C".to_string()));
    assert!(deps.contains(&"D".to_string()));
}

#[test]
fn transitive_deps_diamond() {
    // A → B, A → C, B → D, C → D: transitive_deps(A) = {B, C, D}
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into(), "C".into()];
    let mut b = empty_ctx("B");
    b.dependencies = vec!["D".into()];
    let mut c = empty_ctx("C");
    c.dependencies = vec!["D".into()];
    let d = empty_ctx("D");
    model.bounded_contexts = vec![a, b, c, d];
    store.save_desired(&ws, &model).unwrap();

    let canon = canonicalize(&ws);
    let deps = store.transitive_deps(&canon, "A").unwrap();
    assert_eq!(
        deps.len(),
        3,
        "Diamond: A should reach B, C, D (no duplicates)"
    );

    // Leaf node has no transitive deps
    let leaf_deps = store.transitive_deps(&canon, "D").unwrap();
    assert!(leaf_deps.is_empty(), "Leaf D should have no deps");
}

#[test]
fn transitive_deps_isolated_node() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    model.bounded_contexts = vec![empty_ctx("Alone")];
    store.save_desired(&ws, &model).unwrap();

    let deps = store.transitive_deps(&canonicalize(&ws), "Alone").unwrap();
    assert!(deps.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════
//  2. Cycle detection
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn circular_deps_direct_pair() {
    // A → B, B → A
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into()];
    let mut b = empty_ctx("B");
    b.dependencies = vec!["A".into()];
    model.bounded_contexts = vec![a, b];
    store.save_desired(&ws, &model).unwrap();

    let cycles = store.circular_deps(&canonicalize(&ws)).unwrap();
    assert!(!cycles.is_empty(), "Direct cycle A↔B must be detected");
    // Both (A,B) and (B,A) should appear
    assert!(cycles.contains(&("A".into(), "B".into())));
    assert!(cycles.contains(&("B".into(), "A".into())));
}

#[test]
fn circular_deps_self_loop() {
    // A → A (self-loop)
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["A".into()];
    model.bounded_contexts = vec![a];
    store.save_desired(&ws, &model).unwrap();

    let cycles = store.circular_deps(&canonicalize(&ws)).unwrap();
    assert!(!cycles.is_empty(), "Self-loop must be detected");
    assert!(cycles.contains(&("A".into(), "A".into())));
}

#[test]
fn circular_deps_three_hop() {
    // A → B → C → A
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into()];
    let mut b = empty_ctx("B");
    b.dependencies = vec!["C".into()];
    let mut c = empty_ctx("C");
    c.dependencies = vec!["A".into()];
    model.bounded_contexts = vec![a, b, c];
    store.save_desired(&ws, &model).unwrap();

    let cycles = store.circular_deps(&canonicalize(&ws)).unwrap();
    assert!(
        cycles.len() >= 3,
        "3-hop cycle A→B→C→A must produce mutual reachability pairs"
    );
}

#[test]
fn circular_deps_none_in_dag() {
    // A → B → C (DAG, no cycle)
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into()];
    let mut b = empty_ctx("B");
    b.dependencies = vec!["C".into()];
    let c = empty_ctx("C");
    model.bounded_contexts = vec![a, b, c];
    store.save_desired(&ws, &model).unwrap();

    let cycles = store.circular_deps(&canonicalize(&ws)).unwrap();
    assert!(cycles.is_empty(), "DAG should have no cycles");
}

// ═══════════════════════════════════════════════════════════════════════════
//  3. Layer violations
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn layer_violations_domain_depends_on_infra() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut ctx = empty_ctx("Payments");
    ctx.services = vec![
        Service {
            name: "PaymentProcessor".into(),
            description: "".into(),
            kind: ServiceKind::Domain,
            methods: vec![],
            dependencies: vec!["StripeGateway".into()],
            file_path: None,
            start_line: None,
            end_line: None,
        },
        Service {
            name: "StripeGateway".into(),
            description: "".into(),
            kind: ServiceKind::Infrastructure,
            methods: vec![],
            dependencies: vec![],
            file_path: None,
            start_line: None,
            end_line: None,
        },
    ];
    model.bounded_contexts = vec![ctx];
    store.save_desired(&ws, &model).unwrap();

    let violations = store.layer_violations(&canonicalize(&ws)).unwrap();
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].0, "Payments");
    assert_eq!(violations[0].1, "PaymentProcessor");
    assert_eq!(violations[0].2, "StripeGateway");
}

#[test]
fn layer_violations_none_when_clean() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut ctx = empty_ctx("Clean");
    ctx.services = vec![
        Service {
            name: "DomainSvc".into(),
            description: "".into(),
            kind: ServiceKind::Domain,
            methods: vec![],
            dependencies: vec![],
            file_path: None,
            start_line: None,
            end_line: None,
        },
        Service {
            name: "InfraSvc".into(),
            description: "".into(),
            kind: ServiceKind::Infrastructure,
            methods: vec![],
            dependencies: vec!["DomainSvc".into()],
            file_path: None,
            start_line: None,
            end_line: None,
        },
    ];
    model.bounded_contexts = vec![ctx];
    store.save_desired(&ws, &model).unwrap();

    let violations = store.layer_violations(&canonicalize(&ws)).unwrap();
    assert!(violations.is_empty(), "Infra depending on domain is fine");
}

// ═══════════════════════════════════════════════════════════════════════════
//  4. Policy violations
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn policy_violation_forbidden_layer_dependency() {
    let store = temp_store();
    let ws = ws();
    let canon = canonicalize(&ws);
    let mut model = empty_model();
    let mut app = empty_ctx("App");
    app.dependencies = vec!["Infra".into()];
    let infra = empty_ctx("Infra");
    model.bounded_contexts = vec![app, infra];
    store.save_desired(&ws, &model).unwrap();

    store
        .upsert_layer_assignment(&canon, "App", "application")
        .unwrap();
    store
        .upsert_layer_assignment(&canon, "Infra", "infrastructure")
        .unwrap();
    store
        .upsert_dependency_constraint(
            &canon,
            "layer",
            "application",
            "infrastructure",
            "forbidden",
        )
        .unwrap();

    let result = store.evaluate_policy_violations(&canon).unwrap();
    assert_eq!(result["status"], "false", "Should have violations");
    let violations = result["violations"].as_array().unwrap();
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0]["from_context"], "App");
    assert_eq!(violations[0]["to_context"], "Infra");
}

#[test]
fn policy_violation_forbidden_context_dependency() {
    let store = temp_store();
    let ws = ws();
    let canon = canonicalize(&ws);
    let mut model = empty_model();
    let mut a = empty_ctx("Auth");
    a.dependencies = vec!["Payments".into()];
    let p = empty_ctx("Payments");
    model.bounded_contexts = vec![a, p];
    store.save_desired(&ws, &model).unwrap();

    store
        .upsert_dependency_constraint(&canon, "context", "Auth", "Payments", "forbidden")
        .unwrap();

    let result = store.evaluate_policy_violations(&canon).unwrap();
    assert_eq!(result["status"], "false");
    let violations = result["violations"].as_array().unwrap();
    assert!(
        violations
            .iter()
            .any(|v| v["from_context"] == "Auth" && v["to_context"] == "Payments")
    );
}

#[test]
fn policy_violation_none_when_no_forbidden() {
    let store = temp_store();
    let ws = ws();
    let canon = canonicalize(&ws);
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into()];
    let b = empty_ctx("B");
    model.bounded_contexts = vec![a, b];
    store.save_desired(&ws, &model).unwrap();

    let result = store.evaluate_policy_violations(&canon).unwrap();
    assert_eq!(result["status"], "unconfigured");
    assert_eq!(result["configured"], false);
    assert_eq!(result["count"], 0);
    assert_eq!(result["policy_coverage"]["dependency_constraint_count"], 0);
}

#[test]
fn policy_violation_none_when_policy_configured() {
    let store = temp_store();
    let ws = ws();
    let canon = canonicalize(&ws);
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into()];
    let b = empty_ctx("B");
    model.bounded_contexts = vec![a, b];
    store.save_desired(&ws, &model).unwrap();

    store
        .upsert_layer_assignment(&canon, "A", "domain")
        .unwrap();
    store
        .upsert_layer_assignment(&canon, "B", "infrastructure")
        .unwrap();
    store
        .upsert_dependency_constraint(&canon, "layer", "infrastructure", "domain", "forbidden")
        .unwrap();

    let result = store.evaluate_policy_violations(&canon).unwrap();
    assert_eq!(result["status"], "true");
    assert_eq!(result["configured"], true);
    assert_eq!(result["count"], 0);
}

#[test]
fn policy_assignments_survive_store_reopen_for_crate_root() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = temp_dir().join(format!(
        "axon_policy_reopen_{}_{}",
        std::process::id(),
        unique
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"policy-reopen\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    let ws = canonicalize_path(&root.to_string_lossy());

    {
        let store = Store::open(&root).unwrap();
        // "A" matches no layer convention, so this is a genuine explicit override.
        store.upsert_layer_assignment(&ws, "A", "domain").unwrap();
        // A non-default constraint that must round-trip across reopen.
        store
            .upsert_dependency_constraint(&ws, "context", "Auth", "Billing", "forbidden")
            .unwrap();
    }

    let reopened = Store::open(&root).unwrap();
    assert_eq!(
        reopened.list_layer_assignments(&ws).unwrap(),
        vec![("A".into(), "domain".into())]
    );

    let constraints = reopened.list_dependency_constraints(&ws).unwrap();
    // The explicit override survives the reopen…
    assert!(
        constraints.contains(&(
            "context".into(),
            "Auth".into(),
            "Billing".into(),
            "forbidden".into()
        )),
        "explicit override should survive reopen, got {constraints:?}"
    );
    // …alongside the always-seeded Clean-Architecture defaults.
    for (kind, source, target, rule) in default_layer_constraints() {
        assert!(
            constraints.contains(&(
                kind.to_string(),
                source.to_string(),
                target.to_string(),
                rule.to_string()
            )),
            "default {source}->{target} should be present after reopen"
        );
    }
}

#[test]
fn default_constraints_seeded_at_crate_root() {
    let root = unique_crate_root("axon_policy_defaults");
    let ws = canonicalize_path(&root.to_string_lossy());
    let store = Store::open(&root).unwrap();

    let constraints = store.list_dependency_constraints(&ws).unwrap();
    assert_eq!(
        constraints.len(),
        default_layer_constraints().len(),
        "a fresh store should carry exactly the seeded defaults, got {constraints:?}"
    );
    for (kind, source, target, rule) in default_layer_constraints() {
        assert!(constraints.contains(&(
            kind.to_string(),
            source.to_string(),
            target.to_string(),
            rule.to_string()
        )));
    }
}

#[test]
fn persisted_policy_reload_updates_long_lived_store() {
    let root = unique_crate_root("axon_policy_reload");
    let ws = canonicalize_path(&root.to_string_lossy());
    let store = Store::open(&root).unwrap();

    let mut reasoning = empty_ctx("reasoning");
    reasoning.dependencies = vec!["store".into()];
    let store_ctx = empty_ctx("store");
    let mut model = empty_model();
    model.bounded_contexts = vec![reasoning, store_ctx];
    store.save_actual(&ws, &model).unwrap();

    store
        .upsert_layer_assignment(&ws, "reasoning", "application")
        .unwrap();
    store
        .upsert_layer_assignment(&ws, "store", "infrastructure")
        .unwrap();

    let before = store.evaluate_policy_violations(&ws).unwrap();
    assert_eq!(before["status"], "false");
    assert_eq!(before["count"], 1);

    fs::write(
        root.join(".axon").join("policy.json"),
        r#"{
  "version": 1,
  "layer_assignments": [
    { "context": "reasoning", "layer": "infrastructure" },
    { "context": "store", "layer": "infrastructure" }
  ],
  "dependency_constraints": []
}
"#,
    )
    .unwrap();

    assert!(store.reload_persisted_policy(&ws).unwrap());
    let assignments = store.list_layer_assignments(&ws).unwrap();
    assert!(assignments.contains(&("reasoning".into(), "infrastructure".into())));
    assert!(!assignments.contains(&("reasoning".into(), "application".into())));

    let after = store.evaluate_policy_violations(&ws).unwrap();
    assert_eq!(after["status"], "true");
    assert_eq!(after["count"], 0);

    let health = store.model_health(&ws).unwrap();
    assert!(health.policy_violations.is_empty());
}

#[test]
fn inferred_layers_flag_violation_without_manual_policy() {
    let root = unique_crate_root("axon_policy_infer");
    let ws = canonicalize_path(&root.to_string_lossy());
    let store = Store::open(&root).unwrap();

    // Conventionally-named contexts: domain depends on infrastructure (outward).
    let mut domain = empty_ctx("domain");
    domain.dependencies = vec!["infrastructure".into()];
    let infra = empty_ctx("infrastructure");
    let mut model = empty_model();
    model.bounded_contexts = vec![domain, infra];
    store.save_actual(&ws, &model).unwrap();

    // No upsert_layer_assignment / upsert_dependency_constraint calls at all.
    let assignments = store.list_layer_assignments(&ws).unwrap();
    assert!(assignments.contains(&("domain".into(), "domain".into())));
    assert!(assignments.contains(&("infrastructure".into(), "infrastructure".into())));

    let result = store.evaluate_policy_violations(&ws).unwrap();
    assert_eq!(result["status"], "false", "domain->infra must be flagged");
    let violations = result["violations"].as_array().unwrap();
    assert!(violations.iter().any(|v| {
        v["from_context"] == "domain"
            && v["to_context"] == "infrastructure"
            && v["from_layer"] == "domain"
            && v["to_layer"] == "infrastructure"
    }));
}

#[test]
fn inferred_and_default_policy_are_not_persisted() {
    let root = unique_crate_root("axon_policy_overrideonly");
    let ws = canonicalize_path(&root.to_string_lossy());

    {
        let store = Store::open(&root).unwrap();
        let mut model = empty_model();
        model.bounded_contexts = vec![empty_ctx("domain"), empty_ctx("infrastructure")];
        store.save_actual(&ws, &model).unwrap();

        // Inferred assignments exist in-memory before any explicit action.
        let before = store.list_layer_assignments(&ws).unwrap();
        assert!(before.contains(&("domain".into(), "domain".into())));
        assert!(before.contains(&("infrastructure".into(), "infrastructure".into())));

        // An explicit override on an unconventional context triggers persistence.
        store
            .upsert_layer_assignment(&ws, "billing", "domain")
            .unwrap();
    }

    // Reopen WITHOUT re-scanning: only explicit overrides should reload.
    let reopened = Store::open(&root).unwrap();
    assert_eq!(
        reopened.list_layer_assignments(&ws).unwrap(),
        vec![("billing".into(), "domain".into())],
        "inferred layers must not be persisted — only the explicit override"
    );
    assert_eq!(
        reopened.list_dependency_constraints(&ws).unwrap().len(),
        default_layer_constraints().len(),
        "no constraints beyond the re-seeded defaults should be persisted"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
//  5. Dependency path (proof witness)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn dependency_path_direct() {
    // A → B: path from A to B = [(A, B)]
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into()];
    let b = empty_ctx("B");
    model.bounded_contexts = vec![a, b];
    store.save_desired(&ws, &model).unwrap();

    let paths = store
        .query_dependency_path(&canonicalize(&ws), "A", "B")
        .unwrap();
    assert!(!paths.is_empty(), "Direct dependency must return a path");
    assert!(paths.iter().any(|p| p[0] == "A" && p[1] == "B"));
}

#[test]
fn dependency_path_transitive() {
    // A → B → C → D: path from A to D
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into()];
    let mut b = empty_ctx("B");
    b.dependencies = vec!["C".into()];
    let mut c = empty_ctx("C");
    c.dependencies = vec!["D".into()];
    let d = empty_ctx("D");
    model.bounded_contexts = vec![a, b, c, d];
    store.save_desired(&ws, &model).unwrap();

    let paths = store
        .query_dependency_path(&canonicalize(&ws), "A", "D")
        .unwrap();
    assert!(
        paths.iter().any(|path| path == &vec!["A", "B", "C", "D"]),
        "Transitive path A->B->C->D must be returned as an ordered path: {:?}",
        paths
    );
}

#[test]
fn dependency_path_no_connection() {
    // A, B (disconnected)
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    model.bounded_contexts = vec![empty_ctx("A"), empty_ctx("B")];
    store.save_desired(&ws, &model).unwrap();

    let paths = store
        .query_dependency_path(&canonicalize(&ws), "A", "B")
        .unwrap();
    assert!(paths.is_empty(), "No path between disconnected contexts");
}

// ═══════════════════════════════════════════════════════════════════════════
//  6. Deletion safety (can_delete_symbol)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn can_delete_unreferenced_entity() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut ctx = empty_ctx("Shop");
    ctx.entities = vec![Entity {
        name: "Orphan".into(),
        description: "".into(),
        aggregate_root: false,
        fields: vec![],
        methods: vec![],
        invariants: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    model.bounded_contexts = vec![ctx];
    store.save_desired(&ws, &model).unwrap();

    let result = store
        .can_delete_symbol(&canonicalize(&ws), "Shop", "Orphan")
        .unwrap();
    assert_eq!(result["can_delete"], true);
}

#[test]
fn cannot_delete_entity_with_event_source() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut ctx = empty_ctx("Shop");
    ctx.entities = vec![Entity {
        name: "Order".into(),
        description: "".into(),
        aggregate_root: true,
        fields: vec![],
        methods: vec![],
        invariants: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    ctx.events = vec![DomainEvent {
        name: "OrderPlaced".into(),
        description: "".into(),
        source: "Order".into(),
        fields: vec![],
        file_path: Some("src/shop/events.rs".into()),
        start_line: Some(12),
        end_line: Some(20),
    }];
    model.bounded_contexts = vec![ctx];
    store.save_desired(&ws, &model).unwrap();

    let result = store
        .can_delete_symbol(&canonicalize(&ws), "Shop", "Order")
        .unwrap();
    assert_eq!(result["can_delete"], false);
    let events = result["events_sourced"].as_array().unwrap();
    assert!(events.iter().any(|e| e == "OrderPlaced"));
    let event_refs = result["event_references"].as_array().unwrap();
    assert!(event_refs.iter().any(|e| {
        e["event"] == "OrderPlaced"
            && e["file"] == "src/shop/events.rs"
            && e["start_line"] == 12
            && e["end_line"] == 20
    }));
}

#[test]
fn cannot_delete_entity_with_repository() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut ctx = empty_ctx("Shop");
    ctx.entities = vec![Entity {
        name: "Product".into(),
        description: "".into(),
        aggregate_root: true,
        fields: vec![],
        methods: vec![],
        invariants: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    ctx.repositories = vec![Repository {
        name: "ProductRepo".into(),
        aggregate: "Product".into(),
        methods: vec![],
        file_path: Some("src/shop/product_repo.rs".into()),
        start_line: Some(4),
        end_line: Some(40),
    }];
    model.bounded_contexts = vec![ctx];
    store.save_desired(&ws, &model).unwrap();

    let result = store
        .can_delete_symbol(&canonicalize(&ws), "Shop", "Product")
        .unwrap();
    assert_eq!(result["can_delete"], false);
    let repos = result["repositories_managing"].as_array().unwrap();
    assert!(repos.iter().any(|r| r == "ProductRepo"));
    let repo_refs = result["repository_references"].as_array().unwrap();
    assert!(repo_refs.iter().any(|r| {
        r["repository"] == "ProductRepo"
            && r["file"] == "src/shop/product_repo.rs"
            && r["start_line"] == 4
            && r["end_line"] == 40
    }));
}

#[test]
fn cannot_delete_symbol_with_actual_import_reference() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    model.import_edges = vec![ImportEdge {
        from_file: "src/server/stdio.rs".into(),
        to_module: "crate::store::Store".into(),
        context: "server".into(),
    }];
    store.save_actual(&ws, &model).unwrap();

    let result = store
        .can_delete_symbol(&canonicalize(&ws), "", "Store")
        .unwrap();
    assert_eq!(result["can_delete"], false);
    let import_refs = result["import_references"].as_array().unwrap();
    assert!(import_refs.iter().any(|reference| {
        reference["file"] == "src/server/stdio.rs"
            && reference["import"] == "crate::store::Store"
            && reference["context"] == "server"
    }));
}

#[test]
fn cannot_delete_symbol_with_field_type_reference() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut ctx = empty_ctx("Policy");
    ctx.entities = vec![Entity {
        name: "HealthReport".into(),
        description: "".into(),
        aggregate_root: false,
        fields: vec![Field {
            name: "coverage".into(),
            field_type: "Option<PolicyCoverage>".into(),
            required: false,
            description: "".into(),
        }],
        methods: vec![],
        invariants: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    model.bounded_contexts = vec![ctx];
    store.save_actual(&ws, &model).unwrap();

    let result = store
        .can_delete_symbol(&canonicalize(&ws), "", "PolicyCoverage")
        .unwrap();
    assert_eq!(result["can_delete"], false);
    assert_eq!(
        result["type_references"]["fields"][0]["owner"],
        "HealthReport"
    );
    assert_eq!(
        result["type_references"]["fields"][0]["field_type"],
        "Option<PolicyCoverage>"
    );
}

#[test]
fn cannot_delete_qualified_symbol_with_short_call_reference() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    model.symbols = vec![SymbolDef {
        name: "Store::query_call_paths".into(),
        kind: "method".into(),
        context: "store".into(),
        file_path: "src/store/cozo.rs".into(),
        start_line: 1,
        end_line: 20,
        visibility: "public".into(),
    }];
    model.call_edges = vec![CallEdge {
        caller: "ReasoningKernel::how_connected_claim".into(),
        callee: "query_call_paths".into(),
        file_path: "src/reasoning/mod.rs".into(),
        line: 10,
        context: "reasoning".into(),
    }];
    store.save_actual(&ws, &model).unwrap();

    let result = store
        .can_delete_symbol(&canonicalize(&ws), "", "Store::query_call_paths")
        .unwrap();
    assert_eq!(result["can_delete"], false);
    assert_eq!(result["call_references"][0]["callee"], "query_call_paths");
}

// ═══════════════════════════════════════════════════════════════════════════
//  7. Orphan and god context detection
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn orphan_contexts_detected() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("Connected");
    a.dependencies = vec!["Hub".into()];
    model.bounded_contexts = vec![a, empty_ctx("Hub"), empty_ctx("Isolated")];
    store.save_desired(&ws, &model).unwrap();

    let health = store.model_health(&canonicalize(&ws)).unwrap();
    assert!(
        health.orphan_contexts.contains(&"Isolated".to_string()),
        "Isolated context should be orphan, got: {:?}",
        health.orphan_contexts
    );
    assert!(!health.orphan_contexts.contains(&"Connected".to_string()));
    assert!(!health.orphan_contexts.contains(&"Hub".to_string()));
}

#[test]
fn god_context_element_counts() {
    // Verify that contexts with many elements are counted correctly
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut ctx = empty_ctx("Big");
    for i in 0..8 {
        ctx.entities.push(Entity {
            name: format!("Entity{i}"),
            description: "".into(),
            aggregate_root: false,
            fields: vec![],
            methods: vec![],
            invariants: vec![],
            file_path: None,
            start_line: None,
            end_line: None,
        });
    }
    for i in 0..4 {
        ctx.services.push(Service {
            name: format!("Service{i}"),
            description: "".into(),
            kind: ServiceKind::Application,
            methods: vec![],
            dependencies: vec![],
            file_path: None,
            start_line: None,
            end_line: None,
        });
    }
    model.bounded_contexts = vec![ctx];
    store.save_desired(&ws, &model).unwrap();

    // Verify entity count via Datalog
    let ents = store.run_datalog(
        "?[count(name)] := *entity{workspace: $ws, context: 'Big', name, state: 'desired' @ 'NOW'}",
        &ws,
    ).unwrap();
    assert_eq!(ents[0][0], "8", "Expected 8 entities");

    // Verify service count via Datalog
    let svcs = store.run_datalog(
        "?[count(name)] := *service{workspace: $ws, context: 'Big', name, state: 'desired' @ 'NOW'}",
        &ws,
    ).unwrap();
    assert_eq!(svcs[0][0], "4", "Expected 4 services");
}

// ═══════════════════════════════════════════════════════════════════════════
//  8. Impact analysis
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn impact_analysis_events_and_dependents() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();

    let mut catalog = empty_ctx("Catalog");
    catalog.entities = vec![Entity {
        name: "Product".into(),
        description: "".into(),
        aggregate_root: true,
        fields: vec![],
        methods: vec![],
        invariants: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    catalog.events = vec![DomainEvent {
        name: "ProductCreated".into(),
        description: "".into(),
        source: "Product".into(),
        fields: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    catalog.repositories = vec![Repository {
        name: "ProductRepo".into(),
        aggregate: "Product".into(),
        methods: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    catalog.services = vec![Service {
        name: "CatalogSvc".into(),
        description: "".into(),
        kind: ServiceKind::Application,
        methods: vec![],
        dependencies: vec!["ProductRepo".into()],
        file_path: None,
        start_line: None,
        end_line: None,
    }];

    let mut ordering = empty_ctx("Ordering");
    ordering.dependencies = vec!["Catalog".into()];

    model.bounded_contexts = vec![catalog, ordering];
    store.save_desired(&ws, &model).unwrap();

    let canon = canonicalize(&ws);
    let result = store.impact_analysis(&canon, "Catalog", "Product").unwrap();

    let affected_events = result["affected_events"].as_array().unwrap();
    assert!(
        affected_events
            .iter()
            .any(|e| e["event"] == "ProductCreated"),
        "Product events should be in impact: {:?}",
        result
    );

    let affected_services = result["affected_services"].as_array().unwrap();
    assert!(
        affected_services
            .iter()
            .any(|s| s["service"] == "CatalogSvc"),
        "Service depending on ProductRepo should be impacted: {:?}",
        result
    );

    let dependents = result["dependent_contexts"].as_array().unwrap();
    assert!(
        dependents.iter().any(|d| d == "Ordering"),
        "Ordering depends on Catalog: {:?}",
        result
    );
}

// ═══════════════════════════════════════════════════════════════════════════
//  9. Graph algorithms
// ═══════════════════════════════════════════════════════════════════════════

// NOTE: PageRank, CommunityDetectionLouvain, and BetweennessCentrality depend
// on optional Cozo fixed rules. Pure graph algorithms stay covered here.

#[test]
fn degree_centrality_counts_context_edges() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into(), "C".into()];
    let mut b = empty_ctx("B");
    b.dependencies = vec!["C".into()];
    let c = empty_ctx("C");
    model.bounded_contexts = vec![a, b, c];
    store.save_actual(&ws, &model).unwrap();

    let degrees = store.degree_centrality(&canonicalize(&ws)).unwrap();
    assert!(
        degrees.iter().any(|row| row == &("A".into(), 0, 2)),
        "A degree missing: {:?}",
        degrees
    );
    assert!(
        degrees.iter().any(|row| row == &("B".into(), 1, 1)),
        "B degree missing: {:?}",
        degrees
    );
    assert!(
        degrees.iter().any(|row| row == &("C".into(), 2, 0)),
        "C degree missing: {:?}",
        degrees
    );
}

#[test]
fn topological_order_returns_acyclic_order_without_fixed_rules() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into(), "C".into()];
    let mut b = empty_ctx("B");
    b.dependencies = vec!["C".into()];
    let c = empty_ctx("C");
    model.bounded_contexts = vec![a, b, c];
    store.save_actual(&ws, &model).unwrap();

    let result = store.topological_order(&canonicalize(&ws)).unwrap();
    assert_eq!(result["status"], "acyclic");
    let order = result["order"].as_array().unwrap();
    assert_eq!(order.len(), 3, "unexpected topological order: {:?}", result);
    let position = |name: &str| {
        order
            .iter()
            .position(|item| item["context"] == name)
            .unwrap()
    };
    assert!(position("C") < position("B"));
    assert!(position("B") < position("A"));
}

// ═══════════════════════════════════════════════════════════════════════════
// 10. Model health composite score
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn model_health_perfect_score() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.entities = vec![Entity {
        name: "Root".into(),
        description: "".into(),
        aggregate_root: true,
        fields: vec![],
        methods: vec![],
        invariants: vec!["Must be valid".into()],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    a.events = vec![DomainEvent {
        name: "Created".into(),
        description: "".into(),
        source: "Root".into(),
        fields: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    a.dependencies = vec!["B".into()];
    let b = empty_ctx("B");
    model.bounded_contexts = vec![a, b];
    store.save_desired(&ws, &model).unwrap();

    let health = store.model_health(&canonicalize(&ws)).unwrap();
    assert!(
        health.score >= 90,
        "Clean model should score >=90, got {}",
        health.score
    );
    assert!(health.circular_deps.is_empty());
    assert!(health.layer_violations.is_empty());
}

#[test]
fn model_health_reports_policy_coverage_gaps() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into()];
    let b = empty_ctx("B");
    model.bounded_contexts = vec![a, b];
    store.save_desired(&ws, &model).unwrap();

    let canon = canonicalize(&ws);
    let health = store.model_health(&canon).unwrap();
    assert_eq!(health.policy_coverage.context_count, 2);
    assert_eq!(health.policy_coverage.layer_assignment_count, 0);
    assert_eq!(health.policy_coverage.dependency_constraint_count, 0);
    assert_eq!(health.policy_coverage.missing_layer_assignments.len(), 2);
    assert!(health.score < 100);

    store
        .upsert_layer_assignment(&canon, "A", "application")
        .unwrap();
    store
        .upsert_dependency_constraint(&canon, "layer", "domain", "infrastructure", "forbidden")
        .unwrap();

    let updated = store.model_health(&canon).unwrap();
    assert_eq!(updated.policy_coverage.layer_assignment_count, 1);
    assert_eq!(updated.policy_coverage.dependency_constraint_count, 1);
    assert_eq!(updated.policy_coverage.missing_layer_assignments, vec!["B"]);
}

#[test]
fn model_health_reports_policy_violations() {
    let store = temp_store();
    let ws = ws();
    let canon = canonicalize(&ws);
    let mut model = empty_model();
    let mut app = empty_ctx("App");
    app.dependencies = vec!["Infra".into()];
    let infra = empty_ctx("Infra");
    model.bounded_contexts = vec![app, infra];
    store.save_desired(&ws, &model).unwrap();

    store
        .upsert_layer_assignment(&canon, "App", "application")
        .unwrap();
    store
        .upsert_layer_assignment(&canon, "Infra", "infrastructure")
        .unwrap();
    store
        .upsert_dependency_constraint(
            &canon,
            "layer",
            "application",
            "infrastructure",
            "forbidden",
        )
        .unwrap();

    let health = store.model_health(&canon).unwrap();
    assert_eq!(health.policy_violations.len(), 1);
    assert_eq!(health.policy_violations[0]["from_context"], "App");
    assert!(health.score < 100);
}

#[test]
fn model_health_degrades_with_cycles() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut a = empty_ctx("A");
    a.dependencies = vec!["B".into()];
    let mut b = empty_ctx("B");
    b.dependencies = vec!["A".into()];
    model.bounded_contexts = vec![a, b];
    store.save_desired(&ws, &model).unwrap();

    let health = store.model_health(&canonicalize(&ws)).unwrap();
    assert!(
        health.score < 90,
        "Cycle should degrade score, got {}",
        health.score
    );
    assert!(!health.circular_deps.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════
// 11. Aggregate roots without invariants
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn aggregate_root_missing_invariant() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut ctx = empty_ctx("Shop");
    ctx.entities = vec![
        Entity {
            name: "WithInvariant".into(),
            description: "".into(),
            aggregate_root: true,
            fields: vec![],
            methods: vec![],
            invariants: vec!["rule".into()],
            file_path: None,
            start_line: None,
            end_line: None,
        },
        Entity {
            name: "WithoutInvariant".into(),
            description: "".into(),
            aggregate_root: true,
            fields: vec![],
            methods: vec![],
            invariants: vec![],
            file_path: None,
            start_line: None,
            end_line: None,
        },
        Entity {
            name: "NonRoot".into(),
            description: "".into(),
            aggregate_root: false,
            fields: vec![],
            methods: vec![],
            invariants: vec![],
            file_path: None,
            start_line: None,
            end_line: None,
        },
    ];
    model.bounded_contexts = vec![ctx];
    store.save_desired(&ws, &model).unwrap();

    let missing = store
        .aggregate_roots_without_invariants(&canonicalize(&ws))
        .unwrap();
    assert_eq!(missing.len(), 1, "Only 1 aggregate root lacks invariants");
    assert_eq!(missing[0].1, "WithoutInvariant");
}

// ═══════════════════════════════════════════════════════════════════════════
// 12. Drift computation
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn drift_reflects_desired_actual_diff() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    model.bounded_contexts = vec![empty_ctx("Existing")];
    store.save_desired(&ws, &model).unwrap();
    store.accept(&ws).unwrap();

    // Add a new context to desired only
    model.bounded_contexts.push(empty_ctx("New"));
    store.save_desired(&ws, &model).unwrap();

    store.compute_drift(&ws).unwrap();
    let drift = store.load_drift(&ws).unwrap();
    assert!(
        !drift.is_empty(),
        "Drift should exist after desired diverges from actual"
    );
    assert!(
        drift
            .iter()
            .any(|(_, _, name, ct)| name == "New" && ct == "add"),
        "New context should appear as drift add entry: {:?}",
        drift
    );
}

#[test]
fn drift_empty_when_synced() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    model.bounded_contexts = vec![empty_ctx("A")];
    store.save_desired(&ws, &model).unwrap();
    store.accept(&ws).unwrap();
    store.compute_drift(&ws).unwrap();

    let drift = store.load_drift(&ws).unwrap();
    assert!(drift.is_empty(), "No drift when desired == actual");
}

// ═══════════════════════════════════════════════════════════════════════════
// 13. Full-text search
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn search_architecture_finds_by_description() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    let mut ctx = empty_ctx("Payments");
    ctx.description = "Handles payment processing and billing".into();
    ctx.entities = vec![Entity {
        name: "Invoice".into(),
        description: "Represents a billing invoice".into(),
        aggregate_root: true,
        fields: vec![],
        methods: vec![],
        invariants: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    model.bounded_contexts = vec![ctx];
    store.save_desired(&ws, &model).unwrap();

    let result = store
        .search_text(&canonicalize(&ws), "billing", 20)
        .unwrap();
    let count = result["count"].as_i64().unwrap();
    assert!(
        count > 0,
        "Search for 'billing' should find results: {:?}",
        result
    );
}

#[test]
fn search_architecture_finds_policy_constraints() {
    let store = temp_store();
    let ws = ws();
    let canon = canonicalize(&ws);
    let mut model = empty_model();
    model.bounded_contexts = vec![empty_ctx("Domain"), empty_ctx("Infra")];
    store.save_actual(&ws, &model).unwrap();
    store
        .upsert_layer_assignment(&canon, "Domain", "domain")
        .unwrap();
    store
        .upsert_dependency_constraint(&canon, "layer", "domain", "infrastructure", "forbidden")
        .unwrap();

    let result = store
        .search_text(&canon, "policy dependency constraints architecture", 20)
        .unwrap();
    let results = result["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .any(|item| item["kind"] == "dependency_constraint"),
        "dependency constraint missing from search results: {result:?}"
    );
    assert!(
        results
            .iter()
            .any(|item| item["kind"] == "layer_assignment"),
        "layer assignment missing from search results: {result:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 14. Call graph queries
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn call_graph_callers_and_callees() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    model.call_edges = vec![
        CallEdge {
            caller: "main".into(),
            callee: "init".into(),
            file_path: "src/main.rs".into(),
            line: 10,
            context: "".into(),
        },
        CallEdge {
            caller: "main".into(),
            callee: "run".into(),
            file_path: "src/main.rs".into(),
            line: 11,
            context: "".into(),
        },
        CallEdge {
            caller: "init".into(),
            callee: "setup_db".into(),
            file_path: "src/init.rs".into(),
            line: 5,
            context: "".into(),
        },
    ];
    store.save_actual(&ws, &model).unwrap();

    let canon = canonicalize(&ws);

    // Callees of main
    let callees = store.call_graph_callees(&canon, "main").unwrap();
    assert_eq!(callees["count"], 2);

    // Callers of init
    let callers = store.call_graph_callers(&canon, "init").unwrap();
    assert_eq!(callers["count"], 1);
    assert_eq!(callers["callers"][0]["caller"], "main");

    // Reachability from main
    let reachable = store.call_graph_reachability(&canon, "main").unwrap();
    assert!(
        reachable["count"].as_i64().unwrap() >= 3,
        "main should transitively reach init, run, setup_db: {:?}",
        reachable
    );
}

#[test]
fn call_graph_queries_accept_qualified_and_short_symbol_aliases() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    model.call_edges = vec![
        CallEdge {
            caller: "ReasoningKernel::how_connected_claim".into(),
            callee: "query_call_paths".into(),
            file_path: "src/reasoning/mod.rs".into(),
            line: 10,
            context: "reasoning".into(),
        },
        CallEdge {
            caller: "Store::query_call_paths".into(),
            callee: "collect_dependency_paths".into(),
            file_path: "src/store/cozo.rs".into(),
            line: 20,
            context: "store".into(),
        },
    ];
    store.save_actual(&ws, &model).unwrap();
    let canon = canonicalize(&ws);

    let callers = store
        .call_graph_callers(&canon, "Store::query_call_paths")
        .unwrap();
    assert_eq!(callers["count"], 1);
    assert_eq!(
        callers["callers"][0]["caller"],
        "ReasoningKernel::how_connected_claim"
    );

    let callees = store
        .call_graph_callees(&canon, "query_call_paths")
        .unwrap();
    assert_eq!(callees["count"], 1);
    assert_eq!(callees["callees"][0]["callee"], "collect_dependency_paths");

    let reachable = store
        .call_graph_reachability(&canon, "query_call_paths")
        .unwrap();
    assert!(
        reachable["reachable"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "collect_dependency_paths")
    );
}

#[test]
fn call_graph_stats_summary() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    model.call_edges = vec![
        CallEdge {
            caller: "a".into(),
            callee: "b".into(),
            file_path: "".into(),
            line: 1,
            context: "".into(),
        },
        CallEdge {
            caller: "a".into(),
            callee: "c".into(),
            file_path: "".into(),
            line: 2,
            context: "".into(),
        },
        CallEdge {
            caller: "b".into(),
            callee: "c".into(),
            file_path: "".into(),
            line: 3,
            context: "".into(),
        },
    ];
    model.symbols = vec![SymbolDef {
        name: "Project::c".into(),
        kind: "method".into(),
        context: "".into(),
        file_path: "src/lib.rs".into(),
        start_line: 1,
        end_line: 1,
        visibility: "private".into(),
    }];
    store.save_actual(&ws, &model).unwrap();
    store
        .save_resolved_calls(
            &ws,
            &[
                ResolvedCall {
                    caller: "a".into(),
                    callee: "Project::c".into(),
                    callee_file: "src/lib.rs".into(),
                    callee_line: 1,
                },
                ResolvedCall {
                    caller: "b".into(),
                    callee: "Project::c".into(),
                    callee_file: "src/lib.rs".into(),
                    callee_line: 1,
                },
            ],
        )
        .unwrap();

    let stats = store.call_graph_stats(&canonicalize(&ws)).unwrap();
    assert_eq!(stats["total_edges"], 3);
    assert_eq!(stats["unique_callers"], 2); // a, b
    assert_eq!(stats["unique_callees"], 2); // b, c
    assert_eq!(stats["project_callee_edges"], 2);
    assert_eq!(stats["unique_project_callees"], 1);
    assert_eq!(
        stats["project_callee_stats"]["call_graph_relation"],
        "calls_symbol"
    );
    assert_eq!(stats["project_callee_stats"]["ambiguity"], "name_based");
    assert_eq!(stats["hottest_project_callees"][0]["callee"], "c");
    assert_eq!(stats["hottest_project_callees"][0]["call_count"], 2);
    assert_eq!(
        stats["hottest_project_callees"][0]["call_graph_relation"],
        "calls_symbol"
    );
    assert_eq!(
        stats["hottest_project_callees"][0]["ambiguity"],
        "name_based"
    );
    assert_eq!(stats["resolved_project_callee_edges"], 2);
    assert_eq!(stats["unique_resolved_project_callees"], 1);
    assert_eq!(
        stats["resolved_project_callee_stats"]["call_graph_relation"],
        "resolved_call"
    );
    assert_eq!(
        stats["resolved_project_callee_stats"]["ambiguity"],
        "compiler_resolved"
    );
    assert_eq!(
        stats["hottest_resolved_project_callees"][0]["callee"],
        "Project::c"
    );
    assert_eq!(
        stats["hottest_resolved_project_callees"][0]["callee_file"],
        "src/lib.rs"
    );
    assert_eq!(
        stats["hottest_resolved_project_callees"][0]["callee_line"],
        1
    );
    assert_eq!(
        stats["hottest_resolved_project_callees"][0]["call_count"],
        2
    );
}

#[test]
fn rust_graph_relation_counts_match_call_graph_stats() {
    let store = temp_store();
    let ws = ws();
    let mut model = empty_model();
    model.call_edges = vec![
        CallEdge {
            caller: "a".into(),
            callee: "b".into(),
            file_path: "".into(),
            line: 1,
            context: "".into(),
        },
        CallEdge {
            caller: "a".into(),
            callee: "c".into(),
            file_path: "".into(),
            line: 2,
            context: "".into(),
        },
        CallEdge {
            caller: "b".into(),
            callee: "c".into(),
            file_path: "".into(),
            line: 3,
            context: "".into(),
        },
    ];
    store.save_actual(&ws, &model).unwrap();

    let canon = canonicalize(&ws);
    let counts = store.rust_graph_relation_counts(&canon).unwrap();
    let stats = store.call_graph_stats(&canon).unwrap();

    assert_eq!(counts.get("calls_symbol"), Some(&3));
    assert_eq!(stats["total_edges"], 3);
    assert_eq!(
        counts["calls_symbol"],
        stats["total_edges"].as_u64().unwrap() as usize
    );

    let graph = store
        .query_rust_graph(&canon, &serde_json::json!({ "view": "relations" }))
        .unwrap();
    assert_eq!(graph["relation_counts"]["calls_symbol"], 3);

    let filtered = store
        .query_rust_graph(
            &canon,
            &serde_json::json!({ "view": "relations", "relation": "calls_symbol" }),
        )
        .unwrap();
    assert_eq!(filtered["summary"]["relation_count"], 1);
    assert_eq!(filtered["relation_counts"]["calls_symbol"], 3);
    assert!(filtered["relation_counts"]["import_edge"].is_null());
}

// ═══════════════════════════════════════════════════════════════════════════
//  14. Validity time-travel: copy_state covers modules & api_endpoints
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn copy_state_copies_modules_and_api_endpoints() {
    let store = temp_store();
    let ws = ws();

    let mut ctx = empty_ctx("Orders");
    ctx.modules = vec![Module {
        name: "orders_mod".into(),
        path: "src/orders".into(),
        public: true,
        file_path: "src/orders/mod.rs".into(),
        description: "Orders module".into(),
    }];
    ctx.api_endpoints = vec![APIEndpoint {
        id: "create_order".into(),
        service_id: "OrderService".into(),
        method: "POST".into(),
        route_pattern: "/orders".into(),
        description: "Create an order".into(),
    }];

    let mut model = empty_model();
    model.bounded_contexts = vec![ctx];
    store.save_desired(&ws, &model).unwrap();

    // Accept: copies desired → actual
    store.accept(&ws).unwrap();

    let actual = store
        .load_actual(&ws)
        .unwrap()
        .expect("actual should exist after accept");
    let ctx = actual
        .bounded_contexts
        .iter()
        .find(|c| c.name == "Orders")
        .expect("Orders context");

    // Modules must have been copied
    assert_eq!(
        ctx.modules.len(),
        1,
        "module should be copied to actual state"
    );
    assert_eq!(ctx.modules[0].name, "orders_mod");
    assert_eq!(ctx.modules[0].path, "src/orders");

    // API endpoints must have been copied
    assert_eq!(
        ctx.api_endpoints.len(),
        1,
        "api_endpoint should be copied to actual state"
    );
    assert_eq!(ctx.api_endpoints[0].id, "create_order");
    assert_eq!(ctx.api_endpoints[0].method, "POST");
}

// ═══════════════════════════════════════════════════════════════════════════
//  15. Validity time-travel: copy_state copies ast_edges
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn copy_state_copies_ast_edges() {
    let store = temp_store();
    let ws = ws();

    let mut model = empty_model();
    model.bounded_contexts = vec![empty_ctx("Core")];
    model.ast_edges = vec![ASTEdge {
        from_node: "OrderHandler".into(),
        to_node: "Handler".into(),
        edge_type: "implements".into(),
        file_path: String::new(),
        line: 0,
    }];
    store.save_desired(&ws, &model).unwrap();

    store.accept(&ws).unwrap();

    let actual = store
        .load_actual(&ws)
        .unwrap()
        .expect("actual should exist");
    assert_eq!(
        actual.ast_edges.len(),
        1,
        "ast_edge should be copied to actual state"
    );
    assert_eq!(actual.ast_edges[0].from_node, "OrderHandler");
    assert_eq!(actual.ast_edges[0].edge_type, "implements");
}

// ═══════════════════════════════════════════════════════════════════════════
//  16. Validity time-travel: accept/reset full roundtrip
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn accept_reset_roundtrip_preserves_all_relations() {
    let store = temp_store();
    let ws = ws();

    let mut ctx = empty_ctx("Payments");
    ctx.entities = vec![Entity {
        name: "Payment".into(),
        description: "A payment".into(),
        aggregate_root: true,
        fields: vec![Field {
            name: "amount".into(),
            field_type: "Money".into(),
            required: true,
            description: "".into(),
        }],
        methods: vec![],
        invariants: vec!["Amount must be positive".into()],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    ctx.services = vec![Service {
        name: "PaymentService".into(),
        description: "Processes payments".into(),
        kind: ServiceKind::Application,
        methods: vec![],
        dependencies: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    ctx.events = vec![DomainEvent {
        name: "PaymentReceived".into(),
        description: "Emitted on payment".into(),
        source: "PaymentService".into(),
        fields: vec![],
        file_path: None,
        start_line: None,
        end_line: None,
    }];
    ctx.modules = vec![Module {
        name: "payments_mod".into(),
        path: "src/payments".into(),
        public: true,
        file_path: "src/payments/mod.rs".into(),
        description: "".into(),
    }];

    let mut model = empty_model();
    model.bounded_contexts = vec![ctx];

    // Save implemented graph. accept/reset are compatibility no-ops in actual-first mode.
    store.save_desired(&ws, &model).unwrap();
    store.accept(&ws).unwrap();

    // Now modify the implemented graph via the compatibility alias.
    let mut model2 = model.clone();
    model2.bounded_contexts[0].entities[0].description = "Modified payment".into();
    store.save_desired(&ws, &model2).unwrap();

    // Reset leaves the implemented graph unchanged.
    let restored = store
        .reset(&ws)
        .unwrap()
        .expect("reset should return model");
    assert_eq!(
        restored.bounded_contexts[0].entities[0].description, "Modified payment",
        "reset should leave the actual-first implemented graph unchanged"
    );

    // Modules should survive the reset roundtrip
    assert_eq!(
        restored.bounded_contexts[0].modules.len(),
        1,
        "module should survive reset"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
//  17. Validity time-travel: snapshot_log records timestamps
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn snapshot_log_records_on_save() {
    let store = temp_store();
    let ws = ws();

    // No snapshots yet
    let snaps = store.list_snapshots(&ws, "desired").unwrap();
    assert!(snaps.is_empty(), "no snapshots before any save");

    // Save desired
    let mut model = empty_model();
    model.bounded_contexts = vec![empty_ctx("A")];
    store.save_desired(&ws, &model).unwrap();

    let snaps = store.list_snapshots(&ws, "desired").unwrap();
    assert_eq!(snaps.len(), 1, "one snapshot after first save");

    // Save again
    model.bounded_contexts.push(empty_ctx("B"));
    store.save_desired(&ws, &model).unwrap();

    let snaps = store.list_snapshots(&ws, "desired").unwrap();
    assert_eq!(snaps.len(), 2, "two snapshots after second save");
    // Most recent should be larger timestamp
    assert!(
        snaps[0] > snaps[1],
        "most recent snapshot should have larger timestamp"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
//  18. Validity time-travel: diff_snapshots detects changes
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn diff_snapshots_detects_added_context() {
    let store = temp_store();
    let ws = ws();

    // First snapshot: one context
    let mut model = empty_model();
    model.bounded_contexts = vec![empty_ctx("Alpha")];
    store.save_desired(&ws, &model).unwrap();

    // Small delay to ensure distinct timestamps
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Second snapshot: two contexts
    model.bounded_contexts.push(empty_ctx("Beta"));
    store.save_desired(&ws, &model).unwrap();

    let snaps = store.list_snapshots(&ws, "desired").unwrap();
    assert_eq!(snaps.len(), 2);

    let ts_new = snaps[0]; // most recent
    let ts_old = snaps[1]; // older

    let diff = store
        .diff_snapshots(&ws, "desired", ts_old, ts_new)
        .unwrap();
    let empty_vec = vec![];
    let added = diff["added"].as_array().unwrap_or(&empty_vec);

    // Beta should be in added
    let added_names: Vec<String> = added
        .iter()
        .filter_map(|v| v["name"].as_str().map(String::from))
        .collect();
    assert!(
        added_names.iter().any(|n| n == "Beta"),
        "diff_snapshots should detect added context 'Beta', got: {:?}",
        added_names
    );
}

// ═══════════════════════════════════════════════════════════════════════════
//  19. Validity time-travel: clear_state retracts api_endpoint
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn clear_state_retracts_api_endpoints() {
    let store = temp_store();
    let ws = ws();

    let mut ctx = empty_ctx("Catalog");
    ctx.api_endpoints = vec![APIEndpoint {
        id: "list_items".into(),
        service_id: "CatalogService".into(),
        method: "GET".into(),
        route_pattern: "/items".into(),
        description: "List items".into(),
    }];

    let mut model = empty_model();
    model.bounded_contexts = vec![ctx];
    store.save_desired(&ws, &model).unwrap();

    // Verify endpoint exists
    let loaded = store.load_desired(&ws).unwrap().unwrap();
    assert_eq!(loaded.bounded_contexts[0].api_endpoints.len(), 1);

    // Save empty model → clear_state is called internally, then new state saved
    let empty = empty_model();
    store.save_desired(&ws, &empty).unwrap();

    // Endpoint should no longer appear
    let loaded2 = store.load_desired(&ws).unwrap();
    if let Some(m) = loaded2 {
        let total_endpoints: usize = m
            .bounded_contexts
            .iter()
            .map(|c| c.api_endpoints.len())
            .sum();
        assert_eq!(
            total_endpoints, 0,
            "api_endpoints should be retracted after clear_state"
        );
    }
    // If None, that's also fine — no data at all
}
