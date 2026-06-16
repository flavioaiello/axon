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

This tap tracks `main`: `brew install flavioaiello/axon/axon` checks out the
current `main` branch and builds it locally.

```bash
brew tap flavioaiello/axon https://github.com/flavioaiello/axon
brew install flavioaiello/axon/axon
brew services start flavioaiello/axon/axon
```

To pull a newer commit after it lands on `main`:

```bash
brew update
brew reinstall flavioaiello/axon/axon
brew services restart flavioaiello/axon/axon
```

`axon --version` reports the `main` build plus the embedded commit id.

The MCP `serve` command bridges to the shared daemon. If the daemon is not
running, `serve` exits with an error instead of starting a separate in-process
model.

### From source

```bash
git clone https://github.com/flavioaiello/axon.git
cd axon
cargo install --path .
```

When developing axon itself, remember that MCP clients run the configured
binary, not the source tree. Use `cargo install --path .` to update the `axon`
on your `PATH`, or point your local MCP config at `target/debug/axon` after
running `cargo build`.

## Setup With AI Coding Agents

axon exposes the same live model through multiple adapters:

| Adapter | Use case |
|:--------|:---------|
| MCP stdio | GitHub Copilot, Claude Code, Codex, and other MCP-capable agents |
| CLI | Universal fallback for Claude Code, Codex, scripts, and terminals |
| Local web UI/API | Human inspection and extension integrations that can call HTTP |

### MCP config files by client

Different agents use different MCP configuration files. The server command is
the same, but the file location and top-level schema are not interchangeable.

| Client | VS Code / VSCodium use | Project-shared config | Personal config | Top-level key |
|:-------|:------------------------|:----------------------|:----------------|:--------------|
| GitHub Copilot Chat in VS Code | Native VS Code MCP support | `.vscode/mcp.json` | VS Code user `mcp.json` | `servers` |
| GitHub Copilot Chat in VSCodium | Only if your installed agent extension supports VS Code-style MCP | `.vscode/mcp.json` | Extension/user profile config | `servers` |
| GitHub Copilot CLI | Terminal inside either editor | none | `~/.copilot/mcp-config.json` | `mcpServers` |
| GitHub Copilot cloud agent / code review | GitHub.com, not the editor | Repository Settings > Copilot > MCP servers | none | `mcpServers` |
| Claude Code | Terminal or extension inside either editor | `.mcp.json` | `~/.claude.json` | `mcpServers` |
| Codex CLI / Codex IDE extension | Terminal or extension inside either editor | `.codex/config.toml` | `~/.codex/config.toml` | `mcp_servers` |

Use one entry per client family. VS Code does not read `.mcp.json`, Claude Code
does not read `.vscode/mcp.json`, and Codex does not read either JSON format.

The examples below assume the Homebrew install above and the shared daemon is
running. On Apple Silicon, the Homebrew binary is usually
`/opt/homebrew/bin/axon`; on Intel Macs, use `/usr/local/bin/axon`. If the agent
is launched from a shell where `axon` is already on `PATH`, `command = "axon"`
or `"command": "axon"` is also fine.

### VS Code / VSCodium / GitHub Copilot Chat

Add to `.vscode/mcp.json` in your project:

```json
{
  "servers": {
    "axon": {
      "type": "stdio",
      "command": "/opt/homebrew/bin/axon",
      "args": ["serve"]
    }
  }
}
```

Once the Homebrew service is running and this is configured, GitHub Copilot Chat
in VS Code can discover axon tools, resources, and prompts automatically.
VSCodium follows the same file shape only when the installed agent extension
implements VS Code-style MCP support.

For local axon development, build first and point VS Code at the workspace
binary so MCP requests exercise your edited source:

```json
{
  "servers": {
    "axon-dev": {
      "type": "stdio",
      "command": "${workspaceFolder}/target/debug/axon",
      "args": ["serve"]
    }
  }
}
```

The default `serve` command bridges to the shared daemon. Restarting the
Homebrew service restarts that daemon, but it does not rebuild or reinstall a
locally edited binary. When `--workspace` is omitted, axon infers the workspace
from the nearest Cargo workspace or package ancestor of the MCP child process
cwd. It does not scan downward from unrelated directories.

### GitHub Copilot CLI

GitHub Copilot CLI uses its own user-level MCP file, not `.vscode/mcp.json`.
Add axon to `~/.copilot/mcp-config.json`:

```json
{
  "mcpServers": {
    "axon": {
      "type": "stdio",
      "command": "/opt/homebrew/bin/axon",
      "args": ["serve", "--workspace", "/absolute/path/to/rust/workspace"],
      "tools": ["*"]
    }
  }
}
```

