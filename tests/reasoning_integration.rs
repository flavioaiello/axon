//! Integration tests: first-class relational reasoning vs opaque JSON blobs.
//!
//! These tests measure what the Datalog reasoning layer can do **with**
//! sub-structures promoted to first-class relations (fields, methods,
//! params, invariants, validation rules) versus what was possible
//! **without** them (entity-level only, sub-structures hidden in JSON).
//!
//! Each test pair runs the same logical question two ways:
//!   • `*_with`:    uses field/method/invariant/vo_rule relations → answer
//!   • `*_without`: uses only entity/service/event headers → no answer (or imprecise)

use std::env::temp_dir;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

// We access the binary crate's public API through its library surface.
use axon::domain::model::*;
use axon::store::Store;

// ── Helpers ────────────────────────────────────────────────────────────────

fn temp_store() -> Store {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let id = CTR.fetch_add(1, Ordering::SeqCst);
    let path = temp_dir().join(format!("axon_integ_{}_{}.db", std::process::id(), id));
    Store::open(&path).unwrap()
}

fn ws() -> String {
    format!("/tmp/integ-{}", std::process::id())
}

/// A realistic e-commerce domain with enough sub-structure to exercise
/// cross-cutting Datalog queries that are impossible with JSON blobs.
fn ecommerce_model() -> DomainModel {
    DomainModel {
        name: "ECommerce".into(),
        description: "E-commerce platform".into(),
        bounded_contexts: vec![
            BoundedContext {
                name: "Catalog".into(),
                description: "Product catalog management".into(),
                module_path: "src/catalog".into(),
                ownership: Ownership::default(),
                aggregates: vec![],
                policies: vec![],
                read_models: vec![],
                entities: vec![
                    Entity {
                        name: "Product".into(),
                        description: "A sellable product".into(),
                        aggregate_root: true,
                        fields: vec![
                            Field {
                                name: "id".into(),
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
                            Field {
                                name: "price".into(),
                                field_type: "Money".into(),
                                required: true,
                                description: "".into(),
                            },
                            Field {
                                name: "category_id".into(),
                                field_type: "CategoryId".into(),
                                required: false,
                                description: "".into(),
                            },
                        ],
                        methods: vec![
                            Method {
                                name: "create".into(),
                                description: "".into(),
                                return_type: "Product".into(),
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
                                file_path: None,
                                start_line: None,
                                end_line: None,
                            },
                            Method {
                                name: "update_price".into(),
                                description: "".into(),
                                return_type: "".into(),
                                parameters: vec![Field {
                                    name: "new_price".into(),
                                    field_type: "Money".into(),
                                    required: true,
                                    description: "".into(),
                                }],
                                file_path: None,
                                start_line: None,
                                end_line: None,
                            },
                        ],
                        invariants: vec![
                            "Name must not be empty".into(),
                            "Price must be positive".into(),
                        ],
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    },
                    Entity {
                        name: "Category".into(),
                        description: "Product category".into(),
                        aggregate_root: true,
                        fields: vec![
                            Field {
                                name: "id".into(),
                                field_type: "CategoryId".into(),
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
                        methods: vec![],
                        invariants: vec![],
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    },
                ],
                value_objects: vec![ValueObject {
                    name: "Money".into(),
                    description: "Monetary amount".into(),
                    fields: vec![
                        Field {
                            name: "amount".into(),
                            field_type: "Decimal".into(),
                            required: true,
                            description: "".into(),
                        },
                        Field {
                            name: "currency".into(),
                            field_type: "CurrencyCode".into(),
                            required: true,
                            description: "".into(),
                        },
                    ],
                    validation_rules: vec!["Amount >= 0".into(), "Currency is ISO 4217".into()],
                    file_path: None,
                    start_line: None,
                    end_line: None,
                }],
                services: vec![Service {
                    name: "CatalogService".into(),
                    description: "".into(),
                    kind: ServiceKind::Application,
                    methods: vec![
                        Method {
                            name: "list_products".into(),
                            description: "".into(),
                            return_type: "Vec<Product>".into(),
                            parameters: vec![],
                            file_path: None,
                            start_line: None,
                            end_line: None,
                        },
                        Method {
                            name: "get_product".into(),
                            description: "".into(),
                            return_type: "Product".into(),
                            parameters: vec![Field {
                                name: "id".into(),
                                field_type: "ProductId".into(),
                                required: true,
                                description: "".into(),
                            }],
                            file_path: None,
                            start_line: None,
                            end_line: None,
                        },
                    ],
                    dependencies: vec![],
                    file_path: None,
                    start_line: None,
                    end_line: None,
                }],
                repositories: vec![Repository {
                    name: "ProductRepository".into(),
                    aggregate: "Product".into(),
                    methods: vec![
                        Method {
                            name: "find_by_id".into(),
                            description: "".into(),
                            return_type: "Option<Product>".into(),
                            parameters: vec![Field {
                                name: "id".into(),
                                field_type: "ProductId".into(),
                                required: true,
                                description: "".into(),
                            }],
                            file_path: None,
                            start_line: None,
                            end_line: None,
                        },
                        Method {
                            name: "save".into(),
                            description: "".into(),
                            return_type: "".into(),
                            parameters: vec![Field {
                                name: "product".into(),
                                field_type: "Product".into(),
                                required: true,
                                description: "".into(),
                            }],
                            file_path: None,
                            start_line: None,
                            end_line: None,
                        },
                    ],
                    file_path: None,
                    start_line: None,
                    end_line: None,
                }],
                events: vec![
                    DomainEvent {
                        name: "ProductCreated".into(),
                        description: "".into(),
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
                            Field {
                                name: "price".into(),
                                field_type: "Money".into(),
                                required: true,
                                description: "".into(),
                            },
                        ],
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    },
                    DomainEvent {
                        name: "PriceChanged".into(),
                        description: "".into(),
                        source: "Product".into(),
                        fields: vec![
                            Field {
                                name: "product_id".into(),
                                field_type: "ProductId".into(),
                                required: true,
                                description: "".into(),
                            },
                            Field {
                                name: "old_price".into(),
                                field_type: "Money".into(),
                                required: true,
                                description: "".into(),
                            },
                            Field {
                                name: "new_price".into(),
                                field_type: "Money".into(),
                                required: true,
                                description: "".into(),
                            },
                        ],
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    },
                ],
                modules: vec![],
                dependencies: vec![],
                api_endpoints: vec![],
            },
            BoundedContext {
                name: "Ordering".into(),
                description: "Order management".into(),
                module_path: "src/ordering".into(),
                ownership: Ownership::default(),
                aggregates: vec![],
                policies: vec![],
                read_models: vec![],
                entities: vec![
                    Entity {
                        name: "Order".into(),
                        description: "A customer order".into(),
                        aggregate_root: true,
                        fields: vec![
                            Field {
                                name: "id".into(),
                                field_type: "OrderId".into(),
                                required: true,
                                description: "".into(),
                            },
                            Field {
                                name: "customer_id".into(),
                                field_type: "CustomerId".into(),
                                required: true,
                                description: "".into(),
                            },
                            Field {
                                name: "total".into(),
                                field_type: "Money".into(),
                                required: true,
                                description: "".into(),
                            },
                            Field {
                                name: "status".into(),
                                field_type: "OrderStatus".into(),
                                required: true,
                                description: "".into(),
                            },
                        ],
                        methods: vec![
                            Method {
                                name: "place".into(),
                                description: "".into(),
                                return_type: "Order".into(),
                                parameters: vec![
                                    Field {
                                        name: "customer_id".into(),
                                        field_type: "CustomerId".into(),
                                        required: true,
                                        description: "".into(),
                                    },
                                    Field {
                                        name: "items".into(),
                                        field_type: "Vec<OrderItem>".into(),
                                        required: true,
                                        description: "".into(),
                                    },
                                ],
                                file_path: None,
                                start_line: None,
                                end_line: None,
                            },
                            Method {
                                name: "cancel".into(),
                                description: "".into(),
                                return_type: "".into(),
                                parameters: vec![],
                                file_path: None,
                                start_line: None,
                                end_line: None,
                            },
                        ],
                        invariants: vec![
                            "Order must have at least one item".into(),
                            "Total must match item sum".into(),
                        ],
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    },
                    Entity {
                        name: "OrderItem".into(),
                        description: "A line item".into(),
                        aggregate_root: false,
                        fields: vec![
                            Field {
                                name: "product_id".into(),
                                field_type: "ProductId".into(),
                                required: true,
                                description: "".into(),
                            },
                            Field {
                                name: "quantity".into(),
                                field_type: "u32".into(),
                                required: true,
                                description: "".into(),
                            },
                            Field {
                                name: "unit_price".into(),
                                field_type: "Money".into(),
                                required: true,
                                description: "".into(),
                            },
                        ],
                        methods: vec![],
                        invariants: vec!["Quantity must be > 0".into()],
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    },
                ],
                value_objects: vec![],
                services: vec![Service {
                    name: "OrderService".into(),
                    description: "".into(),
                    kind: ServiceKind::Application,
                    methods: vec![Method {
                        name: "place_order".into(),
                        description: "".into(),
                        return_type: "Order".into(),
                        parameters: vec![
                            Field {
                                name: "customer_id".into(),
                                field_type: "CustomerId".into(),
                                required: true,
                                description: "".into(),
                            },
                            Field {
                                name: "items".into(),
                                field_type: "Vec<OrderItem>".into(),
                                required: true,
                                description: "".into(),
                            },
                        ],
                        file_path: None,
                        start_line: None,
                        end_line: None,
                    }],
                    dependencies: vec!["ProductRepository".into()],
                    file_path: None,
                    start_line: None,
                    end_line: None,
                }],
                repositories: vec![Repository {
                    name: "OrderRepository".into(),
                    aggregate: "Order".into(),
                    methods: vec![
                        Method {
                            name: "find_by_id".into(),
                            description: "".into(),
                            return_type: "Option<Order>".into(),
                            parameters: vec![Field {
                                name: "id".into(),
                                field_type: "OrderId".into(),
                                required: true,
                                description: "".into(),
                            }],
                            file_path: None,
                            start_line: None,
                            end_line: None,
                        },
                        Method {
                            name: "save".into(),
                            description: "".into(),
                            return_type: "".into(),
                            parameters: vec![Field {
                                name: "order".into(),
                                field_type: "Order".into(),
                                required: true,
                                description: "".into(),
                            }],
                            file_path: None,
                            start_line: None,
                            end_line: None,
                        },
                    ],
                    file_path: None,
                    start_line: None,
                    end_line: None,
                }],
                events: vec![DomainEvent {
                    name: "OrderPlaced".into(),
                    description: "".into(),
                    source: "Order".into(),
                    fields: vec![
                        Field {
                            name: "order_id".into(),
                            field_type: "OrderId".into(),
                            required: true,
                            description: "".into(),
                        },
                        Field {
                            name: "customer_id".into(),
                            field_type: "CustomerId".into(),
                            required: true,
                            description: "".into(),
                        },
                        Field {
                            name: "total".into(),
                            field_type: "Money".into(),
                            required: true,
                            description: "".into(),
                        },
                    ],
                    file_path: None,
                    start_line: None,
                    end_line: None,
                }],
                modules: vec![],
                dependencies: vec!["Catalog".into()],
                api_endpoints: vec![],
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

// ═══════════════════════════════════════════════════════════════════════════
//  1. Cross-cutting type usage: "Which fields use type Money?"
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn type_usage_with_first_class_fields() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();

    // WITH: Datalog can query the `field` relation directly
    let rows = store
        .run_datalog(
            "?[ctx, owner_kind, owner, field_name] := \
                *field{workspace: $ws, context: ctx, owner_kind, owner, \
                       name: field_name, field_type: 'Money', state: 'desired'}",
            &ws,
        )
        .unwrap();

    // Money appears in: Product.price, Order.total, OrderItem.unit_price,
    // ValueObject Money.amount(Decimal not Money), ProductCreated.price,
    // PriceChanged.old_price, PriceChanged.new_price
    assert!(
        rows.len() >= 5,
        "Expected >=5 fields of type 'Money', got {}: {:?}",
        rows.len(),
        rows
    );

    // Verify we can see cross-context results
    let contexts: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
    assert!(
        contexts.contains(&"Catalog"),
        "Should find Money in Catalog"
    );
    assert!(
        contexts.contains(&"Ordering"),
        "Should find Money in Ordering"
    );
}

#[test]
fn type_usage_without_first_class_fields() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();

    // WITHOUT: entity-level query can only tell us entities exist,
    // not what types their fields use. This query returns nothing useful.
    let rows = store
        .run_datalog(
            "?[ctx, name] := *entity{workspace: $ws, context: ctx, name, state: 'desired'}",
            &ws,
        )
        .unwrap();

    // We get entity names but CANNOT determine which use Money.
    // The query returns all entities regardless of field types.
    assert_eq!(rows.len(), 4); // Product, Category, Order, OrderItem
    // No way to filter by field type — the information is opaque.
}

// ═══════════════════════════════════════════════════════════════════════════
//  2. Cross-entity method search: "Find all methods named 'find_by_id'"
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn method_search_with_first_class_methods() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();

    // WITH: query the unified `method` relation across all owner types
    let rows = store
        .run_datalog(
            "?[ctx, owner_kind, owner, return_type] := \
                *method{workspace: $ws, context: ctx, owner_kind, owner, \
                        name: 'find_by_id', state: 'desired', return_type}",
            &ws,
        )
        .unwrap();

    // find_by_id exists on ProductRepository and OrderRepository
    assert_eq!(rows.len(), 2, "Expected 2 find_by_id methods: {:?}", rows);
    let owners: Vec<&str> = rows.iter().map(|r| r[2].as_str()).collect();
    assert!(owners.contains(&"ProductRepository"));
    assert!(owners.contains(&"OrderRepository"));
}

#[test]
fn method_search_without_first_class_methods() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();

    // WITHOUT: can only query service/repository headers — no method visibility
    let rows = store
        .run_datalog(
            "?[ctx, name] := *repository{workspace: $ws, context: ctx, name, state: 'desired'}",
            &ws,
        )
        .unwrap();

    assert_eq!(rows.len(), 2); // ProductRepository, OrderRepository
    // We know repositories exist but CANNOT search their methods.
}

// ═══════════════════════════════════════════════════════════════════════════
//  3. Parameter type analysis: "Which methods accept CustomerId?"
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn param_analysis_with_first_class_params() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();

    let rows = store
        .run_datalog(
            "?[ctx, owner_kind, owner, method] := \
                *method_param{workspace: $ws, context: ctx, owner_kind, owner, method, \
                              param_type: 'CustomerId', state: 'desired'}",
            &ws,
        )
        .unwrap();

    // CustomerId params: Order.place(customer_id), OrderService.place_order(customer_id)
    assert!(
        rows.len() >= 2,
        "Expected >=2 methods accepting CustomerId, got {}: {:?}",
        rows.len(),
        rows
    );
}

#[test]
fn param_analysis_without_first_class_params() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();

    // WITHOUT: no method_param table, no way to query parameter types
    // Best we can do is list all services
    let rows = store
        .run_datalog(
            "?[ctx, name, kind] := *service{workspace: $ws, context: ctx, name, kind, state: 'desired'}",
            &ws,
        )
        .unwrap();

    assert_eq!(rows.len(), 2); // CatalogService, OrderService
    // Cannot determine which accept CustomerId — parameters are invisible.
}

