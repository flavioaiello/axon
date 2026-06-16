use super::*;
use crate::domain::model::{ASTEdge, Severity};
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
        reference_edges: vec![],
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
        reference_edges: vec![],
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
        reference_edges: vec![],
    }
}

fn temp_store() -> Store {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = temp_dir().join(format!("axon_cozo_test_{}_{}.db", std::process::id(), id));
    Store::open(&path).unwrap()
}

#[test]
fn canonicalize_path_preserves_filesystem_root() {
    assert_eq!(canonicalize_path("/"), "/");
    assert_eq!(canonicalize_path("////"), "/");
}

#[test]
fn resolved_calls_are_queryable_via_graph() {
    use crate::domain::rust_analyzer::ResolvedCall;
    let store = temp_store();
    let ws = "/tmp/resolved_ws";
    let calls = vec![
        ResolvedCall {
            caller: "CozoStore::save_actual".into(),
            caller_file: "src/store/cozo.rs".into(),
            caller_line: 400,
            callee: "CozoStore::save_state".into(),
            callee_file: "src/store/cozo.rs".into(),
            callee_line: 410,
            call_site_line: 402,
            call_expr: "save_state".into(),
            dispatch_kind: "direct".into(),
        },
        ResolvedCall {
            caller: "CozoStore::save_actual".into(),
            caller_file: "src/store/cozo.rs".into(),
            caller_line: 400,
            callee: "canonicalize_path".into(),
            callee_file: "src/store/cozo.rs".into(),
            callee_line: 7000,
            call_site_line: 401,
            call_expr: "canonicalize_path".into(),
            dispatch_kind: "direct".into(),
        },
        ResolvedCall {
            caller: "main".into(),
            caller_file: "src/main.rs".into(),
            caller_line: 1,
            callee: "CozoStore::save_state".into(),
            callee_file: "src/store/cozo.rs".into(),
            callee_line: 410,
            call_site_line: 3,
            call_expr: "save_state".into(),
            dispatch_kind: "direct".into(),
        },
    ];
    assert_eq!(store.save_resolved_calls(ws, &calls).unwrap(), 3);

    // `from="CozoStore::save_actual"` → what it actually calls (two edges).
    let q = store
            .query_rust_graph(
                ws,
                &json!({ "view": "edges", "relation": "resolved_call", "from": "CozoStore::save_actual" }),
            )
            .unwrap();
    let edges = q["edges"].as_array().unwrap();
    assert_eq!(
        edges.len(),
        2,
        "save_actual makes two resolved calls: {edges:?}"
    );
    assert!(edges.iter().all(|e| e["from"] == "CozoStore::save_actual"));
    assert!(edges.iter().any(|e| e["to"] == "CozoStore::save_state"));
    assert!(edges.iter().any(|e| e["file"] == "src/store/cozo.rs"));
    assert!(
        edges
            .iter()
            .any(|e| e["caller_file"] == "src/store/cozo.rs")
    );
    assert!(edges.iter().any(|e| e["call_site_line"] == 402));
    assert!(edges.iter().any(|e| e["call_expr"] == "save_state"));
    assert!(edges.iter().all(|e| e["dispatch_kind"] == "direct"));

    // `to="save_state"` → every caller resolving to save_state (two callers).
    let q2 = store
        .query_rust_graph(
            ws,
            &json!({ "view": "edges", "relation": "resolved_call", "to": "save_state" }),
        )
        .unwrap();
    assert_eq!(q2["edges"].as_array().unwrap().len(), 2);

    // Pagination is explicit: clients can request the next page and inspect
    // whether the current response is exhaustive for the full result set.
    let page1 = store
        .query_rust_graph(
            ws,
            &json!({ "view": "edges", "relation": "resolved_call", "limit": 2 }),
        )
        .unwrap();
    assert_eq!(page1["edges"].as_array().unwrap().len(), 2);
    assert_eq!(page1["total_before_limit"], 3);
    assert_eq!(page1["truncated"], true);
    assert_eq!(page1["next_offset"], 2);
    assert_eq!(page1["exhaustiveness"]["exhaustive"], false);

    let page2 = store
        .query_rust_graph(
            ws,
            &json!({ "view": "edges", "relation": "resolved_call", "limit": 2, "offset": 2 }),
        )
        .unwrap();
    assert_eq!(page2["edges"].as_array().unwrap().len(), 1);
    assert_eq!(page2["truncated"], false);
    assert_eq!(page2["next_offset"], Value::Null);
    assert_eq!(page2["exhaustiveness"]["page_complete"], true);

    // Re-saving replaces (retract-then-assert): a smaller set shrinks the relation.
    assert_eq!(store.save_resolved_calls(ws, &calls[..1]).unwrap(), 1);
    let q3 = store
        .query_rust_graph(ws, &json!({ "view": "edges", "relation": "resolved_call" }))
        .unwrap();
    assert_eq!(
        q3["edges"].as_array().unwrap().len(),
        1,
        "stale edges must be retracted on re-save"
    );
}

