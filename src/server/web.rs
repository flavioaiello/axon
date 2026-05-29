use crate::domain::model::DomainModel;
use crate::store::CrateRegistry;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

pub const DEFAULT_WEB_PORT: u16 = 3769;
const PORT_FALLBACK_ATTEMPTS: u16 = 20;

pub async fn run(registry: Arc<CrateRegistry>, preferred_port: u16) -> Result<()> {
    let listener = bind_with_fallback(preferred_port).await?;
    let addr = listener.local_addr()?;
    eprintln!("Axon web graph available at http://{}", addr);
    info!("Axon web graph available at http://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let registry = Arc::clone(&registry);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, registry).await {
                warn!("web request failed: {e:#}");
            }
        });
    }
}

async fn bind_with_fallback(preferred_port: u16) -> Result<TcpListener> {
    if preferred_port == 0 {
        return TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("bind web server on an ephemeral port");
    }

    let mut last_error = None;
    for offset in 0..=PORT_FALLBACK_ATTEMPTS {
        let port = preferred_port.saturating_add(offset);
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok(listener),
            Err(e) => last_error = Some(e),
        }
    }

    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow::anyhow!("no web port attempted"))
        .context(format!(
            "bind web server on 127.0.0.1:{}..{}",
            preferred_port,
            preferred_port.saturating_add(PORT_FALLBACK_ATTEMPTS)
        )))
}

async fn handle_connection(mut stream: TcpStream, registry: Arc<CrateRegistry>) -> Result<()> {
    let mut buffer = [0_u8; 8192];
    let read = stream.read(&mut buffer).await?;
    if read == 0 {
        return Ok(());
    }

    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|path| path.split('?').next())
        .unwrap_or("/");

    match path {
        "/" | "/index.html" => {
            respond(&mut stream, "200 OK", "text/html; charset=utf-8", WEB_HTML).await
        }
        "/api/graph" => {
            let body = serde_json::to_string_pretty(&build_graph_json(&registry))?;
            respond(&mut stream, "200 OK", "application/json", &body).await
        }
        "/api/health" => {
            let body = serde_json::to_string_pretty(&build_health_json(&registry))?;
            respond(&mut stream, "200 OK", "application/json", &body).await
        }
        _ => {
            respond(
                &mut stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                "not found",
            )
            .await
        }
    }
}

async fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

pub fn build_graph_json(registry: &CrateRegistry) -> Value {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut crates = Vec::new();
    let mut totals = GraphTotals::default();

    let workspace_path = registry.workspace_root().to_string_lossy().to_string();
    let workspace_label = registry
        .workspace_root()
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| workspace_path.clone());
    let workspace_id = node_id(["workspace", &workspace_path]);
    nodes.push(json!({
        "id": workspace_id,
        "label": workspace_label,
        "kind": "workspace",
        "path": workspace_path,
    }));
    totals.workspaces = 1;

    for entry in registry.crates() {
        let ws = entry.workspace_key();
        let crate_id = node_id(["crate", &entry.name]);
        nodes.push(json!({
            "id": crate_id,
            "label": entry.name,
            "kind": "crate",
            "path": entry.root.to_string_lossy(),
        }));
        edges.push(edge(&workspace_id, &crate_id, "contains"));
        totals.crates += 1;

        let model = entry.store.load_actual(&ws).ok().flatten();
        let mut crate_stats = GraphTotals::default();
        if let Some(model) = model.as_ref() {
            add_model_graph(
                &entry.name,
                &crate_id,
                model,
                &mut nodes,
                &mut edges,
                &mut crate_stats,
            );
            totals.add(&crate_stats);
        }

        crates.push(json!({
            "name": entry.name,
            "workspace": ws,
            "root": entry.root.to_string_lossy(),
            "has_model": model.is_some(),
            "stats": crate_stats,
        }));
    }

    json!({
        "view": {
            "name": "rust_architecture_overview",
            "visible_node_kinds": ["workspace", "crate", "module", "struct"],
            "visible_edge_kinds": ["contains", "declares", "imports", "calls"],
            "complete_facts_stored": true,
            "hidden_fact_kinds": ["source_file", "enum", "trait", "function", "method", "import_edge", "calls_symbol", "ast_edge"]
        },
        "workspace_root": registry.workspace_root().to_string_lossy(),
        "crates": crates,
        "nodes": nodes,
        "edges": edges,
        "stats": totals,
    })
}

