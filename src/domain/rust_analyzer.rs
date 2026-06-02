//! rust-analyzer-backed **resolved call graph** — a compiler-grade semantic layer
//! that complements the fast `syn` scanner.
//!
//! The `syn` scanner records call *sites* by name (`self.save()` → `save`) but
//! cannot say which concrete function a call resolves to — that needs name
//! resolution and type inference. rust-analyzer maintains exactly that (it powers
//! IDE "go to definition" / "call hierarchy"), and runs on a **stable** toolchain.
//!
//! This module drives a minimal, blocking LSP session against the `rust-analyzer`
//! binary: it opens each source file, asks for the document's functions, and for
//! each one uses `callHierarchy/outgoingCalls` to get the *resolved* callees with
//! their definition locations. Only callees defined inside the workspace are kept.
//!
//! The unified scan pipeline invokes this as semantic enrichment after the fast
//! `syn` pass. The `rust_resolve` tool can also run it manually. Spawning
//! rust-analyzer and letting it index a workspace can cost tens of seconds, so
//! callers must surface failures or latency explicitly.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// A call site resolved to the concrete function it targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCall {
    /// Calling function (rust-analyzer's qualified name, e.g. `CozoStore::save_actual`).
    pub caller: String,
    /// Resolved callee function name.
    pub callee: String,
    /// Workspace-relative file where the callee is defined.
    pub callee_file: String,
    /// 1-based line of the callee's definition.
    pub callee_line: usize,
}

// LSP SymbolKind values we treat as callable definitions.
const SYMBOLKIND_METHOD: i64 = 6;
const SYMBOLKIND_FUNCTION: i64 = 12;

const READY_TIMEOUT: Duration = Duration::from_secs(240);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// A callable definition discovered in `documentSymbol`, with the position to
/// drive call-hierarchy from and the name to attribute its calls to.
struct Callable {
    uri: String,
    position: Value,
    qualified: String,
}

/// Resolve the workspace's intra-crate call edges via rust-analyzer.
///
/// Two passes: (1) `documentSymbol` every file to index each callable definition
/// by `(file, line)` → `Owner::method` name; (2) `callHierarchy/outgoingCalls`
/// from each definition, mapping every resolved callee back through the index so
/// both ends carry qualified names and the callee carries its definition site.
pub fn resolve_calls(workspace_root: &Path) -> Result<Vec<ResolvedCall>> {
    use std::collections::HashMap;

    let root = std::fs::canonicalize(workspace_root).with_context(|| {
        format!(
            "workspace path does not exist: {}",
            workspace_root.display()
        )
    })?;
    let files = rust_source_files(&root);
    if files.is_empty() {
        bail!("no Rust source files found under {}", root.display());
    }

    let mut client = RaClient::spawn(&root)?;
    client.initialize(&root)?;
    client.wait_until_ready()?;

    // Pass 1: index callable definitions by (relative file, 0-based line).
    let mut defs: HashMap<(String, u64), String> = HashMap::new();
    let mut work: Vec<Callable> = Vec::new();
    for file in &files {
        let uri = path_to_uri(file);
        let rel = rel_path(&root, file);
        let symbols = client
            .request(
                "textDocument/documentSymbol",
                json!({ "textDocument": { "uri": uri } }),
                REQUEST_TIMEOUT,
            )
            .unwrap_or(Value::Null);
        if let Some(arr) = symbols.as_array() {
            collect_callables(arr, None, &rel, &uri, &mut defs, &mut work);
        }
    }

    // Pass 2: resolved outgoing calls per definition.
    let mut out: Vec<ResolvedCall> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for callable in &work {
        let items = client
            .request(
                "textDocument/prepareCallHierarchy",
                json!({ "textDocument": { "uri": callable.uri }, "position": callable.position }),
                REQUEST_TIMEOUT,
            )
            .unwrap_or(Value::Null);
        let Some(items) = items.as_array() else {
            continue;
        };
        for item in items {
            let outgoing = client
                .request(
                    "callHierarchy/outgoingCalls",
                    json!({ "item": item }),
                    REQUEST_TIMEOUT,
                )
                .unwrap_or(Value::Null);
            let Some(calls) = outgoing.as_array() else {
                continue;
            };
            for call in calls {
                let to = &call["to"];
                let Some(callee_abs) = uri_to_path(to["uri"].as_str().unwrap_or("")) else {
                    continue;
                };
                // Keep only calls resolved to definitions inside this workspace.
                let Ok(rel) = callee_abs.strip_prefix(&root) else {
                    continue;
                };
                let callee_file = rel.to_string_lossy().to_string();
                let line0 = to
                    .pointer("/selectionRange/start/line")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let callee = defs
                    .get(&(callee_file.clone(), line0))
                    .cloned()
                    .unwrap_or_else(|| to["name"].as_str().unwrap_or("").to_string());
                let key = (
                    callable.qualified.clone(),
                    callee.clone(),
                    callee_file.clone(),
                );
                if seen.insert(key) {
                    out.push(ResolvedCall {
                        caller: callable.qualified.clone(),
                        callee,
                        callee_file,
                        callee_line: (line0 + 1) as usize,
                    });
                }
            }
        }
    }
    client.shutdown();

    out.sort_by(|a, b| {
        (&a.caller, &a.callee, &a.callee_file).cmp(&(&b.caller, &b.callee, &b.callee_file))
    });
    Ok(out)
}

