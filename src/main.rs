#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::panic,
        clippy::print_stderr,
        clippy::print_stdout,
        clippy::todo,
        clippy::unwrap_used
    )
)]

use axon::domain;
use axon::server;
use axon::store;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;

const AXON_VERSION: &str = axon::VERSION;

macro_rules! status {
    ($($arg:tt)*) => {
        write_status(format_args!($($arg)*))
    };
}

#[derive(Parser)]
#[command(
    name = "axon",
    version = AXON_VERSION,
    about = "Domain Model Context Protocol Server"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP stdio server (default when no subcommand given)
    Serve {
        /// Workspace path (defaults to the nearest Cargo workspace/package ancestor)
        #[arg(short, long)]
        workspace: Option<String>,

        /// Local web graph port (only used with --standalone)
        #[arg(long, default_value_t = server::web::DEFAULT_WEB_PORT)]
        web_port: u16,

        /// Run an in-process server instead of bridging to the shared daemon.
        /// By default `serve` is daemon-only: it bridges to the daemon and never
        /// runs a standalone model (avoids split-brain and stale fallbacks).
        #[arg(long)]
        standalone: bool,
    },

    /// Start only the local web graph UI and live Rust indexer
    Web {
        /// Workspace path (defaults to the nearest Cargo workspace/package ancestor)
        #[arg(short, long)]
        workspace: Option<String>,

        /// Local web graph port
        #[arg(short, long, default_value_t = server::web::DEFAULT_WEB_PORT)]
        port: u16,
    },

    /// Export a workspace's domain model to a JSON file
    Export {
        /// Output file path
        file: String,

        /// Workspace path whose model to export
        #[arg(short, long)]
        workspace: String,

        /// State to export: actual or both (compatibility aliases: desired/current/planned)
        #[arg(short, long, default_value = "actual")]
        state: String,
    },

    /// List all crates and their model status in a workspace
    List {
        /// Workspace path (defaults to the nearest Cargo workspace/package ancestor)
        #[arg(short, long)]
        workspace: Option<String>,
    },

    /// Check live workspace semantics without prompting LLM
    Check {
        /// Workspace path
        #[arg(short, long)]
        workspace: String,
    },

    /// Scan the workspace source code and populate the implemented domain model
    Scan {
        /// Workspace path
        #[arg(short, long)]
        workspace: String,
    },

    /// Run the shared in-memory daemon that holds every workspace's model.
    ///
    /// Editors keep launching `axon serve` (stdio); those bridge to this one
    /// long-running process so all workspaces stay warm and isolated in memory.
    /// Intended to be run via `brew services start axon`.
    Daemon {
        /// Unix socket path (defaults to $AXON_SOCKET or ~/.axon/daemon.sock)
        #[arg(short, long)]
        socket: Option<String>,

        /// Port for the multi-workspace web graph
        #[arg(long, default_value_t = server::web::DEFAULT_WEB_PORT)]
        web_port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        // Default: serve from cwd, bridging to the shared daemon (daemon-only).
        None => serve_via_daemon_or_workspace_error(None).await?,

        Some(Commands::Serve {
            workspace,
            web_port,
            standalone,
        }) => {
            if standalone {
                let workspace = resolve_workspace(workspace)?;
                run_standalone(workspace, web_port).await?;
            } else {
                serve_via_daemon_or_workspace_error(workspace).await?;
            }
        }

        Some(Commands::Daemon { socket, web_port }) => {
            let socket = socket
                .map(std::path::PathBuf::from)
                .unwrap_or_else(daemon_socket_path);
            server::daemon::run(&socket, web_port).await?;
        }

        Some(Commands::Web { workspace, port }) => {
            let workspace = resolve_workspace(workspace)?;
            let registry = std::sync::Arc::new(store::CrateRegistry::open(std::path::Path::new(
                &workspace,
            ))?);
            tracing::info!(
                "Axon web graph starting for workspace: {} ({} crate(s))",
                workspace,
                registry.crates().len()
            );

            let watcher =
                server::watcher::ActualStateWatcher::new(std::sync::Arc::clone(&registry));
            watcher.spawn().await?;

            server::web::run(registry, port).await?;
        }

        Some(Commands::Export {
            file,
            workspace,
            state,
        }) => {
            let registry = store::CrateRegistry::open(std::path::Path::new(&workspace))?;
            let entry = registry.primary();
            let ws = entry.workspace_key();
            entry.store.export_to_file(&ws, &file, &state)?;
            status!(
                "Exported {} model for crate '{}' to: {}",
                state,
                entry.name,
                file
            )?;
        }

        Some(Commands::List { workspace }) => {
            let workspace = resolve_workspace(workspace)?;
            let registry = store::CrateRegistry::open(std::path::Path::new(&workspace))?;
            status!("{:<25} {:<55} STATUS", "CRATE", "PATH")?;
            status!("{}", "-".repeat(90))?;
            for entry in registry.crates() {
                let ws = entry.workspace_key();
                let has_model = entry
                    .store
                    .load_actual(&ws)
                    .ok()
                    .flatten()
                    .is_some_and(|m| !m.bounded_contexts.is_empty());
                let status = if has_model { "has model" } else { "no model" };
                status!("{:<25} {:<55} {}", entry.name, ws, status)?;
            }
            status!("\n{} crate(s) total", registry.crates().len())?;
        }

        Some(Commands::Check { workspace }) => {
            let registry = store::CrateRegistry::open(std::path::Path::new(&workspace))?;
            for entry in registry.crates() {
                let ws = entry.workspace_key();
                let live_deps = domain::analyze::scan_workspace(&entry.root)?;
                status!("Crate '{}': {} live imports.", entry.name, live_deps.len())?;
                match entry.store.check_live_dependencies(&ws, &live_deps) {
                    Ok(violations) => {
                        if violations.is_empty() {
                            status!("  No architectural layer violations found.")?;
                        } else {
                            status!("  Violations found: {:?}", violations)?;
                        }
                    }
                    Err(e) => status!("  Failed to check: {}", e)?,
                }
            }
        }

        Some(Commands::Scan { workspace }) => {
            let registry = store::CrateRegistry::open(std::path::Path::new(&workspace))?;
            for entry in registry.crates() {
                let ws = entry.workspace_key();
                let previous = entry.store.load_actual(&ws)?;
                let scan = domain::analyze::scan_actual_graph(&entry.root, previous.as_ref())?;
                let actual = &scan.model;

                let entity_count: usize = actual
                    .bounded_contexts
                    .iter()
                    .map(|bc| bc.entities.len())
                    .sum();
                let vo_count: usize = actual
                    .bounded_contexts
                    .iter()
                    .map(|bc| bc.value_objects.len())
                    .sum();
                let svc_count: usize = actual
                    .bounded_contexts
                    .iter()
                    .map(|bc| bc.services.len())
                    .sum();
                let repo_count: usize = actual
                    .bounded_contexts
                    .iter()
                    .map(|bc| bc.repositories.len())
                    .sum();
                let event_count: usize = actual
                    .bounded_contexts
                    .iter()
                    .map(|bc| bc.events.len())
                    .sum();
                let source_file_count = actual.source_files.len();
                let symbol_count = actual.symbols.len();
                let import_edge_count = actual.import_edges.len();
                let reference_edge_count = actual.reference_edges.len();
                let call_edge_count = actual.call_edges.len();
                let resolved_call_count = scan.resolved_calls.len();

                let drift_count = entry.store.save_actual_scan_and_compute_drift(&ws, &scan)?;

                status!(
                    "Crate '{}': {} contexts -> {} entities, {} VOs, {} services, {} repos, {} events; {} files, {} symbols, {} imports, {} references, {} calls, {} resolved calls; {} temporal changes",
                    entry.name,
                    actual.bounded_contexts.len(),
                    entity_count,
                    vo_count,
                    svc_count,
                    repo_count,
                    event_count,
                    source_file_count,
                    symbol_count,
                    import_edge_count,
                    reference_edge_count,
                    call_edge_count,
                    resolved_call_count,
                    drift_count
                )?;
                if let domain::analyze::SemanticResolution::Failed { error } =
                    &scan.semantic_resolution
                {
                    status!(
                        "  rust-analyzer semantic resolution failed; resolved_call edges cleared: {error}"
                    )?;
                }
            }
            status!(
                "Implemented model saved for {} crate(s).",
                registry.crates().len()
            )?;
        }
    }

    Ok(())
}