fn build_health_json(registry: &CrateRegistry) -> Value {
    let crates: Vec<Value> = registry
        .crates()
        .iter()
        .map(|entry| {
            let ws = entry.workspace_key();
            let health = entry.store.model_health(&ws).ok();
            json!({
                "crate": entry.name,
                "workspace": ws,
                "health": health,
            })
        })
        .collect();

    json!({
        "workspace_root": registry.workspace_root().to_string_lossy(),
        "crates": crates,
    })
}

fn add_model_graph(
    crate_name: &str,
    crate_id: &str,
    model: &DomainModel,
    nodes: &mut Vec<Value>,
    edges: &mut Vec<Value>,
    totals: &mut GraphTotals,
) {
    let modules = collect_rust_modules(model);
    let module_ids: BTreeMap<String, String> = modules
        .keys()
        .map(|path| (path.clone(), node_id(["module", crate_name, path])))
        .collect();
    let semantic_labels = collect_semantic_labels(model);
    let mut struct_ids_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut method_owner_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut method_counts_by_owner: BTreeMap<String, usize> = BTreeMap::new();
    let mut architecture_edges: BTreeMap<(String, String, String), usize> = BTreeMap::new();

    for module in modules.values() {
        let id = module_ids
            .get(&module.path)
            .cloned()
            .unwrap_or_else(|| node_id(["module", crate_name, &module.path]));
        nodes.push(json!({
            "id": id,
            "label": module.name,
            "kind": "module",
            "path": module.path,
            "file_path": module.file_path,
            "public": module.public,
            "explicit": module.explicit,
            "file_count": module.file_count,
        }));

        if let Some(parent_path) = &module.parent_path {
            if let Some(parent_id) = module_ids.get(parent_path) {
                edges.push(edge(parent_id, &id, "contains"));
            } else {
                edges.push(edge(crate_id, &id, "contains"));
            }
            totals.submodules += 1;
        } else {
            edges.push(edge(crate_id, &id, "contains"));
        }
        totals.modules += 1;

        let pattern_labels = name_patterns(&module.name.to_ascii_lowercase());
        totals.patterns += pattern_labels.len();
    }

    totals.source_files += model.source_files.len();

    for symbol in &model.symbols {
        totals.symbols += 1;
        match symbol.kind.as_str() {
            "struct" => totals.structs += 1,
            "enum" => totals.enums += 1,
            "trait" => totals.traits += 1,
            "method" => {
                totals.methods += 1;
                if let Some((owner, _)) = symbol.name.split_once("::") {
                    *method_counts_by_owner.entry(owner.to_string()).or_default() += 1;
                }
            }
            "function" => totals.functions += 1,
            _ => {}
        }
    }

    for symbol in model
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == "struct")
    {
        let id = node_id(["struct", crate_name, &symbol.file_path, &symbol.name]);
        struct_ids_by_name
            .entry(symbol.name.clone())
            .or_default()
            .push(id.clone());

        let labels = semantic_labels
            .get(&semantic_label_key(&symbol.context, &symbol.name))
            .cloned()
            .unwrap_or_default();
        totals.semantic_labels += labels.len();

        let pattern_labels = name_patterns(&symbol.name.to_ascii_lowercase());
        totals.patterns += pattern_labels.len();

        nodes.push(json!({
            "id": id,
            "label": symbol.name,
            "kind": "struct",
            "file_path": symbol.file_path,
            "start_line": symbol.start_line,
            "end_line": symbol.end_line,
            "visibility": symbol.visibility,
            "method_count": method_counts_by_owner.get(&symbol.name).copied().unwrap_or_default(),
            "semantic_labels": labels,
            "pattern_labels": pattern_labels,
        }));

        if let Some(module_id) = rust_module_id_for_file(&module_ids, &symbol.file_path) {
            edges.push(edge(&module_id, &id, "declares"));
        } else {
            edges.push(edge(crate_id, &id, "declares"));
        }
    }

    for symbol in model
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == "method")
    {
        let Some((owner, _)) = symbol.name.split_once("::") else {
            continue;
        };
        if let Some(owner_id) = struct_ids_by_name
            .get(owner)
            .and_then(|ids| ids.first())
            .cloned()
        {
            method_owner_ids.insert(symbol.name.clone(), owner_id);
        }
    }

    for import in &model.import_edges {
        totals.imports += 1;
        let Some(from_module_id) = rust_module_id_for_file(&module_ids, &import.from_file) else {
            continue;
        };
        if let Some(to_module_id) = rust_module_id_for_import(&module_ids, &import.to_module) {
            if from_module_id != to_module_id {
                add_counted_edge(
                    &mut architecture_edges,
                    &from_module_id,
                    &to_module_id,
                    "imports",
                );
            }
        }
    }

    for call in &model.call_edges {
        totals.calls += 1;
        let Some(caller_id) =
            struct_id_for_call(&struct_ids_by_name, &method_owner_ids, &call.caller)
        else {
            continue;
        };
        let Some(callee_id) =
            struct_id_for_call(&struct_ids_by_name, &method_owner_ids, &call.callee)
        else {
            continue;
        };
        if caller_id != callee_id {
            add_counted_edge(&mut architecture_edges, &caller_id, &callee_id, "calls");
        }
    }

    flush_counted_edges(architecture_edges, edges);
}

