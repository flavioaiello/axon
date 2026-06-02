use crate::domain::analyze::{SemanticResolution, scan_actual_graph};
use crate::store::CrateRegistry;
use anyhow::Result;
use notify::{Event, RecursiveMode, Watcher};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, sleep};
use tracing::{error, info, warn};

pub struct ActualStateWatcher {
    registry: Arc<CrateRegistry>,
}

impl ActualStateWatcher {
    pub fn new(registry: Arc<CrateRegistry>) -> Self {
        Self { registry }
    }

    /// Spawns the watcher on a background Tokio task
    pub async fn spawn(self) -> Result<()> {
        let (tx, mut rx) = mpsc::unbounded_channel();

        // 1. Initialize the file system watcher
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                let is_source_file = event.paths.iter().any(|p| {
                    // Filter: only Rust source files, never inside target/ or node_modules/ directories.
                    p.extension().is_some_and(|ext| ext == "rs")
                        && !p
                            .components()
                            .any(|c| c.as_os_str() == "target" || c.as_os_str() == "node_modules")
                });

                if is_source_file && tx.send(()).is_err() {
                    warn!("AST watcher event dropped because the sync task has stopped");
                }
            }
        })?;

        // Watch the workspace root recursively.
        // The event filter above excludes target/ and non-Rust files.
        let workspace_root = self.registry.workspace_root();
        watcher.watch(workspace_root, RecursiveMode::Recursive)?;
        info!(
            "Started background AST watcher on {} ({} crate(s))",
            workspace_root.display(),
            self.registry.crates().len()
        );

        let registry = self.registry;

        info!("Performing initial in-memory architecture sync...");
        sync_all_crates(&registry).await?;

        tokio::spawn(async move {
            // Keep the watcher alive by moving it into the task
            let _watcher = watcher;

            loop {
                // 2. Wait for the first file-change event
                if rx.recv().await.is_none() {
                    break;
                }

                // 3. Debounce: wait to see if more events arrive in the next 2 seconds
                // This prevents running the AST parser 10 times during a "Save All"
                let debounce_duration = Duration::from_secs(2);
                let max_debounce_duration = Duration::from_secs(10);
                let batch_started_at = Instant::now();
                loop {
                    tokio::select! {
                        res = rx.recv() => {
                            if res.is_none() {
                                return; // Channel closed, exit the task completely
                            }
                            if batch_started_at.elapsed() >= max_debounce_duration {
                                info!(
                                    "Code modifications still arriving after {:?}. Syncing Actual Model...",
                                    max_debounce_duration
                                );
                                if let Err(e) = sync_all_crates(&registry).await {
                                    error!("Failed to sync actual model: {}", e);
                                }
                                break;
                            }
                            // Reset the debounce timer if another event comes in
                            continue;
                        }
                        _ = sleep(debounce_duration) => {
                            // Timer expired, time to sync!
                            info!("Code modification detected. Syncing Actual Model...");
                            if let Err(e) = sync_all_crates(&registry).await {
                                error!("Failed to sync actual model: {}", e);
                            }
                            break; // Done with this batch, go back to waiting for the next first event
                        }
                    }
                }
            }
        });

        Ok(())
    }
}

/// Sync the actual model for every crate in the registry.
///
/// Each crate is scanned independently: its own `src/` directory is parsed via
/// the AST walker and the result is saved into that crate's local store.
async fn sync_all_crates(registry: &CrateRegistry) -> Result<()> {
    for entry in registry.crates() {
        let ws = entry.workspace_key();

        // The previous implemented graph is optional enrichment, not a gate.
        let previous = entry.store.load_actual(&ws)?;

        // Full bottom-up scan scoped to this crate's root, including semantic
        // enrichment when rust-analyzer can resolve the current workspace.
        let scan = scan_actual_graph(&entry.root, previous.as_ref())?;

        // Save into this crate's local store and refresh temporal drift as one operation.
        let drift_count = entry.store.save_actual_scan_and_compute_drift(&ws, &scan)?;

        if let SemanticResolution::Failed { error } = &scan.semantic_resolution {
            warn!(
                "rust-analyzer semantic resolution failed for crate '{}'; resolved_call edges cleared: {}",
                entry.name, error
            );
        }

        info!(
            "Synced implemented model for crate '{}' ({} temporal change(s))",
            entry.name, drift_count
        );
    }
    Ok(())
}