// ═══════════════════════════════════════════════════════════════════════════
//  4. Invariant coverage: "Aggregate roots without invariants" using
//     first-class invariant relation vs entity-level-only
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn invariant_coverage_with_first_class_invariants() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();
    let canonical = axon::store::canonicalize_path(&ws);

    let missing = store
        .aggregate_roots_without_invariants(&canonical)
        .unwrap();
    // Category is aggregate_root=true but has NO invariants → should be flagged
    assert_eq!(missing.len(), 1, "Expected 1 missing: {:?}", missing);
    assert_eq!(missing[0].1, "Category");
}

#[test]
fn invariant_coverage_without_first_class_invariants() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();

    // WITHOUT: we could only know aggregate_root=true from the entity header,
    // but couldn't verify invariant text content or count via Datalog.
    let rows = store
        .run_datalog(
            "?[ctx, name] := *entity{workspace: $ws, context: ctx, name, \
                                     aggregate_root: true, state: 'desired'}",
            &ws,
        )
        .unwrap();

    // Returns all aggregate roots: Product, Category, Order (3)
    assert_eq!(rows.len(), 3);
    // Without first-class invariant relation, we cannot cross-reference
    // to determine which of these are missing invariants.
}

// ═══════════════════════════════════════════════════════════════════════════
//  5. Field-level diff: Adding a field should appear in diff_graph
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn field_level_diff_with_first_class_fields() {
    let store = temp_store();
    let ws = ws();
    let model = ecommerce_model();
    store.save_desired(&ws, &model).unwrap();
    store.accept(&ws).unwrap();

    // Add a field to Product
    let mut modified = ecommerce_model();
    modified.bounded_contexts[0].entities[0].fields.push(Field {
        name: "sku".into(),
        field_type: "String".into(),
        required: true,
        description: "Stock keeping unit".into(),
    });
    store.save_desired(&ws, &modified).unwrap();

    let diff = store.diff_graph(&ws).unwrap();
    let changes = diff["pending_changes"].as_array().unwrap();

    // WITH: diff_graph detects field-level add
    let field_adds: Vec<_> = changes
        .iter()
        .filter(|c| c["kind"] == "field" && c["action"] == "add")
        .collect();
    assert_eq!(field_adds.len(), 1, "Expected 1 field add: {:?}", changes);
    assert_eq!(field_adds[0]["name"], "sku");
    assert_eq!(field_adds[0]["owner"], "Product");
    assert_eq!(field_adds[0]["owner_kind"], "entity");
}

