//! Long-running, in-memory, multi-workspace daemon.
//!
//! One process holds a `workspace-root → CrateRegistry` map entirely in memory,
//! so every editor session (each a thin [`super::bridge`]) shares one warm brain
//! and every workspace is isolated by its canonical root path. Workspaces are
//! registered lazily on first use, then kept fresh by a per-workspace watcher.
//!
//! Transport is a Unix domain socket speaking the same newline-delimited
//! JSON-RPC as the stdio server; each connection begins with a one-line
//! `{"workspace": "<path>"}` handshake that scopes the rest of the session.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tracing::{info, warn};

use super::WorkspaceRegistries as Registries;
use super::watcher::ActualStateWatcher;
use crate::mcp::handle_request_with_registry;
use crate::mcp::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::store::{CrateRegistry, canonicalize_path};

#[derive(Deserialize)]
struct Handshake {
    workspace: String,
}

/// Run the daemon, listening on `socket_path` until the process is stopped.
/// `web_port` hosts the multi-workspace web graph.
pub async fn run(socket_path: &Path, web_port: u16) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create socket directory {}", parent.display()))?;
    }
    // Refuse to start a second daemon over a live one: a single shared brain
    // owns the socket. (Removing the socket below would otherwise orphan it.)
    if tokio::net::UnixStream::connect(socket_path).await.is_ok() {
        anyhow::bail!(
            "an axon daemon is already listening on {}",
            socket_path.display()
        );
    }
    // A stale socket file from a previous run would block bind().
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("bind daemon socket {}", socket_path.display()))?;
    info!("Axon daemon listening on {}", socket_path.display());

    let registries: Registries = Arc::new(Mutex::new(HashMap::new()));

    // Best-effort multi-workspace web graph on the default port. The bridges
    // never open it, so the daemon owns the single port (no collisions).
    {
        let registries = Arc::clone(&registries);
        tokio::spawn(async move {
            if let Err(e) = super::web::run_multi(registries, web_port).await {
                warn!("daemon web graph unavailable: {e:#}");
            }
        });
    }

    loop {
        let (stream, _) = listener.accept().await?;
        let registries = Arc::clone(&registries);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, registries).await {
                warn!("daemon connection error: {e:#}");
            }
        });
    }
}

async fn handle_connection(stream: UnixStream, registries: Registries) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    // First line scopes the session to a workspace.
    let Some(first) = lines.next_line().await? else {
        return Ok(());
    };
    let handshake: Handshake = serde_json::from_str(first.trim())
        .context("daemon: first line must be a {\"workspace\": ...} handshake")?;
    let registry = match ensure_registry(&registries, &handshake.workspace).await {
        Ok(registry) => registry,
        Err(error) => {
            serve_workspace_registration_error(
                &mut lines,
                &mut write_half,
                &handshake.workspace,
                &error,
            )
            .await?;
            return Ok(());
        }
    };

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let request: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"));
                send_line(&mut write_half, &resp).await?;
                continue;
            }
        };
        // CozoDB work is synchronous but fast; the heavy save happens off the
        // request path in the watcher. Notifications (no id) get no response.
        let response = handle_request_with_registry(&registry, &request);
        if request.id.is_some() {
            send_line(&mut write_half, &response).await?;
        }
    }
    Ok(())
}

async fn serve_workspace_registration_error(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
    workspace: &str,
    error: &anyhow::Error,
) -> Result<()> {
    let message = format!(
        "Workspace registration failed for '{}': {error:#}. Configure axon serve with \
         `--workspace /path/to/rust/workspace` or set the MCP server cwd to a Rust workspace.",
        workspace
    );

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let request: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"));
                send_line(write_half, &resp).await?;
                continue;
            }
        };
        if request.id.is_some() {
            let resp = JsonRpcResponse::error(request.id, -32000, message.clone());
            send_line(write_half, &resp).await?;
        }
    }

    Ok(())
}

/// Return the registry for `workspace`, building, scanning, and starting a
/// watcher for it on first use. Keyed by canonical path so symlinked or relative
/// variants resolve to the same in-memory model.
async fn ensure_registry(registries: &Registries, workspace: &str) -> Result<Arc<CrateRegistry>> {
    let key = canonicalize_path(workspace);
    let mut map = registries.lock().await;
    if let Some(existing) = map.get(&key) {
        return Ok(Arc::clone(existing));
    }

    let registry = Arc::new(
        CrateRegistry::open(Path::new(&key)).with_context(|| format!("open workspace {key}"))?,
    );
    info!(
        "daemon registered workspace {} ({} crate(s))",
        key,
        registry.crates().len()
    );

    // Keep this workspace's model fresh. `spawn` completes the initial scan
    // before returning, so the first MCP request does not see an empty model.
    let watcher = ActualStateWatcher::new(Arc::clone(&registry));
    watcher
        .spawn()
        .await
        .with_context(|| format!("start watcher for workspace {key}"))?;

    map.insert(key, Arc::clone(&registry));
    Ok(registry)
}