/// Collect `*.rs` files under `<root>/src` (falling back to the whole root),
/// honoring .gitignore so `target/` and friends are skipped.
fn rust_source_files(root: &Path) -> Vec<PathBuf> {
    let scan_root = {
        let src = root.join("src");
        if src.is_dir() {
            src
        } else {
            root.to_path_buf()
        }
    };
    let mut files = Vec::new();
    for entry in ignore::WalkBuilder::new(&scan_root).build().flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("rs") {
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    files
}

// ─── Minimal blocking LSP client ────────────────────────────────────────────

struct RaClient {
    child: Child,
    stdin: ChildStdin,
    /// Messages from rust-analyzer, decoded by a background reader thread.
    rx: Receiver<Value>,
    next_id: i64,
    /// Set once rust-analyzer reports it has finished indexing (`serverStatus`
    /// with `quiescent: true`). Recorded wherever messages are pumped so the
    /// signal isn't missed if it arrives mid-request.
    quiescent: bool,
}

impl RaClient {
    fn spawn(root: &Path) -> Result<Self> {
        let bin = rust_analyzer_binary()?;
        let mut child = Command::new(&bin)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to spawn rust-analyzer ({})", bin))?;

        let stdin = child
            .stdin
            .take()
            .context("rust-analyzer stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("rust-analyzer stdout unavailable")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(msg)) = read_message(&mut reader) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            rx,
            next_id: 1,
            quiescent: false,
        })
    }

    fn initialize(&mut self, root: &Path) -> Result<()> {
        let uri = path_to_uri(root);
        let params = json!({
            "processId": std::process::id(),
            "rootUri": uri,
            "capabilities": {
                "window": { "workDoneProgress": true },
                // rust-analyzer extension: emit serverStatus notifications so we
                // can wait for `quiescent: true` (indexing complete).
                "experimental": { "serverStatusNotification": true },
                "textDocument": {
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "callHierarchy": { "dynamicRegistration": false }
                }
            },
            "workspaceFolders": [ { "uri": uri, "name": "workspace" } ]
        });
        self.request("initialize", params, REQUEST_TIMEOUT)?;
        self.notify("initialized", json!({}))?;
        Ok(())
    }

    /// Block until rust-analyzer reports `serverStatus { quiescent: true }` —
    /// i.e. project loaded and indexed. `documentSymbol` is syntactic and would
    /// return instantly (before analysis is ready), so it is *not* a valid
    /// readiness signal; the semantic call-hierarchy needs full indexing.
    fn wait_until_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + READY_TIMEOUT;
        while !self.quiescent {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| {
                    anyhow!("rust-analyzer indexing timed out after {READY_TIMEOUT:?}")
                })?;
            match self.rx.recv_timeout(remaining) {
                Ok(msg) => self.pump(msg)?,
                Err(RecvTimeoutError::Timeout) => {
                    bail!("rust-analyzer indexing timed out after {READY_TIMEOUT:?}")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    bail!("rust-analyzer exited during indexing")
                }
            }
        }
        Ok(())
    }

    /// Handle one incoming message that is *not* the response we're waiting for:
    /// answer server→client requests, and record the quiescent signal.
    fn pump(&mut self, msg: Value) -> Result<()> {
        if let (Some(_), Some(id)) = (msg.get("method"), msg.get("id").cloned()) {
            self.write_message(json!({
                "jsonrpc": "2.0", "id": id, "result": null
            }))?;
            return Ok(());
        }
        if msg.get("method").and_then(Value::as_str) == Some("experimental/serverStatus")
            && msg
                .pointer("/params/quiescent")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            self.quiescent = true;
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        let _ = self.request("shutdown", Value::Null, Duration::from_secs(5));
        let _ = self.notify("exit", Value::Null);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    // ── JSON-RPC plumbing ──

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_message(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Send a request and pump incoming messages until the matching response
    /// arrives. Server→client requests are answered with `null` so rust-analyzer
    /// keeps moving; notifications are ignored.
    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_message(json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }))?;

        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| anyhow!("rust-analyzer request `{method}` timed out"))?;
            let msg = match self.rx.recv_timeout(remaining) {
                Ok(msg) => msg,
                Err(RecvTimeoutError::Timeout) => {
                    bail!("rust-analyzer request `{method}` timed out")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    bail!("rust-analyzer closed the connection during `{method}`")
                }
            };

            // Our response: no `method`, matching `id`.
            if msg.get("method").is_none() && msg.get("id") == Some(&json!(id)) {
                if let Some(err) = msg.get("error") {
                    bail!("rust-analyzer `{method}` error: {err}");
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            // Anything else (server request, notification, stray response): handle
            // it so the quiescent signal is recorded and rust-analyzer keeps moving.
            self.pump(msg)?;
        }
    }

    fn write_message(&mut self, value: Value) -> Result<()> {
        let body = serde_json::to_vec(&value)?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())?;
        self.stdin.write_all(&body)?;
        self.stdin.flush()?;
        Ok(())
    }
}