#[test]
fn field_level_diff_without_first_class_fields() {
    let store = temp_store();
    let ws = ws();
    let model = ecommerce_model();
    store.save_desired(&ws, &model).unwrap();
    store.accept(&ws).unwrap();

    // Add a field — from entity-level perspective, Product still exists,
    // so a header-only diff would show ZERO changes.
    let mut modified = ecommerce_model();
    modified.bounded_contexts[0].entities[0].fields.push(Field {
        name: "sku".into(),
        field_type: "String".into(),
        required: true,
        description: "".into(),
    });
    store.save_desired(&ws, &modified).unwrap();

    let diff = store.diff_graph(&ws).unwrap();
    let changes = diff["pending_changes"].as_array().unwrap();

    // Entity-level changes: none (Product still exists in both states)
    let entity_changes: Vec<_> = changes.iter().filter(|c| c["kind"] == "entity").collect();
    assert!(
        entity_changes.is_empty(),
        "Entity-level diff cannot see field changes: {:?}",
        entity_changes
    );

    // But field-level picks it up (WITH first-class relations, the diff IS detected)
    let field_changes: Vec<_> = changes.iter().filter(|c| c["kind"] == "field").collect();
    assert!(
        !field_changes.is_empty(),
        "Field-level diff detects the change"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
//  6. Method-level diff: Adding a method should appear in diff_graph
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn method_level_diff_with_first_class_methods() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();
    store.accept(&ws).unwrap();

    let mut modified = ecommerce_model();
    modified.bounded_contexts[0].services[0]
        .methods
        .push(Method {
            name: "search_products".into(),
            description: "Full-text search".into(),
            parameters: vec![Field {
                name: "query".into(),
                field_type: "String".into(),
                required: true,
                description: "".into(),
            }],
            return_type: "Vec<Product>".into(),
            file_path: None,
            start_line: None,
            end_line: None,
        });
    store.save_desired(&ws, &modified).unwrap();

    let diff = store.diff_graph(&ws).unwrap();
    let changes = diff["pending_changes"].as_array().unwrap();

    let method_adds: Vec<_> = changes
        .iter()
        .filter(|c| c["kind"] == "method" && c["action"] == "add")
        .collect();
    assert_eq!(method_adds.len(), 1);
    assert_eq!(method_adds[0]["name"], "search_products");
    assert_eq!(method_adds[0]["owner_kind"], "service");
}

// ═══════════════════════════════════════════════════════════════════════════
//  7. Invariant-level diff: Adding an invariant should appear in diff_graph
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn invariant_level_diff_with_first_class_invariants() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();
    store.accept(&ws).unwrap();

    let mut modified = ecommerce_model();
    modified.bounded_contexts[0].entities[0]
        .invariants
        .push("SKU must be unique".into());
    store.save_desired(&ws, &modified).unwrap();

    let diff = store.diff_graph(&ws).unwrap();
    let changes = diff["pending_changes"].as_array().unwrap();

    let inv_adds: Vec<_> = changes
        .iter()
        .filter(|c| c["kind"] == "invariant" && c["action"] == "add")
        .collect();
    assert_eq!(inv_adds.len(), 1);
    assert_eq!(inv_adds[0]["name"], "SKU must be unique");
    assert_eq!(inv_adds[0]["owner"], "Product");
}