#[test]
fn text_matches_is_precise_for_paths_but_fuzzy_for_symbols() {
    // Path/namespace-qualified needles require a full-substring match: a shared
    // segment like "src"/"domain" must NOT match an unrelated file (the bug).
    assert!(text_matches("src/domain/model.rs", "src/domain/model.rs"));
    assert!(!text_matches(
        "src/domain/analyze.rs",
        "src/domain/model.rs"
    ));
    assert!(!text_matches("src/store/cozo.rs", "src/domain/model.rs"));
    assert!(!text_matches("Store::open", "store::save"));
    // Single-token needles keep the loose fuzzy fallback (symbol search).
    assert!(text_matches("DomainModel", "domain"));
    assert!(text_matches("CozoStore::save_actual", "save_actual"));
    // An absolute-path filter matches a stored relative path at a path
    // boundary (and vice-versa), without matching an unrelated sibling.
    // Needles arrive already lowercased from the caller.
    assert!(text_matches(
        "src/dht.rs",
        "/users/flavioaiello/git/magik.run/korium/src/dht.rs"
    ));
    assert!(text_matches(
        "src/domain/model.rs",
        "/users/me/proj/src/domain/model.rs"
    ));
    assert!(text_matches(
        "/users/me/proj/src/domain/model.rs",
        "src/domain/model.rs"
    ));
    assert!(!text_matches(
        "src/domain/analyze.rs",
        "/users/me/proj/src/domain/model.rs"
    ));
    assert!(!text_matches(
        "src/fabric.rs",
        "/users/flavioaiello/git/magik.run/korium/src/dht.rs"
    ));
}

#[test]
fn file_module_path_derivation() {
    assert_eq!(file_module_path("src/domain/scanner.rs"), "domain::scanner");
    assert_eq!(file_module_path("src/domain/mod.rs"), "domain");
    assert_eq!(file_module_path("src/server/web.rs"), "server::web");
    assert_eq!(file_module_path("src/lib.rs"), "");
    assert_eq!(file_module_path("src/main.rs"), "");
}

