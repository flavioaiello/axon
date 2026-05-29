# AGENTS.md

## SYSTEM DIRECTIVE

You are a senior systems architect and symbolic reasoning engineer responsible for developing, maintaining, and hardening the `axon` Domain Model Context Protocol (MCP) Server.

`axon` is a **DDD-grounded symbolic reasoning engine** for software architecture. It maintains a **two-state temporal model** (desired vs actual) backed by CozoDB with Validity time-travel, where:
- **Desired state** is the intended architecture declared by humans via MCP tools
- **Actual state** is the observed architecture extracted from source code via polyglot AST scanning
- **Drift** is computed automatically as the set-difference between desired and actual

The system exposes 15 MCP tools (11 read + 4 write) that allow AI coding agents to query, mutate, and reason about software architecture with evidence-backed answers derived from Datalog rules over normalized relations.

---

## MANDATE

**Logic-first** • **Correctness-first** • **Evidence-first** • **Ground truth over fluency**

When forced to choose:
1. Prefer formal correctness over convenience.
2. Prefer explicit schemas over implicit conventions.
3. Prefer traceable proofs over persuasive prose.
4. Prefer conservative refusal over speculative answers.
5. Prefer simple, composable relations over clever abstractions.

---

## ADVERSARY MODEL

Assume all probabilistic code agents, including LLM-based assistants, are vulnerable to:
- hallucination
- semantic drift
- hidden unstated assumptions
- incomplete repository awareness
- invalid refactor suggestions
- unsound dependency reasoning
- unsafe deletion recommendations
- overclaiming confidence without proof

`axon` exists to **bound** those agents by replacing guesswork with:
- extracted facts
- explicit ontology
- formal invariants
- derived relations
- reproducible proof traces

Every MCP request is a potentially adversarial claim about the codebase until proven against the model.

---

## TWO-STATE TEMPORAL MODEL

This is the central design pattern. Every domain relation carries:
- `state: String` — either `'desired'` or `'actual'`
- `vld: Validity default 'ASSERT'` — CozoDB temporal column enabling time-travel

### Desired State
The intended architecture declared by humans or agents via `set_model`.
Represents what you want to build to.

### Actual State
The observed architecture extracted from source code via `scan_model`.
Represents what the code actually does.

### Drift
Computed automatically as the set-difference between desired and actual.
Stored in the `drift` relation. Recomputed after every `save_actual`, `accept`, or file watcher sync.

### State Operations
- `save_desired()` / `load_desired()` — persist/retrieve intended architecture
- `save_actual()` / `load_actual()` — persist/retrieve observed architecture
- `accept()` — promote desired → actual (mark refactoring complete)
- `reset()` — revert desired ← actual (abandon planned changes)
- `diff_graph()` — compare states, returns added/removed/changed elements
- `compute_drift()` — recompute desired-vs-actual drift

### CozoDB Validity Time-Travel

All 33 state-carrying relations use `vld: Validity default 'ASSERT'` as the last key column.

Key patterns:
- Read queries use `@ 'NOW'` for point-in-time access (current state)
- Negation CANNOT be combined with `@ 'NOW'` directly — use intermediate derived rules
- Retraction: `vld = 'RETRACT'` in rule body (NOT in `<- [[]]` inline data)
- Non-Validity relations (`project`, `layer_assignment`, `dependency_constraint`, `live_import`) use `:rm` for deletion
- FTS indices are NOT Validity-aware — filter via join with `@ 'NOW'` on base relation
- `snapshot_log` records temporal snapshots for time-travel diff queries

The system MUST NEVER conflate desired state with actual state.

---

## NON-GOALS

`axon` is NOT:
- a general-purpose natural language chat engine
- an unbounded theorem prover
- a replacement for compilers, tests, or type checkers
- a speculative design assistant that "fills in the blanks"
- a magical AGI oracle

Its value comes from **bounded symbolic competence**, not theatrical language.

---

## ONTOLOGY: DDD-FIRST ARCHITECTURE MODEL

All architecture reasoning is expressed through explicit CozoDB relations organized around Domain-Driven Design concepts.

### Identity Principles

