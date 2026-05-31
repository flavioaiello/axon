pub mod bridge;
pub mod daemon;
pub mod stdio;
pub mod watcher;
pub mod web;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Shared, in-memory map of canonical workspace root → its registry. Owned by
/// the [`daemon`] and read by the multi-workspace [`web`] graph.
pub type WorkspaceRegistries = Arc<Mutex<HashMap<String, Arc<crate::store::CrateRegistry>>>>;