For GitHub Copilot cloud agent or Copilot code review on GitHub.com, add the
same kind of `mcpServers` entry in the repository's Copilot MCP settings instead
of committing a file. Adapt the `command` and `args` for the cloud runner: a
macOS Homebrew path such as `/opt/homebrew/bin/axon` is local-machine specific,
so install axon in the agent setup workflow or use a command path already
available in that environment.

### Claude Code

Claude Code can use a project-shared `.mcp.json` file:

```json
{
  "mcpServers": {
    "axon": {
      "type": "stdio",
      "command": "/opt/homebrew/bin/axon",
      "args": ["serve"]
    }
  }
}
```

You can also register the same server from a VS Code or VSCodium terminal:

```bash
claude mcp add --scope project --transport stdio axon -- \
  /opt/homebrew/bin/axon serve
```

For a private current-project config, omit `--scope project`; Claude Code stores
that local-scoped entry in `~/.claude.json`. For a private all-projects config,
use `--scope user`.

If a GUI-launched Claude Code extension does not inherit your shell `PATH`, keep
the full Homebrew binary path:

```bash
claude mcp add axon -- /opt/homebrew/bin/axon serve --workspace /path/to/rust/workspace
```

Claude Code extensions that expose MCP settings can use the same command and
args as the `.mcp.json` example above.

### Codex

Codex stores MCP servers in TOML. For a project-shared setup, add
`.codex/config.toml`:

```toml
[mcp_servers.axon]
command = "/opt/homebrew/bin/axon"
args = ["serve"]
```

Leave `cwd` unset here. Codex starts the MCP child in the session workspace, and
axon resolves that launch directory to the nearest Cargo workspace/package
ancestor before handing the session to the long-running daemon.

For a private user-level setup shared by Codex CLI and the Codex IDE extension,
put the same table in `~/.codex/config.toml` with an absolute workspace path:

```toml
[mcp_servers.axon]
command = "/opt/homebrew/bin/axon"
args = ["serve", "--workspace", "/absolute/path/to/rust/workspace"]
```

Or add it with the Codex CLI:

```bash
codex mcp add axon -- /opt/homebrew/bin/axon serve --workspace "$PWD"
```

Codex project config is loaded only after Codex trusts the project. The Codex
CLI and Codex IDE extension share the same `~/.codex/config.toml` and
`.codex/config.toml` layers, whether they are launched from VS Code, VSCodium,
or a standalone terminal.

## MCP tools

The canonical tool names are Rust-native and actual-state first. Older names such as `architecture`, `query_blast_radius`, `scan_model`, and `set_model` are neither advertised nor accepted as tool names; use the `rust_*` names below.

### Read tools

| Tool | Description |
|:-----|:------------|
| `rust_status` | Current actual-state Rust model: crates, modules, source files, symbols, imports, calls, semantic annotations, health, and snapshot freshness |
| `rust_graph` | Bounded graph-database views over Rust modules, source files, symbols, imports, references, calls, AST edges, neighborhoods, paths, and relation counts; repeated facts are returned as compact `schema` + `cols` + `rows` JSON with `offset`/`next_offset` pagination and machine-readable exhaustiveness metadata |
| `rust_readiness` | Product-readiness report for agents: graph confidence, semantic call-resolution coverage, rust-analyzer availability, cargo metadata visibility, version/runtime identity, and remediation actions |
| `rust_impact` | Blast-radius and shape analysis over modules, structs, symbols, dependencies, fields, methods, call graph reachability, optimization/refactor recommendations, and Rust practice findings |
| `rust_delete_safety` | Proof-backed safe-deletion check for structs/symbols with inbound call/import/AST witnesses; module is optional |
| `rust_invariants` | Evaluate actual graph invariants and configured constraints: layer violations, cycles, aggregate quality, orphans, policy violations, drift freshness |
| `rust_explain` | Evidence-backed explanation with witness paths for failing invariants or constraints |
| `rust_history` | List actual Rust graph snapshots, compare two snapshot timestamps, or compare the two most recent snapshots with `mode: "latest_diff"` |
| `rust_search` | Search Rust facts and semantic annotations by keyword |

### Write tools

| Tool | Description |
|:-----|:------------|
| `rust_scan` | Unified scan of workspace source code: refreshes the actual Rust fact graph from AST facts, code references, and compiler-resolved calls when rust-analyzer succeeds; use `rust_graph` to inspect persisted facts |
| `rust_annotations` | Create, update, or remove semantic annotations on top of Rust facts with compact `kind`/`name`/`module` plus `data` arguments; does not mutate source-extracted ground truth |
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