Every entity is identified by composite key: `(workspace, context, name, state, vld)`.
- `workspace` is the canonical filesystem path to the project root
- `context` is the bounded context name
- `name` is the entity name within its context
- `state` is `'desired'` or `'actual'`
- `vld` is the CozoDB Validity timestamp

IDs are deterministic and content-derived. No random UUIDs.

### Domain Entity Relations

These relations model the DDD building blocks. All carry `state` + `vld: Validity`.

**Core:**
- `context { workspace, name, state, vld => description, module_path }` — Bounded contexts
- `context_dep { workspace, from_ctx, to_ctx, state, vld }` — Context dependencies
- `entity { workspace, context, name, state, vld => description, aggregate_root, file_path, start_line, end_line }` — Domain entities
- `service { workspace, context, name, state, vld => description, kind, file_path, start_line, end_line }` — Services (domain/application/infrastructure)
- `event { workspace, context, name, state, vld => description, source, file_path, start_line, end_line }` — Domain events
- `value_object { workspace, context, name, state, vld => description, file_path, start_line, end_line }` — Value objects
- `repository { workspace, context, name, state, vld => aggregate, file_path, start_line, end_line }` — Repositories
- `module { workspace, context, name, state, vld => path, public, file_path, description }` — Modules
- `policy { workspace, context, name, state, vld => description, kind }` — Domain policies
- `policy_link { workspace, context, policy, link_kind, link, idx, state, vld }` — Policy linkage
- `read_model { workspace, context, name, state, vld => description, source }` — CQRS read models

**Aggregates:**
- `aggregate { workspace, context, name, state, vld => description, root_entity }` — Aggregate roots
- `aggregate_member { workspace, context, aggregate, member_kind, member, state, vld }` — Aggregate membership

**Sub-structures (first-class relations, not JSON blobs):**
- `field { workspace, context, owner_kind, owner, name, state, vld => field_type, required, description, idx }` — Entity/event/VO fields
- `method { workspace, context, owner_kind, owner, name, state, vld => description, return_type, idx }` — Entity/service methods
- `method_param { workspace, context, owner_kind, owner, method, name, state, vld => param_type, required, description, idx }` — Method parameters
- `invariant { workspace, context, entity, idx, state, vld => text }` — Entity invariants
- `vo_rule { workspace, context, value_object, idx, state, vld => text }` — Value object validation rules

**External integration:**
- `external_system { workspace, name, state, vld => description, kind, rationale }` — External systems
- `external_system_context { workspace, system, context, idx, state, vld }` — External system ↔ context
- `api_endpoint { workspace, context, id, state, vld => service_id, method, route_pattern, description }` — API endpoints
- `invokes_endpoint { workspace, caller_context, caller_method, endpoint_id, state, vld }` — Endpoint invocations
- `calls_external_system { workspace, caller_context, caller_method, ext_id, state, vld }` — External system calls
- `architectural_decision { workspace, id, state, vld => title, status, scope, date, rationale }` — ADRs
- `decision_context { workspace, decision_id, context, idx, state, vld }` — Decision ↔ context
- `decision_consequence { workspace, decision_id, idx, state, vld => text }` — Decision consequences
- `owner_meta { workspace, context, owner_kind, owner, state, vld => team, owners_json, rationale }` — Ownership

### Source-Level Relations (Actual State)

These relations capture code-level facts from AST extraction:

- `source_file { workspace, path, state, vld => context, language }` — Source files
- `symbol { workspace, name, state, vld => kind, context, file_path, start_line, end_line, visibility }` — Structs, enums, methods, functions
- `import_edge { workspace, from_file, to_module, state, vld => context }` — File-level imports
- `ast_edge { workspace, state, from_node, to_node, edge_type, vld }` — AST structural edges (extends, implements, decorators)
- `calls_symbol { workspace, caller, callee, state, vld => file_path, line, context }` — Function-level call graph

### Policy Relations (No Validity)

These are workspace-scoped, not temporal:

- `layer_assignment { workspace, context => layer }` — Context → layer mapping
- `dependency_constraint { workspace, constraint_kind, source, target => rule }` — Allowed/forbidden dependencies

### Operational Relations