#[derive(Clone)]
struct SemanticLabel {
    label: &'static str,
    confidence: &'static str,
    evidence: String,
}

fn collect_semantic_labels(model: &DomainModel) -> BTreeMap<String, Vec<Value>> {
    let mut labels: BTreeMap<String, Vec<Value>> = BTreeMap::new();

    for context in &model.bounded_contexts {
        for entity in &context.entities {
            push_semantic_label(
                &mut labels,
                &context.name,
                &entity.name,
                SemanticLabel {
                    label: "entity_candidate",
                    confidence: "legacy_heuristic",
                    evidence: "classified by previous DDD overlay".into(),
                },
            );
        }
        for value_object in &context.value_objects {
            push_semantic_label(
                &mut labels,
                &context.name,
                &value_object.name,
                SemanticLabel {
                    label: "value_object_candidate",
                    confidence: "legacy_heuristic",
                    evidence: "classified by previous DDD overlay".into(),
                },
            );
        }
        for service in &context.services {
            push_semantic_label(
                &mut labels,
                &context.name,
                &service.name,
                SemanticLabel {
                    label: "service_candidate",
                    confidence: "legacy_heuristic",
                    evidence: "classified by previous DDD overlay".into(),
                },
            );
        }
        for repository in &context.repositories {
            push_semantic_label(
                &mut labels,
                &context.name,
                &repository.name,
                SemanticLabel {
                    label: "repository_candidate",
                    confidence: "legacy_heuristic",
                    evidence: "classified by previous DDD overlay".into(),
                },
            );
        }
        for event in &context.events {
            push_semantic_label(
                &mut labels,
                &context.name,
                &event.name,
                SemanticLabel {
                    label: "event_candidate",
                    confidence: "legacy_heuristic",
                    evidence: "classified by previous DDD overlay".into(),
                },
            );
        }
    }

    labels
}

fn push_semantic_label(
    labels: &mut BTreeMap<String, Vec<Value>>,
    context: &str,
    symbol: &str,
    label: SemanticLabel,
) {
    labels
        .entry(semantic_label_key(context, symbol))
        .or_default()
        .push(json!({
            "label": label.label,
            "confidence": label.confidence,
            "evidence": label.evidence,
        }));
}

fn semantic_label_key(context: &str, symbol: &str) -> String {
    format!("{context}\u{1f}{symbol}")
}