/// Read one LSP message (`Content-Length` framed). `Ok(None)` on EOF.
fn read_message<R: std::io::BufRead>(reader: &mut R) -> Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().ok();
        }
    }
    let len = content_length.context("LSP message missing Content-Length")?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

// ─── Helpers (pure; unit-tested) ────────────────────────────────────────────

// LSP SymbolKind values that own callable children.
const SYMBOLKIND_INTERFACE: i64 = 11; // trait (default-method bodies)
const SYMBOLKIND_OBJECT: i64 = 19; // rust-analyzer represents `impl` blocks as Object

/// Recursively index callable definitions from a hierarchical `documentSymbol`
/// response, qualifying each as `Container::method` using the enclosing
/// `impl`/trait symbol. `defs` maps `(file, 0-based line) → qualified name` (for
/// resolving callees); `work` lists each definition's position to query from.
fn collect_callables(
    symbols: &[Value],
    container: Option<&str>,
    file_rel: &str,
    uri: &str,
    defs: &mut std::collections::HashMap<(String, u64), String>,
    work: &mut Vec<Callable>,
) {
    for sym in symbols {
        let kind = sym.get("kind").and_then(Value::as_i64);
        let name = sym.get("name").and_then(Value::as_str).unwrap_or("");
        let is_callable = matches!(kind, Some(SYMBOLKIND_FUNCTION) | Some(SYMBOLKIND_METHOD));
        if let Some(start) = sym.pointer("/selectionRange/start").filter(|_| is_callable) {
            let qualified = match container {
                Some(c) => format!("{c}::{name}"),
                None => name.to_string(),
            };
            let line0 = start.get("line").and_then(Value::as_u64).unwrap_or(0);
            defs.insert((file_rel.to_string(), line0), qualified.clone());
            work.push(Callable {
                uri: uri.to_string(),
                position: start.clone(),
                qualified,
            });
        }
        // The container that child methods belong to: the impl's Self type, or
        // the trait's name. Other kinds (modules, free fns) reset to None.
        let child_container = match kind {
            Some(SYMBOLKIND_OBJECT) => container_from_impl(name),
            Some(SYMBOLKIND_INTERFACE) => Some(name.to_string()),
            _ => None,
        };
        if let Some(children) = sym.get("children").and_then(Value::as_array) {
            collect_callables(
                children,
                child_container.as_deref(),
                file_rel,
                uri,
                defs,
                work,
            );
        }
    }
}

