use crate::domain::model::DomainModel;
use crate::mcp::protocol::*;
use crate::store::Store;

/// Returns the list of prompts the Axon server exposes.
pub fn list_prompts() -> Vec<PromptDefinition> {
    vec![PromptDefinition {
        name: "axon_guidelines".into(),
        description:
            "Rust architecture guidelines enriched by Datalog inference from the implemented \
                      Rust fact graph. Surfaces circular deps, layer violations, complexity \
                      hotspots, and model health — all computed live from CozoDB."
                .into(),
        arguments: vec![],
    }]
}

/// Resolve a prompt by name.
///
/// The `store` and `workspace_path` are required so that prompts can run
/// Datalog inference queries against the CozoDB knowledge graph, making
/// the Rust architecture graph a metalayer that actively shapes every interaction.
pub fn get_prompt(
    model: &DomainModel,
    store: &Store,
    workspace_path: &str,
    name: &str,
) -> Option<PromptGetResult> {
    match name {
        "axon_guidelines" => Some(build_guidelines_prompt(model, store, workspace_path)),
        _ => None,
    }
}

fn build_guidelines_prompt(
    model: &DomainModel,
    store: &Store,
    workspace_path: &str,
) -> PromptGetResult {
    let project_name = &model.name;
    let is_empty = model.bounded_contexts.is_empty();

    let context_line = if is_empty {
        "No semantic overlays defined yet; Rust facts remain the ground truth.".to_string()
    } else {
        let names: Vec<&str> = model
            .bounded_contexts
            .iter()
            .map(|bc| bc.name.as_str())
            .collect();
        format!("Semantic overlays: {}", names.join(", "))
    };

    let bootstrap = if is_empty {
        "\n**This project has no architecture model yet.** \
            Analyze the codebase first: identify crates, modules, structs, imports, and calls using `rust_scan` \
            (changes update the actual Rust graph). Use `rust_annotations` only to enrich semantic overlays.\n"
    } else {
        ""
    };

    let rules_section = if model.rules.is_empty() {
        String::new()
    } else {
        let rules: Vec<String> = model
            .rules
            .iter()
            .map(|r| {
                format!(
                    "- **{}** ({}): {}",
                    r.id,
                    format!("{:?}", r.severity).to_lowercase(),
                    r.description
                )
            })
            .collect();
        format!("\n### Rules\n\n{}\n", rules.join("\n"))
    };

    // ── Metalayer: Datalog-inferred health section ─────────────────────
    let health_section = build_health_section(store, workspace_path);

    let text = format!(
        r#"## Axon — {project_name}

{context_line}
{bootstrap}
### Workflow

Axon tracks the **implemented Rust architecture**: source-extracted Rust facts are ground truth; DDD and pattern terms are semantic overlays.

1. **Read the status quo** → call `rust_status` (Rust ontology summary + health score + recent temporal changes)
2. **Refresh from source** → call `rust_scan` after code changes; the watcher usually does this automatically
3. **Use the overview correctly** → the web graph intentionally shows only crate/module/submodule/struct; MCP keeps source files, symbols, imports, and calls available for precision
4. **Enrich missing semantics** → call `rust_annotations` only when static extraction needs DDD/policy labels or ownership context
5. **Analyze impact** → call `rust_impact` with module/struct/symbol aliases and analysis type (dependency_graph, call_graph_callers, call_graph_reachability, optimization_recommendations, practice_findings, etc.)
6. **Review history** → call `rust_diff` or `rust_history` to compare recent actual graph snapshots
7. **Constrain architecture** → call `rust_constraints` to declare layers and forbidden/allowed dependencies

### Continuous Improvement

Use `diagnose` as the improvement loop:

1. **Diagnose** → call `rust_diagnose` with `action: "diagnose"` — runs full analysis and returns prioritized `next_actions`
2. **Follow next_actions** → implement the highest-priority fix
3. **Re-scan** → call `rust_scan` to update the actual Rust graph from source
4. **Diagnose again** → call `rust_diagnose` with `action: "diagnose"` — health score should improve
5. **Iterate** until `status: "healthy"` (score 100)
{rules_section}
{health_section}"#
    );

    PromptGetResult {
        description: format!("Architecture guidelines for {project_name}"),
        messages: vec![PromptMessage {
            role: "user".into(),
            content: ContentBlock::Text { text },
        }],
    }
}

