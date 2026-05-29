use crate::domain::model::DomainModel;
use crate::mcp::protocol::*;
use crate::mcp::tools::build_model_overview;
use crate::store::Store;

/// Returns the list of resources the Axon server exposes.
pub fn list_resources(store: &Store, workspace_path: &str) -> Vec<ResourceDefinition> {
    let mut resources = vec![
        ResourceDefinition {
            uri: "axon://architecture/overview".into(),
            name: "Architecture Overview".into(),
            description: "Architecture overview of the implemented Rust model with ontology contract, overlays, health inputs, and rules".into(),
            mime_type: "application/json".into(),
        },
        ResourceDefinition {
            uri: "axon://rust/ontology".into(),
            name: "Rust Ontology".into(),
            description: "Rust-native facts and overview projection: modules, structs, symbols, imports, calls, and query guidance".into(),
            mime_type: "application/json".into(),
        },
        ResourceDefinition {
            uri: "axon://architecture/rules".into(),
            name: "Architectural Rules".into(),
            description: "All architectural constraints and rules".into(),
            mime_type: "application/json".into(),
        },
        ResourceDefinition {
            uri: "axon://architecture/conventions".into(),
            name: "Conventions".into(),
            description: "Naming, file structure, error handling, and testing conventions".into(),
            mime_type: "application/json".into(),
        },
    ];

    // Add compatibility semantic-overlay resources from Datalog.
    let contexts = store
        .run_datalog(
            "?[name] := *context{workspace: $ws, name, state: 'actual'}",
            workspace_path,
        )
        .unwrap_or_default();

    for row in &contexts {
        let ctx_name = &row[0];
        resources.push(ResourceDefinition {
            uri: format!("axon://context/{}", ctx_name.to_lowercase()),
            name: format!("Semantic Overlay: {}", ctx_name),
            description: format!(
                "Compatibility semantic overlay '{}' — entities, services, events",
                ctx_name
            ),
            mime_type: "application/json".into(),
        });
    }

    resources
}

/// Reads a resource by URI, backed by Datalog relations.
pub fn read_resource(store: &Store, workspace_path: &str, uri: &str) -> ResourceReadResult {
    let (mime, text) = match uri {
        "axon://architecture/overview" => {
            let overview = build_model_overview(store, workspace_path, "actual");
            (
                "application/json",
                serde_json::to_string_pretty(&overview).unwrap_or_default(),
            )
        }
        "axon://rust/ontology" => {
            let overview = build_model_overview(store, workspace_path, "actual");
            (
                "application/json",
                serde_json::to_string_pretty(&overview["rust_ontology"]).unwrap_or_default(),
            )
        }
        "axon://architecture/rules" => {
            let model = load_model(store, workspace_path);
            (
                "application/json",
                serde_json::to_string(&model.rules).unwrap_or_default(),
            )
        }
        "axon://architecture/conventions" => {
            let model = load_model(store, workspace_path);
            (
                "application/json",
                serde_json::to_string(&model.conventions).unwrap_or_default(),
            )
        }
        _ if uri.starts_with("axon://context/") => {
            let ctx_name = uri.strip_prefix("axon://context/").unwrap_or("");
            read_context_from_store(store, workspace_path, ctx_name)
        }
        _ => ("text/plain", format!("Unknown resource: {}", uri)),
    };

    ResourceReadResult {
        contents: vec![ResourceContent {
            uri: uri.to_string(),
            mime_type: mime.to_string(),
            text,
        }],
    }
}

/// Load the implemented model from store for rules/conventions that aren't yet in Datalog relations.
fn load_model(store: &Store, workspace_path: &str) -> DomainModel {
    store
        .load_desired(workspace_path)
        .ok()
        .flatten()
        .unwrap_or_else(|| DomainModel::empty(workspace_path))
}

/// Read a single bounded context from Datalog, assembling its sub-structures.
fn read_context_from_store(
    store: &Store,
    workspace_path: &str,
    ctx_name: &str,
) -> (&'static str, String) {
    // Check if context exists
    let ctx_rows = store.run_datalog(
        "?[name, description] := *context{workspace: $ws, name, description, state: 'actual'}, \
         name = to_lowercase($ctx)",
        workspace_path,
    );

    // We need a custom query with ctx_name bound. Use build_model_overview which already
    // gathers everything, then extract the specific context.
    let overview = build_model_overview(store, workspace_path, "actual");
    if let Some(contexts) = overview.get("bounded_contexts").and_then(|v| v.as_array()) {
        for ctx in contexts {
            if let Some(name) = ctx.get("name").and_then(|v| v.as_str())
                && name.eq_ignore_ascii_case(ctx_name)
            {
                return (
                    "application/json",
                    serde_json::to_string_pretty(ctx).unwrap_or_default(),
                );
            }
        }
    }

    // Also check by exact query
    let _ = ctx_rows;
    (
        "text/plain",
        format!("Bounded context '{}' not found", ctx_name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::*;
    use crate::store::Store;
    use std::env::temp_dir;

    fn test_store_with_model() -> (Store, String) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = temp_dir().join(format!(
            "axon_resources_test_{}_{}.db",
            std::process::id(),
            id
        ));
        let store = Store::open(&path).unwrap();
        let ws = "/test/workspace";

        let model = DomainModel {
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
                entities: vec![],
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
        };
        store.save_desired(ws, &model).unwrap();
        (store, ws.to_string())
    }

    #[test]
    fn test_list_resources_includes_static_and_context() {
        let (store, ws) = test_store_with_model();
        let resources = list_resources(&store, &ws);
        // 4 static + 1 compatibility semantic overlay
        assert_eq!(resources.len(), 5);
        assert!(
            resources
                .iter()
                .any(|r| r.uri == "axon://architecture/overview")
        );
        assert!(resources.iter().any(|r| r.uri == "axon://rust/ontology"));
        assert!(resources.iter().any(|r| r.uri == "axon://context/identity"));
    }

    #[test]
    fn test_read_resource_overview() {
        let (store, ws) = test_store_with_model();
        let result = read_resource(&store, &ws, "axon://architecture/overview");
        assert_eq!(result.contents.len(), 1);
        assert_eq!(result.contents[0].mime_type, "application/json");
        assert!(result.contents[0].text.contains("TestProject"));
    }

    #[test]
    fn test_read_rust_ontology_resource() {
        let (store, ws) = test_store_with_model();
        let result = read_resource(&store, &ws, "axon://rust/ontology");
        assert_eq!(result.contents.len(), 1);
        assert_eq!(result.contents[0].mime_type, "application/json");
        assert!(
            result.contents[0]
                .text
                .contains("Rust facts are the ground truth")
        );
    }

    #[test]
    fn test_read_resource_context() {
        let (store, ws) = test_store_with_model();
        let result = read_resource(&store, &ws, "axon://context/identity");
        assert!(result.contents[0].text.contains("Identity"));
    }

    #[test]
    fn test_read_resource_unknown() {
        let (store, ws) = test_store_with_model();
        let result = read_resource(&store, &ws, "axon://unknown");
        assert!(result.contents[0].text.contains("Unknown resource"));
    }

    #[test]
    fn test_read_resource_context_not_found() {
        let (store, ws) = test_store_with_model();
        let result = read_resource(&store, &ws, "axon://context/nonexistent");
        assert!(result.contents[0].text.contains("not found"));
    }
}