fn write_status(args: fmt::Arguments<'_>) -> Result<()> {
    let stderr = std::io::stderr();
    let mut lock = stderr.lock();
    lock.write_fmt(args)?;
    lock.write_all(b"\n")?;
    Ok(())
}

fn resolve_workspace(workspace: Option<String>) -> Result<String> {
    if let Some(workspace) = workspace {
        return Ok(workspace);
    }

    let cwd = std::env::current_dir().context("cannot determine current directory")?;
    let workspace = infer_workspace_from(&cwd).with_context(|| {
        format!(
            "no Cargo workspace or package found at or above {}; run axon from a Rust project or pass --workspace /path/to/rust/workspace",
            cwd.display()
        )
    })?;

    Ok(workspace.to_string_lossy().into_owned())
}

fn infer_workspace_from(start: &Path) -> Option<PathBuf> {
    let start = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    let mut package_root = None;

    for candidate in start.ancestors() {
        if candidate.parent().is_none() {
            break;
        }

        let cargo_toml = candidate.join("Cargo.toml");
        if !cargo_toml.is_file() {
            continue;
        }

        if cargo_toml_declares_workspace(&cargo_toml) {
            return Some(canonicalize_or_self(candidate));
        }

        if package_root.is_none() && candidate.join("src").is_dir() {
            package_root = Some(canonicalize_or_self(candidate));
        }
    }

    package_root
}

