## What is axon?

**axon** is a local Rust architecture intelligence server for AI coding agents. Instead of letting agents guess about your codebase structure, axon extracts Rust facts from source code, stores them as machine-checkable relations in an embedded Datalog engine ([CozoDB](https://www.cozodb.org/)), and answers architectural questions with **proof-carrying results**.

It focuses on one thing: the implemented architecture. The primary graph is Rust-native: workspace, crate, module, source file, symbol, imports, and calls. Higher-level language such as DDD or design patterns is represented as an optional semantic overlay with evidence, not as the ground truth. Cozo `Validity` snapshots preserve recent history for temporal diffs.

MCP is an adapter, not the engine. The live model is maintained by the local process and can be queried through MCP, CLI commands, or the built-in web graph UI.

## Key capabilities

- **Rust AST extraction** — Rust source indexing via `syn`, focused on a 1:1 Rust structure graph
- **Datalog reasoning** — Transitive dependencies, cycle detection, layer violations, blast radius, dead code analysis
- **Semantic overlays** — DDD and pattern candidates can annotate Rust nodes without replacing Rust facts
- **Impact analysis** — Compute the blast radius of any change before making it
- **Safe deletion** — Proof-backed answers to "can I delete this?" with witness references
- **Live file watching** — Background watcher keeps the implemented model in sync as you code
- **Temporal history** — Recent implemented-graph snapshots show what changed over time
- **Web graph UI** — Human-scale overview of crates, modules, submodules, structs, and weighted architecture edges; complete Rust facts remain available to MCP
- **Multi-crate workspaces** — Isolated in-memory stores with workspace member support

## Installation

### Homebrew (macOS)

```bash
brew tap flavioaiello/axon https://github.com/flavioaiello/axon
brew install axon
```

### From source

```bash
git clone https://github.com/flavioaiello/axon.git
cd axon
cargo install --path .
```

## Setup With AI Coding Agents

axon exposes the same live model through multiple adapters:

| Adapter | Use case |
|:--------|:---------|
| MCP stdio | GitHub Copilot and other MCP-capable VS Code extensions |
| CLI | Universal fallback for Claude Code, Codex, scripts, and terminals |
| Local web UI/API | Human inspection and extension integrations that can call HTTP |

### VS Code / VSCodium / GitHub Copilot

Add to `.vscode/mcp.json` in your project:

```json
{
  "servers": {
    "axon": {
      "type": "stdio",
      "command": "axon",
      "args": ["serve", "--workspace", "${workspaceFolder}"]
    }
  }
}
```

Once configured, Copilot gains access to all axon tools, resources, and prompts automatically.

VSCodium does not provide MCP by itself; MCP support comes from the installed agent extension or from an external CLI. The examples below assume the Homebrew install above, so `axon` is available on `PATH`.

### Claude Code

After installing axon with Homebrew, register it as a stdio MCP server from a VS Code or VSCodium terminal:

```bash
claude mcp add axon -- axon serve --workspace "$PWD"
```

If a GUI-launched Claude Code extension does not inherit your shell `PATH`, point it at the Homebrew binary directly:

```bash
claude mcp add axon -- /opt/homebrew/bin/axon serve --workspace /path/to/rust/workspace
```

On Intel Macs, use `/usr/local/bin/axon` instead.

Claude Code extensions that expose MCP settings can use the same command and args as the VS Code MCP JSON above.

### Codex

After installing axon with Homebrew, add it to `~/.codex/config.toml`:

```toml
[mcp_servers.axon]
command = "axon"
args = ["serve", "--workspace", "/absolute/path/to/rust/workspace"]
```

If Codex is launched from a GUI and cannot find `axon`, use the Homebrew binary path:

```toml
[mcp_servers.axon]
command = "/opt/homebrew/bin/axon"
args = ["serve", "--workspace", "/absolute/path/to/rust/workspace"]
```

On Intel Macs, use `/usr/local/bin/axon` instead. Codex extensions for VS Code or VSCodium should use the same MCP server command when they provide MCP configuration.

## MCP tools

The canonical tool names are Rust-native and actual-state first. Legacy names such as `architecture`, `query_blast_radius`, `scan_model`, and `set_model` remain callable for compatibility, but they are not advertised by `tools/list`.

### Read tools

| Tool | Description |
|:-----|:------------|
| `rust_status` | Current actual-state Rust model: crates, modules, source files, symbols, imports, calls, semantic annotations, health, and snapshot freshness |
| `rust_graph` | Bounded graph-database views over Rust modules, source files, symbols, imports, calls, AST edges, neighborhoods, paths, and relation counts; repeated facts are returned as compact `schema` + `cols` + `rows` JSON |
| `rust_health` | Structured Datalog health report: score (0–100), cycles, violations, missing invariants, orphan modules/contexts, and graph analytics when available |
| `rust_impact` | Blast-radius analysis over modules, structs, symbols, dependencies, fields, methods, and call graph reachability |
| `rust_delete_safety` | Proof-backed safe-deletion check for structs/symbols with inbound call/import/AST witnesses; module is optional |
| `rust_invariants` | Evaluate actual graph invariants and configured constraints: layer violations, cycles, aggregate quality, orphans, policy violations, drift freshness |
| `rust_path` | Return proof paths between Rust modules/components |
| `rust_explain` | Evidence-backed explanation with witness paths for failing invariants or constraints |
| `rust_diff` | Compare recent actual Rust graph snapshots — added/removed facts and changes |
| `rust_history` | List actual Rust graph snapshots or compare two snapshot timestamps |
| `rust_search` | Search Rust facts and semantic annotations by keyword |

### Write tools

| Tool | Description |
|:-----|:------------|
| `rust_scan` | AST-scan workspace source code and refresh the actual Rust fact graph; use `rust_graph` to inspect persisted facts |
| `rust_annotations` | Create, update, or remove semantic annotations on top of Rust facts; does not mutate source-extracted ground truth |
| `rust_diagnose` | Diagnose and plan from actual Rust facts; `accept`/`reset` are compatibility no-ops in actual-first mode |
| `rust_constraints` | Declare and evaluate constraints: layer assignments, allowed/forbidden dependencies |

### Resources

| URI | Content |
|:----|:--------|
| `axon://architecture/overview` | Rust ontology contract, semantic overlays, health inputs, and rules |
| `axon://rust/ontology` | Rust-native facts and overview projection guidance |
| `axon://architecture/rules` | Architectural constraints |
| `axon://architecture/conventions` | Naming, structure, and testing conventions |
| `axon://context/{name}` | Compatibility semantic-overlay details |

## CLI

```
axon [command] [options]
```

| Command | Description |
|:--------|:------------|
| `serve` | Start the MCP stdio server, background Rust watcher, and local web graph |
| `web` | Start only the background Rust watcher and local web graph |
| `export <file>` | Export domain model to JSON (`--state actual`; legacy aliases are accepted) |
| `list` | Show all crates and their model status |
| `check` | Verify workspace semantics (layer violations, cycles) |
| `scan` | AST-scan a workspace and populate the implemented model |

All commands accept `--workspace <path>` (defaults to current directory).

Examples:

```bash
axon web --workspace .
axon serve --workspace .
```

Open `http://127.0.0.1:8888` to inspect the live Rust architecture overview. If the port is occupied, axon reports a bind error instead of switching ports.

## How it works

```
┌──────────────┐       ┌──────────────────────────┐
│ Rust source  │──────►│ live Rust AST indexer    │
└──────────────┘       └────────────┬─────────────┘
               │
          ┌──────────▼──────────┐
          │ in-memory Cozo graph │
          └──────────┬──────────┘
               │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
      ┌────▼────┐        ┌────▼────┐        ┌────▼────┐
      │   MCP   │        │   CLI   │        │ Web/API │
      │ adapter │        │ adapter │        │ adapter │
      └─────────┘        └─────────┘        └─────────┘
```

1. **Ingest** — The Rust AST scanner extracts structural facts (types, imports, modules, calls) from source code
2. **Store** — Facts are normalized into in-memory CozoDB relations with `Validity` history for the running process
3. **Reason** — Datalog rules derive transitive dependencies, cycles, violations, and blast radius
4. **Expose** — MCP and CLI can query complete facts; the web UI renders a compact overview projection
5. **Watch** — Background Rust file watcher keeps the implemented model in sync (2-second debounce)

## Architecture concepts

### Rust Ground Truth

axon stores Rust structure first:

- **Workspace** — Repository root under analysis
- **Crate** — Cargo package boundary
- **Module / submodule** — Rust module tree derived from files and `mod` declarations
- **Source file** — Concrete `.rs` file location
- **Symbol** — Structs, enums, methods, and other Rust symbols discovered by the scanner
- **Edges** — `contains`, `declares`, `imports`, `calls`, and related structural facts

DDD and design-pattern vocabulary lives as semantic labels on top of these Rust nodes. For example, a struct may carry an `entity_candidate` label, but it remains a Rust `struct` in the graph.

### UI Representation

The web UI deliberately does **not** draw every fact. It shows the map a human needs for orientation:

- **Visible nodes** — crate, module, submodule, struct
- **Visible edges** — containment, declarations, aggregated imports, aggregated calls
- **Hidden but stored facts** — source files, methods, functions, enums, call sites, import records, AST edges

Mermaid is useful as an export format for a selected slice: a dependency path, a proposed refactor, or a PR/design note. It is not the best primary live UI for the whole graph because Rust call graphs become dense quickly and Mermaid's static layout has limited drill-down. The primary UI should stay interactive, filterable, and layered; Mermaid can be generated from the selected subgraph when a stable diagram is useful.

### First-class relations

Sub-structures (fields, methods, parameters, invariants) are stored as **independent CozoDB relations**, not nested JSON. This enables cross-cutting Datalog queries that would be impossible with flat document storage.

### Proof-carrying results

Every reasoning tool returns structured results with:

- **status** — `true`, `false`, or `unknown`
- **proof** — Witness paths and supporting edges
- **evidence** — Source files and line spans
- **limitations** — Explicit uncertainty (dynamic dispatch, reflection, partial ingestion)

The system never guesses. If it can't prove a claim, it returns `unknown`.

## Example tool outputs

### `rust_graph`

```json
{
  "schema": "axon.rust_graph.relations.v1",
  "format": "schema_rows",
  "view": "relations",
  "cols": ["rel", "count"],
  "rows": [["symbol", 210], ["calls_symbol", 2952]],
  "proof": { "rule": "bounded Rust graph query over persisted Cozo relations" }
}
```

### `rust_health`

```json
{
  "score": 85,
  "circular_deps": [],
  "layer_violations": [],
  "missing_invariants": [["Catalog", "Category"]],
  "orphan_contexts": ["Notifications"],
  "god_contexts": [],
  "unsourced_events": [],
  "complexity": [
    { "context": "Catalog", "entity_count": 3, "service_count": 2, "event_count": 2, "dep_count": 0 },
    { "context": "Ordering", "entity_count": 2, "service_count": 1, "event_count": 1, "dep_count": 1 }
  ]
}
```

### `rust_delete_safety`

```json
{
  "can_delete": false,
  "aggregates_referencing": [],
  "events_sourced": ["OrderPlaced", "OrderCancelled"],
  "repositories_managing": ["OrderRepository"],
  "import_references": [],
  "ast_references": [],
  "call_references": [
    { "caller": "process_payment", "file": "src/billing/service.rs", "line": 42 }
  ]
}
```

### `rust_diff`

```json
{
  "basis": "actual_history",
  "pending_changes": [
    { "kind": "context", "action": "add", "context": "", "name": "Notifications" },
    { "kind": "field", "action": "add", "context": "Catalog", "name": "sku", "owner_kind": "entity", "owner": "Product" },
    { "kind": "entity", "action": "remove", "context": "Ordering", "name": "LegacyOrder" }
  ],
  "pending_change_count": 3
}
```

## Supported languages

| Language | Parser | Coverage |
|:---------|:-------|:---------|
| Rust | `syn` crate | Full AST parsing |

Non-Rust language support was intentionally removed so axon can focus on being excellent for Rust codebases.

## License
This project is licensed under the MIT License.