### Prompts

| Prompt | Description |
|:-------|:------------|
| `axon_guidelines` | Rust architecture workflow guidance enriched with live Datalog health, constraints, temporal drift, and semantic-overlay context |

## CLI

```
axon [command] [options]
```

| Command | Description |
|:--------|:------------|
| `serve` | Start the MCP stdio bridge to the shared daemon; accepts `--workspace <path>` (defaults to current directory). Use `--standalone` for an in-process server; `--web-port` only applies in standalone mode |
| `daemon` | Run the shared in-memory daemon, Unix socket bridge, live watcher, and multi-workspace web graph; accepts `--socket <path>` and `--web-port <port>` |
| `web` | Start the background Rust watcher and local web graph for one workspace; accepts `--workspace <path>` (defaults to current directory) and `--port <port>` |
| `export <file>` | Export a workspace model to JSON; requires `--workspace <path>` and accepts `--state actual` or compatibility aliases |
| `list` | Show all discovered crates and their model status; accepts `--workspace <path>` (defaults to current directory) |
| `check` | Parse live imports and report imports that do not map to the current actual semantic overlays; requires `--workspace <path>` |
| `scan` | Run the unified Rust scan, save actual facts, compute temporal drift, and attempt rust-analyzer resolved-call enrichment; requires `--workspace <path>` |

The daemon socket defaults to `$AXON_SOCKET` when set, otherwise
`~/.axon/daemon.sock`.

Examples:

```bash
axon web --workspace .
axon serve --workspace .
axon serve --workspace . --standalone
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

Reasoning tools return structured results with:

- **status** — Tool-specific state such as `true`/`false`, `reachable`, `in_sync`, `pending_changes`, `ready`, or `not_scanned`
- **proof** — Derivation rules and witness counts when the result comes from the reasoning kernel
- **evidence** — Supporting facts, paths, relation counts, and source locations when available
- **limitations** — Explicit uncertainty, such as dynamic dispatch, generated code, missing scans, or stale drift
- **truth_maintenance** — Snapshot and drift context for persisted reasoning claims

The system prefers bounded, evidence-backed answers. If the stored graph is
missing or incomplete, tools surface that through status, limitations, and next
actions instead of silently treating absence as proof.

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

### Health in `rust_status`

```json
{
  "status": "ok",
  "health": {
    "score": 85,
    "circular_deps": [],
    "module_cycles": [],
    "layer_violations": [],
    "missing_invariants": [["Catalog", "Category"]]
  },
  "graph_confidence": {
    "status": "usable_with_warnings",
    "score": 85,
    "counts": { "source_files": 14, "symbols": 210, "resolved_call_edges": 198 }
  },
  "readiness_summary": { "status": "usable_with_warnings" },
  "proof": { "rule": "architecture overview combines implemented graph reconstruction with health and temporal diff summary" }
}
```

### `rust_delete_safety`

```json
{
  "status": "false",
  "claim_kind": "safe_to_delete",
  "context": "Billing",
  "entity": "Order",
  "can_delete": false,
  "result": {
    "events_sourced": ["OrderPlaced", "OrderCancelled"],
    "repositories_managing": ["OrderRepository"],
    "import_references": [],
    "ast_references": [],
    "call_references": [
      { "caller": "process_payment", "file": "src/billing/service.rs", "line": 42 }
    ]
  },
  "proof": { "rule": "entity deletable IFF no inbound references are present in the stored implemented graph" },
  "limitations": ["Dynamic dispatch, reflection, string-based lookups, and out-of-repository consumers are not tracked."]
}
```

### `rust_history` with `mode: "latest_diff"`

```json
{
  "status": "latest_diff",
  "claim_kind": "history",
  "state": "actual",
  "ts_old": 1760000000000000,
  "ts_new": 1760000001000000,
  "summary": { "total_changes": 3, "additions": 2, "removals": 1 },
  "added": [
    { "kind": "context", "action": "add", "context": "", "name": "Notifications" },
    { "kind": "field", "action": "add", "context": "Catalog", "name": "sku", "owner_kind": "entity", "owner": "Product" }
  ],
  "removed": [
    { "kind": "entity", "action": "remove", "context": "Ordering", "name": "LegacyOrder" }
  ],
  "proof": { "rule": "latest_diff compares the two most recent stored temporal snapshots" }
}
```

## Supported languages

| Language | Parser | Coverage |
|:---------|:-------|:---------|
| Rust | `syn` crate | Full AST parsing |

Non-Rust language support was intentionally removed so axon can focus on being excellent for Rust codebases.

## License
This project is licensed under the MIT License.