#[test]
fn resolve_internal_module_handles_super_self_crate() {
    let known: BTreeSet<String> = [
        "domain",
        "domain::analyze",
        "domain::scanner",
        "fabric",
        "dht",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(
        resolve_internal_module("crate::domain::scanner::AstScanner", "mcp::tools", &known),
        Some("domain::scanner".into())
    );
    assert_eq!(
        resolve_internal_module("super::analyze::CallInfo", "domain::rust_syn", &known),
        Some("domain::analyze".into())
    );
    assert_eq!(
        resolve_internal_module("super::*", "domain::rust_syn", &known),
        Some("domain".into())
    );
    assert_eq!(
        resolve_internal_module("std::fs", "domain::analyze", &known),
        None
    );
    assert_eq!(
        resolve_internal_module("anyhow::Result", "domain::analyze", &known),
        None
    );
}

#[test]
fn is_ancestor_detects_parent_child() {
    assert!(is_ancestor("domain", "domain::analyze"));
    assert!(is_ancestor("", "domain"));
    assert!(!is_ancestor("domain::analyze", "domain"));
    assert!(!is_ancestor("dht", "fabric"));
}

#[test]
fn module_cycles_finds_sibling_cycle() {
    let store = temp_store();
    let ws = "/tmp/modcyc_ws";
    let model: DomainModel = serde_json::from_value(json!({
            "name": "cyc",
            "source_files": [
                {"path": "src/dht.rs", "context": "core", "language": "rust"},
                {"path": "src/fabric.rs", "context": "core", "language": "rust"},
                {"path": "src/util.rs", "context": "core", "language": "rust"}
            ],
            "import_edges": [
                {"from_file": "src/dht.rs", "to_module": "crate::fabric::Frame", "context": "core"},
                {"from_file": "src/fabric.rs", "to_module": "crate::dht::Node", "context": "core"},
                {"from_file": "src/dht.rs", "to_module": "std::collections::HashMap", "context": "core"},
                {"from_file": "src/util.rs", "to_module": "crate::dht::Node", "context": "core"}
            ]
        }))
        .unwrap();
    store.save_actual(ws, &model).unwrap();

    let cycles = store.module_cycles(ws).unwrap();
    assert_eq!(
        cycles,
        vec![vec!["dht".to_string(), "fabric".to_string()]],
        "dht⇄fabric is a 2-module cycle; util->dht is one-way; std is external: {cycles:?}"
    );
}

#[test]
fn pure_rust_graph_algorithms_run_without_cozo_fixed_rules() {
    let store = temp_store();
    let ws = "/tmp/algo_ws";
    let model: DomainModel = serde_json::from_value(json!({
        "name": "deps",
        "bounded_contexts": [
            {"name": "A", "dependencies": ["B"]},
            {"name": "B", "dependencies": ["C"]},
            {"name": "C", "dependencies": []}
        ]
    }))
    .unwrap();
    store.save_actual(ws, &model).unwrap();

    // PageRank: every context ranked; sink C accumulates the most rank.
    let pr = store.pagerank(ws).unwrap();
    assert_eq!(pr.len(), 3, "one rank per context: {pr:?}");
    assert_eq!(pr[0].0, "C", "sink context should rank highest: {pr:?}");

    // Betweenness: B sits on the only shortest path A->C.
    let bc = store.betweenness_centrality(ws).unwrap();
    assert_eq!(bc.len(), 3);
    let b = bc.iter().find(|(n, _)| n == "B").unwrap().1;
    assert!(b > 0.0, "B is a bridge, betweenness must be > 0: {bc:?}");

    // Community detection returns one assignment per context.
    assert_eq!(store.community_detection(ws).unwrap().len(), 3);
}

#[test]
fn optimization_recommendations_surface_shape_candidates() {
    let store = temp_store();
    let ws = "/tmp/shape_ws";
    let mut model = DomainModel::empty(ws);
    model.source_files = vec![
        SourceFile {
            path: "src/store/cozo.rs".into(),
            context: "store".into(),
            language: "rust".into(),
        },
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
            path: "src/mcp/write_tools.rs".into(),
            context: "mcp".into(),
            language: "rust".into(),
        },
        SourceFile {
            path: "src/server/web.rs".into(),
            context: "server".into(),
            language: "rust".into(),
        },
        SourceFile {
            path: "src/domain/ports.rs".into(),
            context: "domain".into(),
            language: "rust".into(),
        },
    ];
    model.symbols = vec![
        SymbolDef {
            name: "Store::save_state".into(),
            kind: "method".into(),
            context: "store".into(),
            file_path: "src/store/cozo.rs".into(),
            start_line: 10,
            end_line: 20,
            visibility: "private".into(),
        },
        SymbolDef {
            name: "McpHandler::handle".into(),
            kind: "method".into(),
            context: "mcp".into(),
            file_path: "src/mcp/tools.rs".into(),
            start_line: 30,
            end_line: 40,
            visibility: "public".into(),
        },
        SymbolDef {
            name: "ServerHandler::handle".into(),
            kind: "method".into(),
            context: "server".into(),
            file_path: "src/server/web.rs".into(),
            start_line: 50,
            end_line: 60,
            visibility: "public".into(),
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
            from_file: "src/mcp/write_tools.rs".into(),
            to_module: "crate::domain::rust_syn::RustSynScanner".into(),
            context: "mcp".into(),
        },
        ImportEdge {
            from_file: "src/server/web.rs".into(),
            to_module: "crate::domain::rust_syn::RustSynScanner".into(),
            context: "server".into(),
        },
        ImportEdge {
            from_file: "src/mcp/tools.rs".into(),
            to_module: "crate::store::Store".into(),
            context: "mcp".into(),
        },
        ImportEdge {
            from_file: "src/mcp/resources.rs".into(),
            to_module: "crate::store::Store".into(),
            context: "mcp".into(),
        },
        ImportEdge {
            from_file: "src/server/web.rs".into(),
            to_module: "crate::store::Store".into(),
            context: "server".into(),
        },
    ];
    model.call_edges = vec![
        CallEdge {
            caller: "Tools::sync".into(),
            callee: "save_state".into(),
            file_path: "src/mcp/tools.rs".into(),
            line: 10,
            context: "mcp".into(),
        },
        CallEdge {
            caller: "Resources::load".into(),
            callee: "save_state".into(),
            file_path: "src/mcp/resources.rs".into(),
            line: 11,
            context: "mcp".into(),
        },
        CallEdge {
            caller: "WriteTools::scan".into(),
            callee: "save_state".into(),
            file_path: "src/mcp/write_tools.rs".into(),
            line: 12,
            context: "mcp".into(),
        },
        CallEdge {
            caller: "Web::refresh".into(),
            callee: "save_state".into(),
            file_path: "src/server/web.rs".into(),
            line: 13,
            context: "server".into(),
        },
        CallEdge {
            caller: "Tools::render".into(),
            callee: "collect".into(),
            file_path: "src/mcp/tools.rs".into(),
            line: 14,
            context: "mcp".into(),
        },
        CallEdge {
            caller: "Resources::list".into(),
            callee: "collect".into(),
            file_path: "src/mcp/resources.rs".into(),
            line: 15,
            context: "mcp".into(),
        },
        CallEdge {
            caller: "WriteTools::schema".into(),
            callee: "collect".into(),
            file_path: "src/mcp/write_tools.rs".into(),
            line: 16,
            context: "mcp".into(),
        },
        CallEdge {
            caller: "Web::nodes".into(),
            callee: "collect".into(),
            file_path: "src/server/web.rs".into(),
            line: 17,
            context: "server".into(),
        },
        CallEdge {
            caller: "test_save_desired_a".into(),
            callee: "save_desired".into(),
            file_path: "src/store/cozo_tests.rs".into(),
            line: 20,
            context: "store".into(),
        },
        CallEdge {
            caller: "test_save_desired_b".into(),
            callee: "save_desired".into(),
            file_path: "src/store/cozo_tests.rs".into(),
            line: 21,
            context: "store".into(),
        },
        CallEdge {
            caller: "test_save_desired_c".into(),
            callee: "save_desired".into(),
            file_path: "src/store/cozo_tests.rs".into(),
            line: 22,
            context: "store".into(),
        },
        CallEdge {
            caller: "test_save_desired_d".into(),
            callee: "save_desired".into(),
            file_path: "src/store/cozo_tests.rs".into(),
            line: 23,
            context: "store".into(),
        },
    ];
    model.ast_edges = vec![ASTEdge {
        from_node: "Store".into(),
        to_node: "RepositoryPort".into(),
        edge_type: "implements".into(),
        file_path: "src/store/cozo.rs".into(),
        line: 15,
    }];
    store.save_actual(ws, &model).unwrap();

    let result = store.optimization_recommendations(ws).unwrap();
    let recommendations = result["recommendations"].as_array().unwrap();
    let kinds: BTreeSet<_> = recommendations
        .iter()
        .filter_map(|recommendation| recommendation["kind"].as_str())
        .collect();

    assert!(
        kinds.contains("facade"),
        "expected facade candidate: {result}"
    );
    assert!(
        kinds.contains("map_reduce"),
        "expected map/reduce candidate: {result}"
    );
    assert!(
        kinds.contains("actor_boundary"),
        "expected actor candidate: {result}"
    );
    assert!(
        kinds.contains("rename"),
        "expected rename candidate: {result}"
    );
    assert!(
        kinds.contains("move_or_facade"),
        "expected move/facade candidate: {result}"
    );
    assert!(
        kinds.contains("port_adapter_review"),
        "expected port/adapter candidate: {result}"
    );

    assert!(
        recommendations
            .iter()
            .any(|recommendation| recommendation["target"] == "save_desired"),
        "all-scope recommendations should include test-only fan-in: {result}"
    );
    assert!(
        recommendations
            .iter()
            .all(|recommendation| recommendation["target"] != "collect"),
        "bare std collection calls should not become map/reduce recommendations: {result}"
    );
    assert!(
        recommendations.iter().all(|recommendation| {
            recommendation["kind"] != "facade" || recommendation["target"] != "store"
        }),
        "root facade imports through crate::store should not be flagged as leaks: {result}"
    );

    let production_result = store
        .optimization_recommendations_scoped(ws, "production")
        .unwrap();
    let production_recommendations = production_result["recommendations"].as_array().unwrap();
    assert_eq!(production_result["scope"], "production");
    assert!(
        production_recommendations
            .iter()
            .all(|recommendation| recommendation["target"] != "save_desired"),
        "production-scope recommendations should exclude test-only fan-in: {production_result}"
    );
}