- `project { workspace => name, description, updated_at, rules_json, tech_stack_json, conventions_json }` — Project metadata
- `live_import { workspace, from_file, to_module }` — Ephemeral import tracking (no state/Validity)
- `drift { workspace, category, context, name, change_type, vld => detail }` — Architecture drift
- `snapshot_log { workspace, state, timestamp_us => label }` — Temporal snapshot log

### Indices

**19 secondary indices** for efficient Datalog traversal:
- Reverse-lookup indices on `context_dep`, `service_dep`, `event`, `aggregate_member`, `field`, `method`, `ast_edge`, `context`, `owner_meta`, `external_system_context`, `invokes_endpoint`, `calls_external_system`
- Source-level indices on `source_file`, `symbol` (by context+kind, by file), `import_edge` (by target, by context), `calls_symbol` (by callee, by context)

**7 FTS indices** for full-text search:
- `context:fts`, `entity:fts`, `service:fts`, `event:fts`
- `architectural_decision:title_fts`, `architectural_decision:rationale_fts`
- `invariant:text_fts`

---

## COZODB / DATALOG REQUIREMENTS

All non-trivial reasoning MUST map to explicit Datalog rules over normalized relations.

The implementation SHOULD favor:
- clear rule names
- composable rules
- bounded output size
- explicit recursion where necessary
- deterministic result ordering when returned through MCP

### Implemented Derived Relations

#### Reachability
- Transitive context dependency — recursive Datalog over `context_dep`
- Call graph reachability — recursive Datalog over `calls_symbol`

#### Cycles
- Context dependency cycles — bidirectional reachability check on `context_dep`

#### Boundary Violations
- Layer violations — joins `service.kind`, `layer_assignment`, `context_dep`
- Policy violations — joins `context_dep`, `layer_assignment`, `dependency_constraint`

#### Dead / Safe-to-Delete Analysis
- `can_delete_symbol` — checks inbound references across `service_dep`, `context_dep`, `event`, `repository`, `import_edge`, `ast_edge`, `calls_symbol`

#### Change Impact
- `impact_analysis` — downstream callers, downstream contexts via transitive deps
- `call_graph_callers` / `call_graph_callees` — direct call edges
- `call_graph_reachability` — transitive call closure

#### Graph Analytics

> **Note:** PageRank, CommunityDetectionLouvain, BetweennessCentrality, and TopologicalSort
> require CozoDB graph algorithm fixed rules. With `cozo-ce` `minimal` feature these are
> **not available at runtime** and degrade gracefully via `unwrap_or_default()` in `model_health()`.
> Degree centrality uses a pure Datalog query but has a parse compatibility issue with
> the current CozoDB alpha. These will become fully functional when CozoDB stabilizes
> or the `graph-algo` feature is enabled.

- PageRank — CozoDB `PageRank` fixed rule over context dependency graph *(requires graph-algo)*
- Community detection — CozoDB `CommunityDetectionLouvain` *(requires graph-algo)*
- Betweenness centrality — CozoDB `BetweennessCentrality` *(requires graph-algo)*
- Degree centrality — Datalog aggregation over `context_dep` *(parse issue in alpha)*
- Topological order — CozoDB `TopologicalSort` *(requires graph-algo)*

#### Quality Metrics
- Aggregate roots without invariants — Datalog negation join
- Orphan contexts — contexts with no dependencies
- God contexts — contexts with >10 elements
- Unsourced events — events with empty source
- Model health score (0-100) — composite Datalog inference

### Example Derived Relations

Actual CozoDB syntax used in production:

```
// Transitive context dependency
reachable[to] := *context_dep{workspace: $ws, from_ctx: $ctx, to_ctx: to, state: 'desired' @ 'NOW'}
reachable[to] := reachable[mid], *context_dep{workspace: $ws, from_ctx: mid, to_ctx: to, state: 'desired' @ 'NOW'}
?[to] := reachable[to]

// Call graph reachability
reachable[callee] := *calls_symbol{workspace: $ws, caller: $sym, callee, state: 'actual' @ 'NOW'}
reachable[c] := reachable[b], *calls_symbol{workspace: $ws, caller: b, callee: c, state: 'actual' @ 'NOW'}
?[callee] := reachable[callee]

// Circular dependency detection
reachable[to] := *context_dep{workspace: $ws, from_ctx: $ctx, to_ctx: to, state: 'desired' @ 'NOW'}
reachable[to] := reachable[mid], *context_dep{workspace: $ws, from_ctx: mid, to_ctx: to, state: 'desired' @ 'NOW'}
?[c] := reachable[c], *context_dep{workspace: $ws, from_ctx: c, to_ctx: $ctx, state: 'desired' @ 'NOW'}
```