async fn send_line(
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
    resp: &JsonRpcResponse,
) -> Result<()> {
    let mut json = serde_json::to_string(resp)?;
    json.push('\n');
    write_half.write_all(json.as_bytes()).await?;
    write_half.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn daemon_handshake_and_initialize_round_trip() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        // Minimal temp crate to register as a workspace.
        let root = std::env::temp_dir().join(format!("axon_daemon_test_{unique}"));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src").join("lib.rs"),
            "pub struct Foo { pub x: u64 }\n",
        )
        .unwrap();

        // Keep the socket path short: macOS Unix-domain socket paths are capped
        // around 100 bytes, while `std::env::temp_dir()` can be much longer.
        let socket = std::path::PathBuf::from(format!("/tmp/axon_daemon_{unique}.sock"));
        let socket_run = socket.clone();
        // web_port 0 → ephemeral, so tests never collide on a fixed port.
        tokio::spawn(async move {
            if let Err(e) = run(&socket_run, 0).await {
                eprintln!("daemon test server failed: {e:#}");
            }
        });

        // Wait until the listener accepts connections; socket-file visibility can
        // race with bind readiness on CI and slower local runs.
        let mut stream = None;
        for _ in 0..100 {
            match UnixStream::connect(&socket).await {
                Ok(connected) => {
                    stream = Some(connected);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
        let stream = stream.expect("connect daemon");
        let (read_half, mut write_half) = stream.into_split();
        let mut lines = BufReader::new(read_half).lines();

        let handshake = serde_json::json!({ "workspace": root.to_string_lossy() }).to_string();
        write_half.write_all(handshake.as_bytes()).await.unwrap();
        write_half.write_all(b"\n").await.unwrap();
        write_half
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n")
            .await
            .unwrap();
        write_half.flush().await.unwrap();

        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("daemon response timed out")
            .unwrap()
            .expect("daemon closed without responding");
        let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert!(
            resp.get("result").is_some(),
            "initialize should return a result, got: {line}"
        );

        write_half
            .write_all(
                br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"rust_status","arguments":{"detail":"full"}}}
"#,
            )
            .await
            .unwrap();
        write_half.flush().await.unwrap();

        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("daemon rust_status response timed out")
            .unwrap()
            .expect("daemon closed without responding to rust_status");
        let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 2);
        let content = resp["result"]["content"].as_array().unwrap();
        let text = content[0]["text"].as_str().unwrap();
        let status: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            status["implemented"]["bounded_contexts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(status["truth_maintenance"]["drift"]["entry_count"], 0);

        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn daemon_invalid_workspace_returns_json_rpc_error() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        let root = std::env::temp_dir().join(format!("axon_empty_workspace_{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        let socket = std::path::PathBuf::from(format!("/tmp/axon_daemon_invalid_{unique}.sock"));
        let socket_run = socket.clone();
        tokio::spawn(async move {
            if let Err(e) = run(&socket_run, 0).await {
                eprintln!("daemon invalid-workspace test server failed: {e:#}");
            }
        });

        let mut stream = None;
        for _ in 0..100 {
            match UnixStream::connect(&socket).await {
                Ok(connected) => {
                    stream = Some(connected);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
        let stream = stream.expect("connect daemon");
        let (read_half, mut write_half) = stream.into_split();
        let mut lines = BufReader::new(read_half).lines();

        let handshake = serde_json::json!({ "workspace": root.to_string_lossy() }).to_string();
        write_half.write_all(handshake.as_bytes()).await.unwrap();
        write_half.write_all(b"\n").await.unwrap();
        write_half
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n")
            .await
            .unwrap();
        write_half.flush().await.unwrap();

        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("daemon error response timed out")
            .unwrap()
            .expect("daemon closed without error response");
        let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["error"]["code"], -32000);
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap()
                .contains("--workspace"),
            "error should explain how to configure a workspace: {line}"
        );

        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn bridge_reports_when_no_daemon() {
        // Connecting to a non-existent socket must signal absence, not error.
        let missing = std::env::temp_dir().join("axon_no_such_daemon.sock");
        let _ = std::fs::remove_file(&missing);
        let bridged = super::super::bridge::try_bridge(&missing, "/tmp/whatever")
            .await
            .expect("try_bridge should not error when daemon is absent");
        assert!(
            !bridged,
            "no daemon -> bridge should return false so caller can retry or abort"
        );
    }
}
