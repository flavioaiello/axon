<p align="center">
  <img src="axon.svg" alt="axon" width="420"/>
</p>

<p align="center">
  <strong>Live Rust structure graph for AI coding agents</strong><br/>
  <em>Repo-grounded symbolic reasoning over the implemented architecture</em>
</p>

<p align="center">
  <a href="https://github.com/flavioaiello/axon/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-7c3aed" alt="MIT License"/></a>
  <img src="https://img.shields.io/badge/rust-2024_edition-f97316" alt="Rust 2024"/>
  <img src="https://img.shields.io/badge/MCP-2025--03--26-14b8a6" alt="MCP Spec"/>
</p>

---

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

### VS Code / GitHub Copilot

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

Claude Code and Codex integrations can use the same MCP configuration when their extension supports MCP. Otherwise, use the CLI or local web API against the running `axon web` process.

## MCP tools

The canonical tool names below are exposed directly. Short aliases are also available for interactive use: `architecture`, `impact`, `safe_to_delete`, `check`, `how_connected`, `why`, `drift`, `search`, `define`, `sync`, `refactor`, and `constrain`.

### Read tools

| Tool | Description |
|:-----|:------------|
| `get_model` | Returns the implemented Rust ontology contract with health and temporal change status |
| `model_health` | Structured health report via Datalog — score (0–100), cycles, violations, complexity |
| `query_blast_radius` | Downstream impact analysis over modules, structs, symbols, dependencies, fields, methods, and calls |
| `can_delete_symbol` | Proof-backed safe-deletion check for structs/symbols with inbound reference witnesses |
| `check_architectural_invariant` | Evaluate invariants: layer violations, cycles, aggregate quality, orphans |
| `query_dependency_path` | Return proof paths between Rust modules/components |
| `explain_violation` | Evidence-backed explanation with witness paths for any violation |
| `diff_models` | Compare recent implemented graph snapshots — added/removed entities and changes |

### Write tools

| Tool | Description |
|:-----|:------------|
| `set_model` | Create, update, or remove semantic overlays on top of Rust facts |
| `scan_model` | AST-scan workspace source code and populate the implemented Rust fact graph |
| `refactor_model` | Diagnose and plan from implemented facts; `accept`/`reset` are compatibility no-ops in actual-first mode |
| `assert_model` | Declare constraints: layer assignments, allowed/forbidden dependencies |

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
axon web --workspace . --port 3769
axon serve --workspace . --web-port 3769
```

Open `http://127.0.0.1:3769` to inspect the live Rust architecture overview. If the port is occupied, axon automatically tries the next available port and prints the URL.

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

### `model_health`

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

### `can_delete_symbol`

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

### `diff_models`

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