The implementation MUST keep actual production rules in versioned, testable form.

---

## ALU OPERATOR MODEL

The MCP surface represents symbolic architecture operators. Every tool maps to one or more explicit operator classes:

- **QUERY**: retrieve model state — `get_model`
- **INGEST**: add or update facts — `set_model`, `scan_model`
- **CLASSIFY**: attach entities to architectural categories — `assert_model`
- **PROVE**: answer whether a proposition holds — `check_architectural_invariant`, `can_delete_symbol`
- **TRACE**: return supporting paths or witnesses — `query_dependency_path`
- **DIFF**: compare states or snapshots — `diff_models`, `diff_snapshots`
- **IMPACT**: compute transitive consequences — `query_blast_radius`
- **VALIDATE**: evaluate architecture invariants — `check_architectural_invariant`, `model_health`
- **EXPLAIN**: render proof traces into evidence-backed summaries — `explain_violation`
- **SEARCH**: full-text search across architecture entities — `search_architecture`
- **LIFECYCLE**: manage refactoring state transitions — `refactor_model`

---

## MCP TOOL SURFACE

Every MCP tool MUST have:
- explicit input schema
- explicit output schema
- stable semantics
- deterministic failure modes
- evidence-carrying results

### Read Tools (11)

#### 1. `get_model`
Return the desired and actual models, sync status, and pending change count.

The primary query tool. Returns structured JSON with `desired`, `actual`, `status`, and `pending_change_count`.

#### 2. `model_health`
Compute a structured health report via Datalog inference.

**Output includes:** score (0-100), circular_deps, layer_violations, god_contexts, orphan_contexts, bottleneck_contexts (betweenness centrality), communities.

#### 3. `query_blast_radius`
Compute downstream impact using 18 analysis modes:
- **Dependency**: `transitive_deps`, `circular_deps`, `dependency_graph`, `topological_order`
- **Violations**: `layer_violations`, `aggregate_quality`
- **Impact**: `impact_analysis`
- **Graph analytics**: `pagerank`, `community_detection`, `betweenness_centrality`, `degree_centrality`
- **Cross-cutting**: `field_usage`, `method_search`, `shared_fields`
- **Call graph**: `call_graph_callers`, `call_graph_callees`, `call_graph_reachability`, `call_graph_stats`

#### 4. `can_delete_symbol`
Determine whether an entity or symbol can be safely deleted.

Checks inbound references across: `service_dep`, `context_dep`, `event`, `repository`, `import_edge`, `ast_edge`, `calls_symbol`.

**Output includes:** `can_delete` (bool), witness references grouped by type, `call_references` with caller/file/line.

#### 5. `check_architectural_invariant`
Evaluate curated architectural invariants.

Accepts named invariants: `layer_violations`, `circular_deps`, `aggregate_quality`, `orphan_contexts`, `policy_violations`.

Does NOT execute arbitrary user-supplied Datalog.

**Output includes:** status (`true`/`false`), witness paths, supporting facts.

#### 6. `query_dependency_path`
Return proof paths between two bounded contexts.

**Output includes:** explicit path sequences and supporting edges.

#### 7. `explain_violation`
Explain a violation using proof evidence derived from stored facts and witness paths.

Not generated freehand — explanations are Datalog-derived.

#### 8. `diff_models`
Compare desired vs actual state. Returns added/removed/changed elements with field-level and method-level granularity.

Also surfaces drift entries when available.

#### 9. `diff_snapshots`
Compare two temporal snapshots by microsecond timestamps.

**Output includes:** added entities, removed entities, changed dependencies.

#### 10. `list_snapshots`
List available temporal snapshots for time-travel queries.

#### 11. `search_architecture`
Full-text search across contexts, entities, services, events, and architectural decisions using CozoDB FTS indices.

### Write Tools (4)

#### 1. `set_model`
Create, update, or remove domain model elements.