#[test]
fn optimization_recommendations_prefer_resolved_calls_for_move_candidates() {
    let store = temp_store();
    let ws = "/tmp/resolved_shape_ws";
    let mut model = DomainModel::empty(ws);
    model.source_files = vec![
        SourceFile {
            path: "src/domain/rust_facts.rs".into(),
            context: "domain".into(),
            language: "rust".into(),
        },
        SourceFile {
            path: "src/mcp/tools.rs".into(),
            context: "mcp".into(),
            language: "rust".into(),
        },
        SourceFile {
            path: "src/store/cozo.rs".into(),
            context: "store".into(),
            language: "rust".into(),
        },
    ];
    model.symbols = vec![
        SymbolDef {
            name: "RustScanScope::as_str".into(),
            kind: "method".into(),
            context: "domain".into(),
            file_path: "src/domain/rust_facts.rs".into(),
            start_line: 28,
            end_line: 34,
            visibility: "public".into(),
        },
        SymbolDef {
            name: "Value::as_str".into(),
            kind: "method".into(),
            context: "mcp".into(),
            file_path: "src/mcp/tools.rs".into(),
            start_line: 10,
            end_line: 12,
            visibility: "public".into(),
        },
        SymbolDef {
            name: "Tools::parse_scope".into(),
            kind: "method".into(),
            context: "mcp".into(),
            file_path: "src/mcp/tools.rs".into(),
            start_line: 20,
            end_line: 30,
            visibility: "private".into(),
        },
        SymbolDef {
            name: "Store::render_scope".into(),
            kind: "method".into(),
            context: "store".into(),
            file_path: "src/store/cozo.rs".into(),
            start_line: 40,
            end_line: 50,
            visibility: "private".into(),
        },
        SymbolDef {
            name: "Store::save_state".into(),
            kind: "method".into(),
            context: "store".into(),
            file_path: "src/store/cozo.rs".into(),
            start_line: 60,
            end_line: 70,
            visibility: "private".into(),
        },
        SymbolDef {
            name: "Tools::sync_a".into(),
            kind: "method".into(),
            context: "mcp".into(),
            file_path: "src/mcp/tools.rs".into(),
            start_line: 80,
            end_line: 90,
            visibility: "private".into(),
        },
        SymbolDef {
            name: "Tools::sync_b".into(),
            kind: "method".into(),
            context: "mcp".into(),
            file_path: "src/mcp/tools.rs".into(),
            start_line: 91,
            end_line: 100,
            visibility: "private".into(),
        },
        SymbolDef {
            name: "Tools::sync_c".into(),
            kind: "method".into(),
            context: "mcp".into(),
            file_path: "src/mcp/tools.rs".into(),
            start_line: 101,
            end_line: 110,
            visibility: "private".into(),
        },
        SymbolDef {
            name: "test_save_state_a".into(),
            kind: "function".into(),
            context: "store".into(),
            file_path: "src/store/cozo_tests.rs".into(),
            start_line: 120,
            end_line: 130,
            visibility: "private".into(),
        },
        SymbolDef {
            name: "test_save_state_b".into(),
            kind: "function".into(),
            context: "store".into(),
            file_path: "src/store/cozo_tests.rs".into(),
            start_line: 131,
            end_line: 140,
            visibility: "private".into(),
        },
        SymbolDef {
            name: "test_save_state_c".into(),
            kind: "function".into(),
            context: "store".into(),
            file_path: "src/store/cozo_tests.rs".into(),
            start_line: 141,
            end_line: 150,
            visibility: "private".into(),
        },
    ];
    model.call_edges = vec![
        CallEdge {
            caller: "Tools::parse_scope".into(),
            callee: "as_str".into(),
            file_path: "src/mcp/tools.rs".into(),
            line: 24,
            context: "mcp".into(),
        },
        CallEdge {
            caller: "Tools::parse_scope".into(),
            callee: "as_str".into(),
            file_path: "src/mcp/tools.rs".into(),
            line: 25,
            context: "mcp".into(),
        },
        CallEdge {
            caller: "Store::render_scope".into(),
            callee: "as_str".into(),
            file_path: "src/store/cozo.rs".into(),
            line: 44,
            context: "store".into(),
        },
    ];
    store.save_actual(ws, &model).unwrap();
    store
        .save_resolved_calls(
            ws,
            &[
                crate::domain::rust_analyzer::ResolvedCall {
                    caller: "Tools::parse_scope".into(),
                    callee: "Value::as_str".into(),
                    callee_file: "src/mcp/tools.rs".into(),
                    callee_line: 10,
                    ..Default::default()
                },
                crate::domain::rust_analyzer::ResolvedCall {
                    caller: "Store::render_scope".into(),
                    callee: "RustScanScope::as_str".into(),
                    callee_file: "src/domain/rust_facts.rs".into(),
                    callee_line: 28,
                    ..Default::default()
                },
                crate::domain::rust_analyzer::ResolvedCall {
                    caller: "Tools::sync_a".into(),
                    callee: "Store::save_state".into(),
                    callee_file: "src/store/cozo.rs".into(),
                    callee_line: 60,
                    ..Default::default()
                },
                crate::domain::rust_analyzer::ResolvedCall {
                    caller: "Tools::sync_b".into(),
                    callee: "Store::save_state".into(),
                    callee_file: "src/store/cozo.rs".into(),
                    callee_line: 60,
                    ..Default::default()
                },
                crate::domain::rust_analyzer::ResolvedCall {
                    caller: "Tools::sync_c".into(),
                    callee: "Store::save_state".into(),
                    callee_file: "src/store/cozo.rs".into(),
                    callee_line: 60,
                    ..Default::default()
                },
                crate::domain::rust_analyzer::ResolvedCall {
                    caller: "test_save_state_a".into(),
                    callee: "Store::save_state".into(),
                    callee_file: "src/store/cozo.rs".into(),
                    callee_line: 60,
                    ..Default::default()
                },
                crate::domain::rust_analyzer::ResolvedCall {
                    caller: "test_save_state_b".into(),
                    callee: "Store::save_state".into(),
                    callee_file: "src/store/cozo.rs".into(),
                    callee_line: 60,
                    ..Default::default()
                },
                crate::domain::rust_analyzer::ResolvedCall {
                    caller: "test_save_state_c".into(),
                    callee: "Store::save_state".into(),
                    callee_file: "src/store/cozo.rs".into(),
                    callee_line: 60,
                    ..Default::default()
                },
            ],
        )
        .unwrap();

    let result = store
        .optimization_recommendations_scoped(ws, "production")
        .unwrap();
    let recommendations = result["recommendations"].as_array().unwrap();

    assert!(
        recommendations.iter().all(|recommendation| {
            recommendation["kind"] != "move_or_facade"
                || recommendation["target"] != "RustScanScope::as_str"
        }),
        "resolved calls should prevent noisy short-name as_str fan-in: {result}"
    );
    assert!(
        recommendations
            .iter()
            .any(|recommendation| recommendation["kind"] == "move_or_facade"
                && recommendation["target"] == "Store::save_state"
                && recommendation["evidence"]["caller_context_counts"]["mcp"] == 3
                && recommendation["evidence"]["call_graph_relation"] == "resolved_call"),
        "move/facade recommendations should disclose resolved-call evidence and exclude test callers in production scope: {result}"
    );
}