fn cargo_toml_declares_workspace(cargo_toml: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(cargo_toml) else {
        return false;
    };

    contents.lines().any(|line| {
        let line = line.split('#').next().unwrap_or_default().trim();
        line == "[workspace]" || line.starts_with("[workspace.")
    })
}

fn canonicalize_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn spawn_web_graph(registry: std::sync::Arc<store::CrateRegistry>, port: u16) {
    tokio::spawn(async move {
        if let Err(e) = server::web::run(registry, port).await {
            tracing::warn!("Web graph unavailable: {e:#}");
        }
    });
}

/// Where the daemon listens and where `serve` looks for it.
fn daemon_socket_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("AXON_SOCKET") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home)
        .join(".axon")
        .join("daemon.sock")
}

/// Daemon-only MCP serve: bridge every request to the shared daemon and never
/// run an in-process model. This guarantees a single shared brain and avoids the
/// stale standalone fallback that a momentary daemon outage could otherwise leave
/// behind. If the daemon closes the session (e.g. it restarts), we return so the
/// editor respawns us against the fresh daemon. If no daemon is reachable, we
/// retry briefly to ride out a restart window, then error with guidance rather
/// than silently degrading to standalone.
async fn serve_via_daemon(workspace: String) -> Result<()> {
    let socket = daemon_socket_path();
    // ~10s of 200ms retries to ride out a daemon restart before giving up.
    for _ in 0..50 {
        match server::bridge::try_bridge(&socket, &workspace).await {
            // The bridge ran until the editor or the daemon closed the session.
            Ok(true) => return Ok(()),
            // No daemon listening yet — wait and retry.
            Ok(false) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
            Err(e) => {
                tracing::warn!("daemon bridge error ({e:#}); retrying");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
    anyhow::bail!(
        "no axon daemon reachable at {} — start it with `brew services start axon`, \
         or run `axon serve --standalone` for an in-process server",
        socket.display()
    )
}

async fn serve_via_daemon_or_workspace_error(workspace: Option<String>) -> Result<()> {
    match resolve_workspace(workspace) {
        Ok(workspace) => serve_via_daemon(workspace).await,
        Err(error) => serve_workspace_resolution_error(error).await,
    }
}

async fn serve_workspace_resolution_error(error: anyhow::Error) -> Result<()> {
    tracing::warn!(
        "workspace resolution failed before MCP daemon bridge startup: {error:#}; serving JSON-RPC errors until the client closes stdin"
    );
    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    serve_static_workspace_error(stdin, &mut stdout, format!("{error:#}")).await
}

async fn serve_static_workspace_error<R, W>(
    mut reader: R,
    writer: &mut W,
    error_message: String,
) -> Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    while let Some(mcp_message) = server::stdio::read_message(&mut reader).await? {
        let body = mcp_message.body.trim();
        if body.is_empty() {
            continue;
        }

        let request: axon::mcp::protocol::JsonRpcRequest = match serde_json::from_str(body) {
            Ok(request) => request,
            Err(error) => {
                let response = axon::mcp::protocol::JsonRpcResponse::error(
                    None,
                    -32700,
                    format!("Parse error: {error}"),
                );
                let response = serde_json::to_string(&response)?;
                server::stdio::write_message(writer, &response, mcp_message.format).await?;
                continue;
            }
        };

        if request.id.is_some() {
            let response = axon::mcp::protocol::JsonRpcResponse::error(
                request.id,
                -32000,
                error_message.clone(),
            );
            let response = serde_json::to_string(&response)?;
            server::stdio::write_message(writer, &response, mcp_message.format).await?;
        }
    }

    Ok(())
}

/// The original single-workspace in-process server: builds a registry, starts a
/// watcher and the web graph, and serves MCP over stdio.
async fn run_standalone(workspace: String, web_port: u16) -> Result<()> {
    let registry = std::sync::Arc::new(store::CrateRegistry::open(std::path::Path::new(
        &workspace,
    ))?);
    tracing::info!(
        "Axon Server starting for workspace: {} ({} crate(s))",
        workspace,
        registry.crates().len()
    );

    let watcher = server::watcher::ActualStateWatcher::new(std::sync::Arc::clone(&registry));
    watcher.spawn().await?;

    spawn_web_graph(std::sync::Arc::clone(&registry), web_port);
    server::stdio::run(registry).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}_{}_{}", std::process::id(), unique))
    }

    #[test]
    fn workspace_resolution_finds_package_root_from_nested_dir() {
        let root = temp_root("axon_workspace_resolution_pkg");
        let nested = root.join("src").join("bin");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();

        assert_eq!(
            infer_workspace_from(&nested),
            Some(root.canonicalize().unwrap())
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_resolution_prefers_cargo_workspace_root() {
        let root = temp_root("axon_workspace_resolution_ws");
        let member = root.join("crates").join("app");
        let nested = member.join("src");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n",
        )
        .unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();

        assert_eq!(
            infer_workspace_from(&nested),
            Some(root.canonicalize().unwrap())
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_resolution_does_not_search_unrelated_directories() {
        let root = temp_root("axon_workspace_resolution_empty");
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();

        assert_eq!(infer_workspace_from(&nested), None);

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn workspace_resolution_error_is_reported_over_stdio() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut output = Vec::new();

        serve_static_workspace_error(
            tokio::io::BufReader::new(input.as_bytes()),
            &mut output,
            "no Cargo workspace".into(),
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        let (_, response) = output.split_once("\r\n\r\n").unwrap();
        let response: serde_json::Value = serde_json::from_str(response).unwrap();

        assert_eq!(response["error"]["code"], -32000);
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("no Cargo workspace")
        );
    }
}