fn collect_rust_modules(model: &DomainModel) -> BTreeMap<String, GraphModule> {
    let mut modules = BTreeMap::new();

    for source_file in &model.source_files {
        if let Some(path) = rust_module_path_from_file_path(&source_file.path) {
            upsert_module_path(
                &mut modules,
                &path,
                source_file.path.clone(),
                false,
                false,
                true,
            );
        }
    }

    for context in &model.bounded_contexts {
        for module in &context.modules {
            let path = if module.path.is_empty() {
                module.name.clone()
            } else {
                module.path.clone()
            };
            upsert_module_path(
                &mut modules,
                &path,
                module.file_path.clone(),
                module.public,
                true,
                false,
            );
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

fn rust_module_id_for_file(
    module_ids: &BTreeMap<String, String>,
    file_path: &str,
) -> Option<String> {
    let mut path = rust_module_path_from_file_path(file_path)?;
    loop {
        if let Some(id) = module_ids.get(&path) {
            return Some(id.clone());
        }
        let Some((parent, _)) = path.rsplit_once("::") else {
            return None;
        };
        path = parent.to_string();
    }
}

fn rust_module_id_for_import(
    module_ids: &BTreeMap<String, String>,
    import_path: &str,
) -> Option<String> {
    let mut candidate = import_path
        .strip_prefix("crate::")
        .or_else(|| import_path.strip_prefix("super::"))
        .unwrap_or(import_path)
        .trim_end_matches("::*")
        .to_string();

    loop {
        if let Some(id) = module_ids.get(&candidate) {
            return Some(id.clone());
        }
        let Some((parent, _)) = candidate.rsplit_once("::") else {
            return None;
        };
        candidate = parent.to_string();
    }
}

#[derive(Clone)]
struct GraphModule {
    name: String,
    path: String,
    parent_path: Option<String>,
    file_path: String,
    public: bool,
    explicit: bool,
    file_count: usize,
}

fn upsert_module_path(
    modules: &mut BTreeMap<String, GraphModule>,
    path: &str,
    file_path: String,
    public: bool,
    explicit: bool,
    count_file: bool,
) {
    let segments: Vec<&str> = path
        .split("::")
        .filter(|segment| !segment.is_empty())
        .collect();
    for index in 0..segments.len() {
        let current_path = segments[..=index].join("::");
        let parent_path = (index > 0).then(|| segments[..index].join("::"));
        let entry = modules.entry(current_path.clone()).or_insert(GraphModule {
            name: segments[index].to_string(),
            path: current_path.clone(),
            parent_path,
            file_path: String::new(),
            public: false,
            explicit: false,
            file_count: 0,
        });

        if index == segments.len() - 1 {
            if !file_path.is_empty() {
                entry.file_path = file_path.clone();
            }
            entry.public |= public;
            entry.explicit |= explicit;
            if count_file {
                entry.file_count += 1;
            }
        }
    }
}

fn name_patterns(lower_name: &str) -> Vec<&'static str> {
    let mut patterns = Vec::new();
    if lower_name.contains("facade") || lower_name.contains("gateway") {
        patterns.push("facade_candidate");
    }
    if lower_name.contains("actor")
        || lower_name.contains("worker")
        || lower_name.contains("supervisor")
        || lower_name.contains("watcher")
    {
        patterns.push("actor_candidate");
    }
    if lower_name.contains("adapter")
        || lower_name == "mcp"
        || lower_name == "web"
        || lower_name == "stdio"
        || lower_name.contains("protocol")
    {
        patterns.push("adapter_candidate");
    }
    if lower_name.contains("visitor") {
        patterns.push("visitor_candidate");
    }
    if lower_name.contains("factory") {
        patterns.push("factory_candidate");
    }
    if lower_name.contains("builder") {
        patterns.push("builder_candidate");
    }
    if lower_name.contains("strategy") {
        patterns.push("strategy_candidate");
    }
    patterns
}

fn node_id<const N: usize>(parts: [&str; N]) -> String {
    parts
        .iter()
        .map(|part| {
            part.chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() {
                        ch.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
                .trim_matches('-')
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(":")
}

fn edge(from: &str, to: &str, label: &str) -> Value {
    json!({"from": from, "to": to, "label": label})
}

fn counted_edge(from: &str, to: &str, label: &str, count: usize) -> Value {
    json!({"from": from, "to": to, "label": label, "count": count})
}

fn add_counted_edge(
    edges: &mut BTreeMap<(String, String, String), usize>,
    from: &str,
    to: &str,
    label: &str,
) {
    *edges
        .entry((from.to_string(), to.to_string(), label.to_string()))
        .or_default() += 1;
}

fn flush_counted_edges(
    counted_edges: BTreeMap<(String, String, String), usize>,
    edges: &mut Vec<Value>,
) {
    for ((from, to, label), count) in counted_edges {
        edges.push(counted_edge(&from, &to, &label, count));
    }
}

fn struct_id_for_call(
    struct_ids_by_name: &BTreeMap<String, Vec<String>>,
    method_owner_ids: &BTreeMap<String, String>,
    symbol_name: &str,
) -> Option<String> {
    method_owner_ids.get(symbol_name).cloned().or_else(|| {
        symbol_name
            .split_once("::")
            .and_then(|(owner, _)| struct_ids_by_name.get(owner))
            .or_else(|| struct_ids_by_name.get(symbol_name))
            .and_then(|ids| ids.first())
            .cloned()
    })
}

#[derive(Default, serde::Serialize)]
struct GraphTotals {
    workspaces: usize,
    crates: usize,
    contexts: usize,
    context_dependencies: usize,
    modules: usize,
    submodules: usize,
    source_files: usize,
    structs: usize,
    enums: usize,
    traits: usize,
    functions: usize,
    methods: usize,
    symbols: usize,
    imports: usize,
    calls: usize,
    patterns: usize,
    semantic_labels: usize,
    entities: usize,
    value_objects: usize,
    services: usize,
    repositories: usize,
    events: usize,
}

impl GraphTotals {
    fn add(&mut self, other: &GraphTotals) {
        self.workspaces += other.workspaces;
        self.crates += other.crates;
        self.contexts += other.contexts;
        self.context_dependencies += other.context_dependencies;
        self.modules += other.modules;
        self.submodules += other.submodules;
        self.source_files += other.source_files;
        self.structs += other.structs;
        self.enums += other.enums;
        self.traits += other.traits;
        self.functions += other.functions;
        self.methods += other.methods;
        self.symbols += other.symbols;
        self.imports += other.imports;
        self.calls += other.calls;
        self.patterns += other.patterns;
        self.semantic_labels += other.semantic_labels;
        self.entities += other.entities;
        self.value_objects += other.value_objects;
        self.services += other.services;
        self.repositories += other.repositories;
        self.events += other.events;
    }
}

const WEB_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Axon Graph</title>
  <style>
    :root {
      --bg: #101418;
      --panel: #171d22;
      --line: #39454f;
      --text: #e8edf1;
      --muted: #9aa8b4;
            --workspace: #f2f5f7;
            --crate: #3fbfb5;
            --module: #64c7ff;
            --struct: #f0b84f;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      min-height: 100vh;
      display: grid;
      grid-template-columns: minmax(0, 1fr) 360px;
      background: radial-gradient(circle at 20% 20%, #1d2a2f 0, transparent 30%), var(--bg);
      color: var(--text);
      font: 14px/1.45 ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    }
    main { position: relative; min-width: 0; }
    header {
      position: absolute;
      inset: 20px auto auto 24px;
      z-index: 2;
    }
    h1 { margin: 0; font-size: 22px; font-weight: 700; letter-spacing: 0; }
    #subtitle { color: var(--muted); margin-top: 4px; }
    #graph { width: 100%; height: 100vh; display: block; }
    aside {
      border-left: 1px solid #2a333b;
      background: color-mix(in srgb, var(--panel), #000 12%);
      padding: 20px;
      overflow: auto;
    }
    .stat-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin: 20px 0; }
    .stat { border: 1px solid #29333b; padding: 10px; border-radius: 6px; background: #12181d; }
    .stat strong { display: block; font-size: 20px; }
    .legend { display: grid; gap: 8px; margin-top: 18px; }
    .legend span { display: flex; align-items: center; gap: 8px; color: var(--muted); }
    .swatch { width: 12px; height: 12px; border-radius: 99px; display: inline-block; }
    pre { white-space: pre-wrap; word-break: break-word; color: var(--muted); }
    .node { cursor: grab; }
    .node.dragging { cursor: grabbing; }
    .node circle { stroke: #071014; stroke-width: 2; }
    .node text { fill: var(--text); paint-order: stroke; stroke: #101418; stroke-width: 4px; stroke-linejoin: round; font-size: 12px; }
    .edge { stroke: var(--line); stroke-width: 1.4; opacity: .72; }
    @media (max-width: 820px) {
      body { grid-template-columns: 1fr; grid-template-rows: 68vh auto; }
      #graph { height: 68vh; }
      aside { border-left: 0; border-top: 1px solid #2a333b; }
    }
  </style>
</head>
<body>
  <main>
    <header>
      <h1>Axon</h1>
        <div id="subtitle">Live Rust architecture overview</div>
    </header>
        <svg id="graph" role="img" aria-label="Rust architecture overview"></svg>
  </main>
  <aside>
    <h2>Graph</h2>
    <div id="stats" class="stat-grid"></div>
    <h2>Selected</h2>
    <pre id="details">Select a node.</pre>
    <div class="legend">
            <span><i class="swatch" style="background:var(--workspace)"></i>Workspace</span>
            <span><i class="swatch" style="background:var(--crate)"></i>Crate</span>
            <span><i class="swatch" style="background:var(--module)"></i>Module / submodule</span>
            <span><i class="swatch" style="background:var(--struct)"></i>Struct</span>
    </div>
  </aside>
  <script>
            const colors = { workspace: '#f2f5f7', crate: '#3fbfb5', module: '#64c7ff', struct: '#f0b84f' };
    const svg = document.getElementById('graph');
    const details = document.getElementById('details');
    const stats = document.getElementById('stats');

    fetch('/api/graph').then(r => r.json()).then(render).catch(err => {
      details.textContent = String(err);
    });

    function render(data) {
      document.getElementById('subtitle').textContent = `${data.nodes.length} nodes · ${data.edges.length} edges`;
      stats.innerHTML = Object.entries(data.stats).map(([key, value]) => `<div class="stat"><strong>${value}</strong>${key.replaceAll('_', ' ')}</div>`).join('');
      const width = svg.clientWidth || 1000;
      const height = svg.clientHeight || 700;
      svg.setAttribute('viewBox', `0 0 ${width} ${height}`);
      svg.innerHTML = '';

    const positioned = layout(data.nodes, data.edges, width, height);
      const byId = new Map(positioned.map(node => [node.id, node]));
    const edgeViews = [];

      for (const edge of data.edges) {
        const from = byId.get(edge.from);
        const to = byId.get(edge.to);
        if (!from || !to) continue;
        const line = document.createElementNS('http://www.w3.org/2000/svg', 'line');
        line.setAttribute('class', 'edge');
        line.setAttribute('x1', from.x);
        line.setAttribute('y1', from.y);
        line.setAttribute('x2', to.x);
        line.setAttribute('y2', to.y);
        line.setAttribute('stroke-width', Math.min(5, 1.2 + Math.log2(edge.count || 1)));
        const title = document.createElementNS('http://www.w3.org/2000/svg', 'title');
        title.textContent = `${edge.label}${edge.count ? ` (${edge.count})` : ''}`;
        line.appendChild(title);
        svg.appendChild(line);
                edgeViews.push({ edge, line });
      }

      for (const node of positioned) {
        const group = document.createElementNS('http://www.w3.org/2000/svg', 'g');
        group.setAttribute('class', 'node');
        group.setAttribute('transform', `translate(${node.x}, ${node.y})`);
                group.addEventListener('click', () => {
                    if (!node.wasDragged) details.textContent = JSON.stringify(node.raw, null, 2);
                    node.wasDragged = false;
                });
        const circle = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
                circle.setAttribute('r', radiusFor(node.kind));
        circle.setAttribute('fill', colors[node.kind] || '#9aa8b4');
                const title = document.createElementNS('http://www.w3.org/2000/svg', 'title');
                title.textContent = `${node.kind}: ${node.label}`;
        const text = document.createElementNS('http://www.w3.org/2000/svg', 'text');
        text.setAttribute('x', 16);
        text.setAttribute('y', 4);
        text.textContent = node.label;
                group.append(title, circle, text);
                enableDrag(group, node, byId, edgeViews);
        svg.appendChild(group);
      }
    }

        function radiusFor(kind) {
            if (kind === 'crate') return 22;
            if (kind === 'workspace') return 24;
            if (kind === 'module') return 14;
            return 12;
        }

        function enableDrag(group, node, byId, edgeViews) {
            let dragging = false;
            let offsetX = 0;
            let offsetY = 0;
            let moved = false;

            group.addEventListener('pointerdown', event => {
                const point = svgPoint(event);
                dragging = true;
                moved = false;
                offsetX = point.x - node.x;
                offsetY = point.y - node.y;
                group.classList.add('dragging');
                group.setPointerCapture(event.pointerId);
                event.preventDefault();
            });

            group.addEventListener('pointermove', event => {
                if (!dragging) return;
                const point = svgPoint(event);
                node.x = point.x - offsetX;
                node.y = point.y - offsetY;
                moved = true;
                updateNodeView(group, node, byId, edgeViews);
            });

            group.addEventListener('pointerup', event => finishDrag(group, node, event, moved));
            group.addEventListener('pointercancel', event => finishDrag(group, node, event, moved));
        }

        function finishDrag(group, node, event, moved) {
            group.classList.remove('dragging');
            if (group.hasPointerCapture(event.pointerId)) group.releasePointerCapture(event.pointerId);
            node.wasDragged = moved;
        }

        function updateNodeView(group, node, byId, edgeViews) {
            group.setAttribute('transform', `translate(${node.x}, ${node.y})`);
            for (const { edge, line } of edgeViews) {
                if (edge.from === node.id) {
                    line.setAttribute('x1', node.x);
                    line.setAttribute('y1', node.y);
                }
                if (edge.to === node.id) {
                    line.setAttribute('x2', node.x);
                    line.setAttribute('y2', node.y);
                }
            }
            byId.set(node.id, node);
        }

        function svgPoint(event) {
            const point = svg.createSVGPoint();
            point.x = event.clientX;
            point.y = event.clientY;
            return point.matrixTransform(svg.getScreenCTM().inverse());
        }

        function layout(nodes, edges, width, height) {
      const centerX = width / 2;
      const centerY = height / 2;
            const byId = new Map(nodes.map(node => [node.id, node]));
            const childrenByParent = new Map();
            for (const edge of edges) {
                if (!['contains', 'declares'].includes(edge.label)) continue;
                if (!childrenByParent.has(edge.from)) childrenByParent.set(edge.from, []);
                childrenByParent.get(edge.from).push(edge.to);
            }
      const positioned = [];

            const place = (node, x, y) => {
                const existing = positioned.find(item => item.id === node.id);
                if (existing) return existing;
                const placed = { ...node, raw: node, x, y };
                positioned.push(placed);
                return placed;
            };

            const workspace = nodes.find(node => node.kind === 'workspace');
            if (workspace) place(workspace, centerX, centerY);

            const placeChildren = (parent, kinds, radius, startAngle = -Math.PI / 2) => {
                const childIds = (childrenByParent.get(parent.id) || [])
                    .map(id => byId.get(id))
                    .filter(node => node && kinds.includes(node.kind));
                childIds.forEach((node, index) => {
                    const angle = startAngle + (Math.PI * 2 * index) / Math.max(1, childIds.length);
                    place(node, parent.x + Math.cos(angle) * radius, parent.y + Math.sin(angle) * radius);
                });
            };

            for (const parent of [...positioned]) placeChildren(parent, ['crate'], Math.min(width, height) * 0.18);
            for (const parent of [...positioned]) placeChildren(parent, ['module'], 130);
            for (const parent of [...positioned]) placeChildren(parent, ['module'], 84);
            for (const parent of [...positioned]) placeChildren(parent, ['struct'], 92, Math.PI / 5);

            for (const node of nodes) {
                if (positioned.some(item => item.id === node.id)) continue;
                const index = positioned.length;
                place(node, centerX + (index % 8) * 42 - 168, centerY + Math.floor(index / 8) * 42 + 220);
      }
      return positioned;
    }
  </script>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::*;
    use std::env::temp_dir;
    use std::fs;

    #[test]
    fn graph_json_contains_actual_model_nodes() {
        let root = temp_dir().join(format!("axon_web_test_{}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='web_test'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        let registry = CrateRegistry::open(&root).unwrap();
        let entry = registry.primary();
        let ws = entry.workspace_key();
        let model = DomainModel {
            name: "WebTest".into(),
            description: String::new(),
            bounded_contexts: vec![BoundedContext {
                name: "Billing".into(),
                description: String::new(),
                module_path: "src/billing".into(),
                ownership: Ownership::default(),
                aggregates: vec![],
                policies: vec![],
                read_models: vec![],
                entities: vec![Entity {
                    name: "Invoice".into(),
                    description: String::new(),
                    aggregate_root: true,
                    fields: vec![],
                    methods: vec![],
                    invariants: vec![],
                    file_path: Some("src/billing/invoice.rs".into()),
                    start_line: Some(1),
                    end_line: Some(12),
                }],
                value_objects: vec![],
                services: vec![],
                repositories: vec![],
                events: vec![],
                modules: vec![Module {
                    name: "workflow".into(),
                    path: "billing::workflow".into(),
                    public: true,
                    file_path: "src/billing/mod.rs".into(),
                    description: String::new(),
                }],
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
            source_files: vec![
                SourceFile {
                    path: "src/billing/mod.rs".into(),
                    context: "Billing".into(),
                    language: "rust".into(),
                },
                SourceFile {
                    path: "src/billing/worker.rs".into(),
                    context: "Billing".into(),
                    language: "rust".into(),
                },
            ],
            symbols: vec![SymbolDef {
                name: "Invoice".into(),
                kind: "struct".into(),
                context: "Billing".into(),
                file_path: "src/billing/invoice.rs".into(),
                start_line: 1,
                end_line: 12,
                visibility: "public".into(),
            }],
            import_edges: vec![ImportEdge {
                from_file: "src/billing/worker.rs".into(),
                to_module: "tokio::sync::mpsc".into(),
                context: "Billing".into(),
            }],
            call_edges: vec![],
        };
        entry.store.save_actual(&ws, &model).unwrap();

        let graph = build_graph_json(&registry);
        assert_eq!(graph["view"]["name"], "rust_architecture_overview");
        assert_eq!(graph["view"]["complete_facts_stored"], true);
        assert_eq!(graph["stats"]["workspaces"], 1);
        assert_eq!(graph["stats"]["crates"], 1);
        assert_eq!(graph["stats"]["contexts"], 0);
        assert!(graph["stats"]["modules"].as_u64().unwrap() >= 2);
        assert!(graph["stats"]["submodules"].as_u64().unwrap() >= 1);
        assert_eq!(graph["stats"]["source_files"], 2);
        assert_eq!(graph["stats"]["symbols"], 1);
        assert_eq!(graph["stats"]["structs"], 1);
        assert_eq!(graph["stats"]["semantic_labels"], 1);
        assert!(graph["nodes"].as_array().unwrap().iter().all(|node| {
            matches!(
                node["kind"].as_str(),
                Some("workspace" | "crate" | "module" | "struct")
            )
        }));
        assert!(graph["nodes"].as_array().unwrap().iter().any(|node| {
            node["kind"] == "struct"
                && node["label"] == "Invoice"
                && node["semantic_labels"][0]["label"] == "entity_candidate"
        }));
        assert!(
            graph["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| { node["kind"] == "module" && node["path"] == "billing::workflow" })
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn web_page_contains_graph_bootstrap() {
        assert!(WEB_HTML.contains("/api/graph"));
        assert!(WEB_HTML.contains("Live Rust architecture overview"));
        assert!(WEB_HTML.contains("enableDrag"));
        assert!(WEB_HTML.contains("Module / submodule"));
        assert!(!WEB_HTML.contains("Source file"));
    }
}