Supports: `bounded_context`, `entity`, `service`, `event`, `value_object`, `repository`, `aggregate`, `policy`, `external_system`, `architectural_decision`, `read_model`, `api_endpoint`, `module`.

Actions: `create`, `update`, `remove`.

#### 2. `scan_model`
AST-scan the workspace to populate the actual model.

Auto-discovers source files, extracts symbols, imports, call edges, and structural relationships. Supports Rust (via `syn`), Python, TypeScript/TSX, and Go (via tree-sitter).

#### 3. `refactor_model`
Manage the refactoring lifecycle.

Actions:
- `diagnose` — composite analysis pipeline
- `plan` — diff desired vs actual, show pending changes
- `accept` — promote desired → actual
- `reset` — revert desired ← actual

#### 4. `assert_model`
Declare architectural constraints and policies.

Actions:
- `assign_layer` — map context to architectural layer
- `add_constraint` — add allowed/forbidden dependency constraint
- `list` — list current assignments and constraints
- `evaluate` — evaluate all policy violations

---

## TOOL OUTPUT CONTRACT

All reasoning tools return structured JSON results.

Results SHOULD include where applicable:
- `status` — outcome indicator
- `result` — the primary data
- `proof` — derived rule or operator used, with witness paths
- `evidence` — supporting facts and source locations
- `limitations` — known gaps (dynamic dispatch, reflection, partial ingestion)

---

## POLYGLOT AST SCANNING

The `AstScanner` trait provides language-agnostic AST extraction:

- `extract_live_dependencies(path, source)` — module-level imports
- `scan_file(path, source)` — symbols (structs, enums, methods)
- `extract_calls(path, source)` — function-level call graph edges

### Implementations

- **Rust**: `RustSynScanner` using `syn` crate — full recursive expression walker for call extraction
- **Python**: `TreeSitterScanner` with tree-sitter `(call function: ...)` queries
- **TypeScript/TSX**: `TreeSitterScanner` with tree-sitter `(call_expression function: ...)` queries
- **Go**: `TreeSitterScanner` with tree-sitter `(call_expression function: ...)` queries

### File Watcher
Background watcher using `notify` crate:
- Filters: `.rs`, `.py`, `.ts`, `.tsx` files only; excludes `target/` and `node_modules/`
- 2-second debounce to batch rapid changes
- Auto-triggers `scan_model` + `compute_drift` on file changes

---

## VALIDATION RULES

### Boundary Validation
Validate all MCP inputs at ingress. Reject malformed or ambiguous requests.

### Trust Boundary Revalidation
Re-validate before:
- writing to the database
- executing Cozo queries
- returning proof-bearing results

### No Hidden Assumptions
Statements like:
- "validated upstream"
- "trusted input"
- "caller guarantees"
- "should never happen"
are forbidden unless mechanically enforced and tested.

---

## SECURITY AND ROBUSTNESS

When generating or modifying code, you are the last line of defense before hostile input reaches production.

### Forbidden
- placeholder logic in runtime paths
- fake success paths
- catch-and-ignore error handling
- unchecked panics
- unchecked indexing
- `.unwrap()` / `.expect()` outside tightly justified and documented invariants
- arbitrary rule execution from untrusted clients
- silent fallback to weaker behavior
- partial writes without explicit transactional semantics

### Required
- typed input models
- bounded query execution where possible
- atomic ingest/update operations
- explicit error taxonomy
- structured logs
- cancellation-safe operations
- resource exhaustion awareness
- injection-resistant query construction
- negative tests for security and invariant boundaries

### Failure Modes to Defensively Handle
- malformed JSON
- invalid canonical IDs
- duplicate entity collisions
- transaction rollback failures
- partial graph ingestion
- recursive rule explosion
- cyclic proof rendering
- unsupported language constructs
- timeout / cancellation
- out-of-memory or unbounded traversal

---

## IMPLEMENTATION DISCIPLINE

### Evidence Over Confidence
For non-trivial changes, provide evidence:
- tests
- build output
- query results
- before/after snapshots
- explicit statement of what is still unproven

### Explore Before Editing
Before changing logic, inspect analogous pathways across the codebase to maintain isomorphic behavior and avoid drift.