/// Build the Datalog-inferred health section by querying the CozoDB knowledge graph.
///
/// This is the core metalayer mechanism: every prompt is enriched with live
/// inference results from the domain model's relational decomposition. The
/// knowledge graph isn't just stored — it **thinks** and surfaces issues that
/// static JSON analysis can never detect (transitive closure, negation, recursion).
fn build_health_section(store: &Store, workspace_path: &str) -> String {
    let health = match store.model_health(workspace_path) {
        Ok(h) => h,
        Err(_) => return String::new(), // graceful degradation
    };

    let mut sections: Vec<String> = Vec::new();
    sections.push(format!(
        "### Architecture Health — Score: {}/100\n\n\
         _Computed live from the architecture knowledge graph._",
        health.score
    ));

    // ── Critical issues ────────────────────────────────────────────────
    if !health.circular_deps.is_empty() {
        let items: Vec<String> = health
            .circular_deps
            .iter()
            .map(|[a, b]| format!("- **{a}** ⇄ **{b}**"))
            .collect();
        sections.push(format!(
            "#### ⛔ Circular Dependencies (CRITICAL)\n\n\
             These bounded contexts form dependency cycles. Break them before adding code.\n\n{}",
            items.join("\n")
        ));
    }

    if !health.layer_violations.is_empty() {
        let items: Vec<String> = health
            .layer_violations
            .iter()
            .map(|v| {
                format!(
                    "- **{}.{}** → `{}` (infra)",
                    v.context, v.domain_service, v.infra_dependency
                )
            })
            .collect();
        sections.push(format!(
            "#### ⛔ Layer Violations (CRITICAL)\n\n\
             Domain services depending on infrastructure. Invert these via ports/adapters.\n\n{}",
            items.join("\n")
        ));
    }

    // ── Warnings ───────────────────────────────────────────────────────
    if !health.missing_invariants.is_empty() {
        let items: Vec<String> = health
            .missing_invariants
            .iter()
            .map(|[ctx, ent]| format!("- **{ctx}.{ent}** — aggregate root without invariants"))
            .collect();
        sections.push(format!(
            "#### ⚠️ Missing Invariants\n\n\
             Aggregate roots should enforce business rules. Define invariants for these.\n\n{}",
            items.join("\n")
        ));
    }

    if !health.god_contexts.is_empty() {
        sections.push(format!(
            "#### ⚠️ God Contexts (>10 entities+services)\n\n\
             Consider splitting: {}",
            health.god_contexts.join(", ")
        ));
    }

    if !health.unsourced_events.is_empty() {
        let items: Vec<String> = health
            .unsourced_events
            .iter()
            .map(|[ctx, evt]| format!("- **{ctx}.{evt}**"))
            .collect();
        sections.push(format!(
            "#### ⚠️ Events Without Source\n\n\
             These events have no source entity set. Link them to their originating aggregate.\n\n{}",
            items.join("\n")
        ));
    }

    // ── Info ────────────────────────────────────────────────────────────
    if !health.orphan_contexts.is_empty() {
        sections.push(format!(
            "#### ℹ️ Orphan Contexts\n\n\
             No dependencies to or from: {}. \
             This may be intentional (standalone modules) or indicate missing relationships.",
            health.orphan_contexts.join(", ")
        ));
    }

    // ── Complexity table ───────────────────────────────────────────────
    if !health.complexity.is_empty() {
        let mut table = String::from(
            "#### Complexity Distribution\n\n\
             | Context | Entities | Services | Events | Deps |\n\
             |---------|----------|----------|--------|------|\n",
        );
        for c in &health.complexity {
            table.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                c.context, c.entity_count, c.service_count, c.event_count, c.dep_count
            ));
        }
        sections.push(table);
    }

    // If the model is perfectly healthy
    if health.score == 100 {
        sections.push("✅ **No issues detected.** The domain model is structurally sound.".into());
    }

    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::*;

    fn test_model() -> DomainModel {
        DomainModel {
            name: "TestProject".into(),
            description: "Test".into(),
            bounded_contexts: vec![BoundedContext {
                name: "Identity".into(),
                description: "".into(),
                module_path: "".into(),
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
            reference_edges: vec![],
        }
    }

    fn test_store() -> Store {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("axon_prompt_test_{}_{}.db", std::process::id(), id));
        Store::open(&path).unwrap()
    }

    #[test]
    fn test_list_prompts() {
        let prompts = list_prompts();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].name, "axon_guidelines");
    }

    #[test]
    fn test_get_prompt_found() {
        let model = test_model();
        let store = test_store();
        let result = get_prompt(&model, &store, "/tmp/test", "axon_guidelines");
        assert!(result.is_some());
        let prompt = result.unwrap();
        assert!(prompt.description.contains("TestProject"));
        assert_eq!(prompt.messages.len(), 1);
    }

    #[test]
    fn test_get_prompt_not_found() {
        let model = test_model();
        let store = test_store();
        assert!(get_prompt(&model, &store, "/tmp/test", "nonexistent").is_none());
    }

    #[test]
    fn test_prompt_includes_contexts() {
        let model = test_model();
        let store = test_store();
        let prompt = get_prompt(&model, &store, "/tmp/test", "axon_guidelines").unwrap();
        let text = match &prompt.messages[0].content {
            ContentBlock::Text { text } => text,
        };
        assert!(text.contains("Identity"));
    }

    #[test]
    fn test_prompt_includes_workflow() {
        let model = test_model();
        let store = test_store();
        let prompt = get_prompt(&model, &store, "/tmp/test", "axon_guidelines").unwrap();
        let text = match &prompt.messages[0].content {
            ContentBlock::Text { text } => text,
        };
        assert!(text.contains("`rust_impact`"));
        assert!(text.contains("`rust_status`"));
    }

    #[test]
    fn test_prompt_includes_health_section() {
        let model = test_model();
        let store = test_store();
        let prompt = get_prompt(&model, &store, "/tmp/test", "axon_guidelines").unwrap();
        let text = match &prompt.messages[0].content {
            ContentBlock::Text { text } => text,
        };
        assert!(text.contains("Architecture Health"));
        assert!(text.contains("Score:"));
    }

    #[test]
    fn test_prompt_healthy_model_no_warnings() {
        // Empty store → score 100, no issues
        let model = test_model();
        let store = test_store();
        let prompt = get_prompt(&model, &store, "/tmp/test", "axon_guidelines").unwrap();
        let text = match &prompt.messages[0].content {
            ContentBlock::Text { text } => text,
        };
        assert!(text.contains("100/100"));
        assert!(text.contains("No issues detected"));
        assert!(!text.contains("CRITICAL"));
    }
}
