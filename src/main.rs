use axon::domain;
use axon::server;
use axon::store;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

const AXON_VERSION: &str = axon::VERSION;

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
        /// Workspace path (defaults to current directory)
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
        /// Workspace path (defaults to current directory)
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
        /// Workspace path (defaults to current directory)
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

    // Resolve workspace: explicit flag > cwd
    let resolve_workspace = |w: Option<String>| -> String {
        w.unwrap_or_else(|| {
            std::env::current_dir()
                .expect("cannot determine current directory")
                .to_string_lossy()
                .into_owned()
        })
    };

    match cli.command {
        // Default: serve from cwd, bridging to the shared daemon (daemon-only).
        None => serve_via_daemon(resolve_workspace(None)).await?,

        Some(Commands::Serve {
            workspace,
            web_port,
            standalone,
        }) => {
            let workspace = resolve_workspace(workspace);
            if standalone {
                run_standalone(workspace, web_port).await?;
            } else {
                serve_via_daemon(workspace).await?;
            }
        }

        Some(Commands::Daemon { socket, web_port }) => {
            let socket = socket
                .map(std::path::PathBuf::from)
                .unwrap_or_else(daemon_socket_path);
            server::daemon::run(&socket, web_port).await?;
        }

        Some(Commands::Web { workspace, port }) => {
            let workspace = resolve_workspace(workspace);
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
            eprintln!(
                "Exported {} model for crate '{}' to: {}",
                state, entry.name, file
            );
        }

        Some(Commands::List { workspace }) => {
            let workspace = resolve_workspace(workspace);
            let registry = store::CrateRegistry::open(std::path::Path::new(&workspace))?;
            eprintln!("{:<25} {:<55} STATUS", "CRATE", "PATH");
            eprintln!("{}", "-".repeat(90));
            for entry in registry.crates() {
                let ws = entry.workspace_key();
                let has_model = entry
                    .store
                    .load_actual(&ws)
                    .ok()
                    .flatten()
                    .is_some_and(|m| !m.bounded_contexts.is_empty());
                let status = if has_model { "has model" } else { "no model" };
                eprintln!("{:<25} {:<55} {}", entry.name, ws, status);
            }
            eprintln!("\n{} crate(s) total", registry.crates().len());
        }

        Some(Commands::Check { workspace }) => {
            let registry = store::CrateRegistry::open(std::path::Path::new(&workspace))?;
            for entry in registry.crates() {
                let ws = entry.workspace_key();
                let live_deps = domain::analyze::scan_workspace(&entry.root)?;
                eprintln!("Crate '{}': {} live imports.", entry.name, live_deps.len());
                match entry.store.check_live_dependencies(&ws, &live_deps) {
                    Ok(violations) => {
                        if violations.is_empty() {
                            eprintln!("  No architectural layer violations found.");
                        } else {
                            eprintln!("  Violations found: {:?}", violations);
                        }
                    }
                    Err(e) => eprintln!("  Failed to check: {}", e),
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

                eprintln!(
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
                );
                if let domain::analyze::SemanticResolution::Failed { error } =
                    &scan.semantic_resolution
                {
                    eprintln!(
                        "  rust-analyzer semantic resolution failed; resolved_call edges cleared: {error}"
                    );
                }
            }
            eprintln!(
                "Implemented model saved for {} crate(s).",
                registry.crates().len()
            );
        }
    }

    Ok(())
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