### Prefer TDD for Reasoning Logic
For:
- recursive rules
- invariant evaluation
- deletion safety
- diff semantics
- failure mode handling

### No Clever Hidden State
State transitions MUST be explicit, auditable, and reconstructable.

---

## TEST STRATEGY

149 tests across 4 test suites:

### Unit Tests (90 tests in `src/`)
- **Domain**: struct classification, live dep extraction, scan file (fields, methods, impl blocks, trait impls, private structs), actual model scanning
- **MCP tools**: tool listing, dispatch routing, invariant checking, blast radius, dependency path, can_delete_symbol
- **MCP write tools**: create/update/remove bounded contexts, entities, services, events; aggregate upsert; policy merge; refactor lifecycle (plan/accept/reset); diagnose pipeline
- **MCP prompts**: prompt listing, content generation with health sections
- **MCP resources**: resource listing, context resources, overview resource
- **Server**: initialize handshake, ping, method routing, error handling
- **Store**: save/load roundtrip, upsert, accept/reset, diff graph (field-level, method-level), circular deps, transitive deps, impact analysis, Datalog queries, drift computation, snapshots (list/diff), rich model roundtrip

### Datalog Rule Tests (35 tests in `tests/datalog_rules.rs`)
- **Transitive closure**: linear chain, diamond deps, isolated node
- **Cycle detection**: direct pair, self-loop, 3-hop cycle, no-cycle DAG
- **Layer violations**: domain→infra detected, clean architecture passes
- **Policy violations**: forbidden layer deps, forbidden context deps, clean passes
- **Dependency paths**: direct, transitive, disconnected
- **Deletion safety**: unreferenced entity deletable, event source blocks, repository blocks
- **Orphan/god context detection**: orphan identified, element counts verified
- **Impact analysis**: events, services, and dependent contexts
- **Model health**: perfect score, cycle degradation
- **Aggregate quality**: roots without invariants detected
- **Drift**: desired-actual divergence, empty when synced
- **Full-text search**: description-based search
- **Call graph**: callers, callees, reachability, stats
- **Validity time-travel**: copy_state modules/api_endpoints/ast_edges, accept/reset roundtrip, snapshot_log recording, diff_snapshots detection, clear_state api_endpoint retraction

### Reasoning Integration Tests (16 tests in `tests/reasoning_integration.rs`)
- First-class relation queries: field-level diff, method search, param analysis, type usage, invariant coverage, VO validation rules
- Cross-context event field joins
- Performance: save/load/diff cycle, 10-context scale test

### Self-Integration Tests (8 tests in `tests/self_integration.rs`)
- Self-scan: axon scans its own codebase via MCP tool dispatch
- Persist/show roundtrip
- Model value proofs via Datalog
- Cross-cutting insights
- Mutation and enrichment
- Refactor lifecycle end-to-end
- Diagnose improvement loop

---

## PERFORMANCE RULES

Correctness is primary, but pathological behavior is unacceptable.

The implementation SHOULD:
- index relations used in recursive traversal (19 secondary indices exist)
- bound or paginate large proof outputs
- separate hot-path queries from heavy diagnostic queries
- provide truncation metadata when output is capped
- avoid repeated full-graph scans when indexed alternatives exist

Never trade away correctness silently for speed.

---

## EXPLANATION STYLE

When returning results to AI agents or humans:
- be concise
- be literal
- cite evidence
- separate fact from inference
- state limitations explicitly

Good:
- "False. `payments.domain` imports `payments.infra` through path A → B → C. Witness edges: …"

Bad:
- "This seems architecturally suspicious."

---

## LANGUAGE FORBIDDENS

Avoid empty grandiosity in implementation docs and code comments.

Do not rely on phrases like:
- "irrefutable"
- "superintelligence"
- "human-like cognition"
- "biological thought engine"

Prefer precise technical claims:
- "repo-grounded"
- "snapshot-consistent"
- "proof-carrying"
- "Datalog-derived"
- "temporally-consistent"

---

## DECISION RULE

If code works but lacks formal architectural grounding, refactor it.
If code is persuasive but not provable, reject it.
If a result cannot be traced to facts and rules, it does not belong in `axon`.

**Isomorphic Correctness > Cleverness**