#[test]
fn practice_findings_rank_rust_best_practice_signals() {
    let store = temp_store();
    let ws = "/tmp/practice_ws";
    let mut model = DomainModel::empty(ws);
    model.symbols = vec![SymbolDef {
        name: "Store::save_state".into(),
        kind: "method".into(),
        context: "store".into(),
        file_path: "src/store/cozo.rs".into(),
        start_line: 10,
        end_line: 20,
        visibility: "private".into(),
    }];
    model.call_edges = vec![
        CallEdge {
            caller: "Tools::sync".into(),
            callee: "save_state".into(),
            file_path: "src/mcp/tools.rs".into(),
            line: 10,
            context: "mcp".into(),
        },
        CallEdge {
            caller: "Resources::load".into(),
            callee: "save_state".into(),
            file_path: "src/mcp/resources.rs".into(),
            line: 11,
            context: "mcp".into(),
        },
        CallEdge {
            caller: "WriteTools::scan".into(),
            callee: "save_state".into(),
            file_path: "src/mcp/write_tools.rs".into(),
            line: 12,
            context: "mcp".into(),
        },
        CallEdge {
            caller: "Web::refresh".into(),
            callee: "save_state".into(),
            file_path: "src/server/web.rs".into(),
            line: 13,
            context: "server".into(),
        },
        CallEdge {
            caller: "Store::load".into(),
            callee: "unwrap".into(),
            file_path: "src/store/cozo.rs".into(),
            line: 30,
            context: "store".into(),
        },
        CallEdge {
            caller: "test_load_fixture".into(),
            callee: "unwrap".into(),
            file_path: "src/store/cozo_tests.rs".into(),
            line: 31,
            context: "store".into(),
        },
    ];
    model.ast_edges = vec![
        ASTEdge {
            from_node: "Store::legacy".into(),
            to_node: "allow(dead_code)".into(),
            edge_type: "decorators".into(),
            file_path: "src/store/cozo.rs".into(),
            line: 8,
        },
        ASTEdge {
            from_node: "test_load_fixture".into(),
            to_node: "test".into(),
            edge_type: "decorators".into(),
            file_path: "src/store/cozo_tests.rs".into(),
            line: 29,
        },
    ];
    model.reference_edges = vec![ReferenceEdge {
        from_file: "src/store/cozo.rs".into(),
        to_path: "panic".into(),
        reference_kind: "macro".into(),
        line: 35,
        context: "store".into(),
    }];
    store.save_actual(ws, &model).unwrap();

    let result = store.practice_findings(ws).unwrap();
    let findings = result["findings"].as_array().unwrap();
    let kinds: BTreeSet<_> = findings
        .iter()
        .filter_map(|finding| finding["kind"].as_str())
        .collect();

    assert_eq!(result["analysis"], "practice_findings");
    assert!(
        kinds.contains("panic_macro"),
        "expected panic finding: {result}"
    );
    assert!(
        kinds.contains("unchecked_unwrap"),
        "expected unwrap finding: {result}"
    );
    assert!(
        kinds.contains("lint_suppression_stale_code"),
        "expected lint suppression finding: {result}"
    );
    assert!(
        kinds.contains("high_fan_in_private_symbol"),
        "expected fan-in finding: {result}"
    );
    assert!(
        findings
            .windows(2)
            .all(|pair| pair[0]["priority_score"].as_i64().unwrap()
                >= pair[1]["priority_score"].as_i64().unwrap()),
        "findings should be score-sorted: {result}"
    );
    let production_unwrap = findings
        .iter()
        .find(|finding| finding["target"] == "Store::load->unwrap")
        .expect("production unwrap finding");
    let test_unwrap = findings
        .iter()
        .find(|finding| finding["target"] == "test_load_fixture->unwrap")
        .expect("test unwrap finding");
    assert_eq!(production_unwrap["scope"], "production");
    assert_eq!(test_unwrap["scope"], "test");
    assert_eq!(test_unwrap["severity"], "info");
    assert_eq!(result["summary"]["actionable_count"], 4);
    assert_eq!(result["summary"]["test_count"], 1);
    assert_eq!(result["summary"]["production_count"], 4);
    assert!(
        production_unwrap["priority_score"].as_i64().unwrap()
            > test_unwrap["priority_score"].as_i64().unwrap(),
        "test-only findings should not outrank production findings: {result}"
    );

    let production_result = store.practice_findings_scoped(ws, "production").unwrap();
    let production_findings = production_result["findings"].as_array().unwrap();
    assert_eq!(production_result["scope"], "production");
    assert_eq!(production_result["summary"]["test_count"], 0);
    assert!(
        production_findings
            .iter()
            .all(|finding| finding["scope"] == "production"),
        "production-scope findings should exclude tests: {production_result}"
    );

    let all_test_edges = store
        .query_rust_graph(
            ws,
            &json!({ "view": "edges", "relation": "ast_edge", "to": "test", "scope": "all" }),
        )
        .unwrap();
    assert_eq!(all_test_edges["count"], 1);

    let production_test_edges = store
        .query_rust_graph(
            ws,
            &json!({ "view": "edges", "relation": "ast_edge", "to": "test", "scope": "production" }),
        )
        .unwrap();
    assert_eq!(production_test_edges["count"], 0);
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

    store
        .save_reasoning_claims(ws, std::slice::from_ref(&claim))
        .unwrap();

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