/// Extract the implementing type from an impl symbol name: `impl Store` → `Store`,
/// `impl Visit<'ast> for StructMethodVisitor` → `StructMethodVisitor`,
/// `impl<T> Foo<T>` → `Foo`.
fn container_from_impl(impl_name: &str) -> Option<String> {
    let rest = impl_name.strip_prefix("impl")?.trim_start();
    // Trait impls: the Self type follows " for ". Inherent impls: skip the impl's
    // own generic parameters (`<T>`/`<'a>`) that come right after `impl`.
    let ty = match rest.rfind(" for ") {
        Some(i) => rest[i + 5..].trim_start(),
        None => skip_leading_generics(rest),
    };
    // Keep the leading path identifier, dropping any type generics/lifetimes.
    let end = ty.find(['<', ' ', '\'']).unwrap_or(ty.len());
    let ty = ty[..end].trim();
    (!ty.is_empty()).then(|| ty.to_string())
}

/// Skip a leading balanced `<...>` generic-parameter block, if present.
fn skip_leading_generics(s: &str) -> &str {
    let s = s.trim_start();
    if !s.starts_with('<') {
        return s;
    }
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return s[i + 1..].trim_start();
                }
            }
            _ => {}
        }
    }
    s
}

/// Workspace-relative path string for a file under `root`.
fn rel_path(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .to_string()
}

/// `file:///abs/path` → absolute path. Returns `None` for non-file URIs.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    Some(PathBuf::from(rest.replace("%20", " ")))
}

/// Absolute path → `file://` URI.
fn path_to_uri(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy().replace(' ', "%20"))
}

/// Locate the `rust-analyzer` binary: prefer rustup's resolution, fall back to PATH.
fn rust_analyzer_binary() -> Result<String> {
    if let Ok(out) = Command::new("rustup")
        .args(["which", "rust-analyzer"])
        .output()
    {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && !p.is_empty() {
            return Ok(p);
        }
    }
    // Fall back to PATH lookup.
    Ok("rust-analyzer".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_callables_qualifies_methods_by_impl() {
        // documentSymbol shape from rust-analyzer: a free fn, plus an `impl Store`
        // block (kind 19) whose method children must be qualified `Store::method`.
        let symbols = json!([
            { "name": "free_fn", "kind": 12,
              "selectionRange": { "start": { "line": 3, "character": 7 } }, "children": [] },
            { "name": "impl Store", "kind": 19,
              "selectionRange": { "start": { "line": 9, "character": 5 } },
              "children": [
                { "name": "save", "kind": 6,
                  "selectionRange": { "start": { "line": 11, "character": 11 } }, "children": [] }
              ] }
        ]);
        let mut defs = std::collections::HashMap::new();
        let mut work = Vec::new();
        collect_callables(
            symbols.as_array().unwrap(),
            None,
            "src/store/cozo.rs",
            "file:///x/src/store/cozo.rs",
            &mut defs,
            &mut work,
        );
        assert_eq!(work.len(), 2, "free_fn + Store::save");
        let names: Vec<&str> = work.iter().map(|c| c.qualified.as_str()).collect();
        assert!(names.contains(&"free_fn"));
        assert!(names.contains(&"Store::save"));
        // The index lets pass 2 resolve a callee at (file, line) → qualified name.
        assert_eq!(
            defs.get(&("src/store/cozo.rs".to_string(), 11))
                .map(String::as_str),
            Some("Store::save")
        );
    }

    #[test]
    fn container_from_impl_extracts_self_type() {
        assert_eq!(container_from_impl("impl Store").as_deref(), Some("Store"));
        assert_eq!(
            container_from_impl("impl Visit<'ast> for StructMethodVisitor").as_deref(),
            Some("StructMethodVisitor")
        );
        assert_eq!(
            container_from_impl("impl<T> Foo<T>").as_deref(),
            Some("Foo")
        );
        assert_eq!(container_from_impl("not an impl"), None);
    }

    #[test]
    fn uri_path_roundtrip() {
        let p = PathBuf::from("/Users/x/axon/src/store/cozo.rs");
        assert_eq!(uri_to_path(&path_to_uri(&p)), Some(p));
    }
}