// ═══════════════════════════════════════════════════════════════════════════
//  8. Cross-context join: "Events carrying a ProductId field"
//     — joins event → field in a single Datalog query
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn cross_context_event_field_join_with_first_class() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();

    let rows = store
        .run_datalog(
            "?[ctx, event_name, field_name] := \
                *event{workspace: $ws, context: ctx, name: event_name, state: 'desired'}, \
                *field{workspace: $ws, context: ctx, owner_kind: 'event', owner: event_name, \
                       name: field_name, field_type: 'ProductId', state: 'desired'}",
            &ws,
        )
        .unwrap();

    // ProductCreated.product_id, PriceChanged.product_id, OrderItem references
    // but OrderItem is entity not event. Events with ProductId:
    //   ProductCreated.product_id, PriceChanged.product_id
    assert!(
        rows.len() >= 2,
        "Expected >=2 events with ProductId field: {:?}",
        rows
    );
}

// ═══════════════════════════════════════════════════════════════════════════
//  9. Validation rule reasoning: "Value objects without validation rules"
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn vo_validation_rules_with_first_class() {
    let store = temp_store();
    let ws = ws();
    store.save_desired(&ws, &ecommerce_model()).unwrap();

    // Query value objects that HAVE validation rules
    let with_rules = store
        .run_datalog(
            "has_rule[ctx, vo] := *vo_rule{workspace: $ws, context: ctx, value_object: vo, state: 'desired'} \
             ?[ctx, name] := *value_object{workspace: $ws, context: ctx, name, state: 'desired'}, has_rule[ctx, name]",
            &ws,
        )
        .unwrap();
    assert_eq!(with_rules.len(), 1); // Money has 2 rules
    assert_eq!(with_rules[0][1], "Money");

    // Query value objects WITHOUT validation rules
    let without_rules = store
        .run_datalog(
            "has_rule[ctx, vo] := *vo_rule{workspace: $ws, context: ctx, value_object: vo, state: 'desired'} \
             ?[ctx, name] := *value_object{workspace: $ws, context: ctx, name, state: 'desired'}, not has_rule[ctx, name]",
            &ws,
        )
        .unwrap();
    // Our model only has Money VO which has rules, so this should be empty
    assert!(without_rules.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════
// 10. Performance: save + load + diff cycle timing
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn perf_save_load_diff_cycle() {
    let store = temp_store();
    let ws = ws();
    let model = ecommerce_model();

    // Measure save
    let t0 = Instant::now();
    store.save_desired(&ws, &model).unwrap();
    let save_us = t0.elapsed().as_micros();

    // Measure load
    let t1 = Instant::now();
    let loaded = store.load_desired(&ws).unwrap().unwrap();
    let load_us = t1.elapsed().as_micros();

    // Measure accept
    let t2 = Instant::now();
    store.accept(&ws).unwrap();
    let accept_us = t2.elapsed().as_micros();

    // Measure diff (after modification)
    let mut modified = ecommerce_model();
    modified.bounded_contexts[0].entities[0].fields.push(Field {
        name: "sku".into(),
        field_type: "String".into(),
        required: false,
        description: "".into(),
    });
    store.save_desired(&ws, &modified).unwrap();
    let t3 = Instant::now();
    let diff = store.diff_graph(&ws).unwrap();
    let diff_us = t3.elapsed().as_micros();

    // Assertions: operations complete, results are correct
    assert_eq!(loaded.bounded_contexts.len(), 2);
    let changes = diff["pending_changes"].as_array().unwrap();
    assert!(!changes.is_empty());

    // Print timing (visible with `cargo test -- --nocapture`)
    eprintln!("── Performance (first-class relations) ──");
    eprintln!("  save_desired : {:>8} µs", save_us);
    eprintln!("  load_desired : {:>8} µs", load_us);
    eprintln!("  accept       : {:>8} µs", accept_us);
    eprintln!("  diff_graph   : {:>8} µs", diff_us);
    eprintln!(
        "  total        : {:>8} µs",
        save_us + load_us + accept_us + diff_us
    );
    eprintln!("  relations    : field, method, method_param, invariant, vo_rule");
    eprintln!("  diff changes : {}", changes.len());
}

// ═══════════════════════════════════════════════════════════════════════════
// 11. Scale: N-context model with sub-structures
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn perf_scale_10_contexts() {
    let store = temp_store();
    let ws = ws();

    // Generate a model with 10 contexts, each with 3 entities × 5 fields × 2 methods
    let mut contexts = Vec::new();
    for i in 0..10 {
        let mut entities = Vec::new();
        for j in 0..3 {
            let fields: Vec<Field> = (0..5)
                .map(|k| Field {
                    name: format!("field_{k}"),
                    field_type: if k == 0 {
                        format!("Ctx{i}Entity{j}Id")
                    } else {
                        "String".into()
                    },
                    required: k < 2,
                    description: "".into(),
                })
                .collect();
            let methods: Vec<Method> = (0..2)
                .map(|k| Method {
                    name: format!("method_{k}"),
                    description: "".into(),
                    return_type: "".into(),
                    parameters: vec![Field {
                        name: "arg".into(),
                        field_type: "String".into(),
                        required: true,
                        description: "".into(),
                    }],
                    file_path: None,
                    start_line: None,
                    end_line: None,
                })
                .collect();
            entities.push(Entity {
                name: format!("Entity{j}"),
                description: "".into(),
                aggregate_root: j == 0,
                fields,
                methods,
                invariants: if j == 0 {
                    vec!["Must be valid".into()]
                } else {
                    vec![]
                },
                file_path: None,
                start_line: None,
                end_line: None,
            });
        }
        contexts.push(BoundedContext {
            name: format!("Context{i}"),
            description: "".into(),
            module_path: format!("src/ctx{i}"),
            ownership: Ownership::default(),
            aggregates: vec![],
            policies: vec![],
            read_models: vec![],
            entities,
            value_objects: vec![],
            services: vec![],
            repositories: vec![],
            events: vec![],
            modules: vec![],
            dependencies: if i > 0 {
                vec![format!("Context{}", i - 1)]
            } else {
                vec![]
            },
            api_endpoints: vec![],
        });
    }

    let model = DomainModel {
        name: "ScaleTest".into(),
        description: "".into(),
        bounded_contexts: contexts,
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
    };

    // Total: 10 contexts × 3 entities × (5 fields + 2 methods + 2 params) = 270 sub-structure rows
    let t0 = Instant::now();
    store.save_desired(&ws, &model).unwrap();
    let save_us = t0.elapsed().as_micros();

    let t1 = Instant::now();
    let loaded = store.load_desired(&ws).unwrap().unwrap();
    let load_us = t1.elapsed().as_micros();

    let t2 = Instant::now();
    store.accept(&ws).unwrap();
    let accept_us = t2.elapsed().as_micros();

    // Verify round-trip fidelity
    assert_eq!(loaded.bounded_contexts.len(), 10);
    for (i, _bc) in loaded.bounded_contexts.iter().enumerate() {
        // Contexts are keyed by name — find by name, not index
        let ctx = loaded
            .bounded_contexts
            .iter()
            .find(|c| c.name == format!("Context{i}"))
            .unwrap();
        assert_eq!(ctx.entities.len(), 3, "Context{i} entity count");
        for entity in &ctx.entities {
            assert_eq!(entity.fields.len(), 5, "Entity field count in Context{i}");
            assert_eq!(entity.methods.len(), 2, "Entity method count in Context{i}");
            for method in &entity.methods {
                assert_eq!(method.parameters.len(), 1, "Method param count");
            }
        }
    }

    // Cross-cutting query at scale: all fields of type String
    let t3 = Instant::now();
    let string_fields = store
        .run_datalog(
            "?[ctx, owner, name] := *field{workspace: $ws, context: ctx, owner_kind: 'entity', \
                                           owner, name, field_type: 'String', state: 'desired'}",
            &ws,
        )
        .unwrap();
    let query_us = t3.elapsed().as_micros();

    // Each entity has 4 String fields (field_1..4), 3 entities × 10 contexts = 120
    assert_eq!(string_fields.len(), 120, "Expected 120 String fields");

    eprintln!("── Scale: 10 contexts × 3 entities × 9 sub-rows ──");
    eprintln!("  save_desired   : {:>8} µs", save_us);
    eprintln!("  load_desired   : {:>8} µs", load_us);
    eprintln!("  accept         : {:>8} µs", accept_us);
    eprintln!(
        "  cross-cut query: {:>8} µs  (120 String fields found)",
        query_us
    );
}
