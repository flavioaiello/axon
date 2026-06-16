mod cozo;

pub(crate) use cozo::{ACTUAL_STATE, canonical_model_state};
pub use cozo::{
    ContextComplexity, DriftFreshness, FactSnapshotSummary, LayerViolation, ModelHealth,
    PersistedReasoningClaim, PolicyCoverage, ProjectInfo, ReasoningAssumption, ReasoningDependency,
    ReasoningDerivation, ReasoningFactRef, ReasoningJustification, ReasoningProvenance,
    ReasoningSupportEdge, Store, TruthMaintenanceReport, canonicalize_path,
    default_layer_constraints,
};

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ─── Crate Entry ───────────────────────────────────────────────────────────

/// A discovered crate within a workspace, with its own in-memory CozoDB store.
///
/// Each crate gets an independent store for the lifetime of the current process.
/// The `crate_key()` value is the canonical crate root path.
pub struct CrateEntry {
    /// Crate name (from directory name)
    pub name: String,
    /// Absolute path to the crate root (contains Cargo.toml)
    pub root: PathBuf,
    /// Per-crate CozoDB store
    pub store: Arc<Store>,
}

impl CrateEntry {
    /// The canonical key for this crate's store operations.
    pub fn crate_key(&self) -> String {
        canonicalize_path(&self.root.to_string_lossy())
    }

    /// Compatibility alias for relations that still store this key in a
    /// historically named `workspace` column.
    pub fn workspace_key(&self) -> String {
        self.crate_key()
    }
}

// ─── Crate Registry ────────────────────────────────────────────────────────

/// Registry of per-crate CozoDB stores for a workspace.
///
/// Replaces the old global `~/.axon/axon.db` with one in-memory store
/// per crate. This provides:
///
/// - **Multi-project isolation**: different VS Code projects → different crate
///   roots → independent stores.
/// - **Multi-crate support**: a workspace with multiple crates gets one store
///   per crate, each tracking its own domain model independently.
pub struct CrateRegistry {
    workspace_root: PathBuf,
    crates: Vec<CrateEntry>,
}

impl CrateRegistry {
    /// Discover crates in the workspace and open a Store for each.
    pub fn open(workspace_root: &Path) -> Result<Self> {
        let workspace_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());

        let crate_roots = discover_crate_roots(&workspace_root);

        anyhow::ensure!(
            !crate_roots.is_empty(),
            "No crates found in workspace: {}",
            workspace_root.display()
        );

        let mut crates = Vec::with_capacity(crate_roots.len());
        for (name, root) in crate_roots {
            let store = Arc::new(Store::open(&root).with_context(|| {
                format!(
                    "Failed to open in-memory store for crate '{}' rooted at {}",
                    name,
                    root.display()
                )
            })?);
            crates.push(CrateEntry { name, root, store });
        }

        Ok(Self {
            workspace_root,
            crates,
        })
    }

    /// The workspace root path.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// All discovered crates.
    pub fn crates(&self) -> &[CrateEntry] {
        &self.crates
    }

    /// The primary crate (whose root matches the workspace root, or the first).
    pub fn primary(&self) -> &CrateEntry {
        self.crates
            .iter()
            .find(|c| c.root == self.workspace_root)
            .unwrap_or(&self.crates[0])
    }

    /// Find the crate that owns a given file path (deepest matching root wins).
    pub fn for_path(&self, path: &Path) -> Option<&CrateEntry> {
        self.crates
            .iter()
            .filter(|c| path.starts_with(&c.root))
            .max_by_key(|c| c.root.components().count())
    }

    /// Find a crate by name.
    pub fn by_name(&self, name: &str) -> Option<&CrateEntry> {
        self.crates.iter().find(|c| c.name == name)
    }
}

// ─── Crate Discovery ──────────────────────────────────────────────────────

/// Discover crate roots in a workspace.
///
/// Returns `(name, root_path)` for each crate that has a `Cargo.toml` with an
/// adjacent `src/` directory. Respects `.gitignore` via the `ignore` crate.
fn discover_crate_roots(workspace_root: &Path) -> Vec<(String, PathBuf)> {
    let mut roots = Vec::new();

    // Never scan from the filesystem root. GUI-launched MCP hosts can start
    // children with cwd=/; walking from there can discover an unrelated Rust
    // crate elsewhere on the machine and silently bind the session to it.
    if workspace_root.parent().is_none() {
        return roots;
    }

    let root_cargo = workspace_root.join("Cargo.toml");
    let root_src = workspace_root.join("src");

    // Check the workspace root itself
    if root_cargo.exists() && root_src.is_dir() {
        let name = workspace_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "root".into());
        roots.push((name, workspace_root.to_path_buf()));
    }

    // Walk for workspace member crates (nested Cargo.toml files)
    for entry in ignore::WalkBuilder::new(workspace_root)
        .max_depth(Some(4))
        .build()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == "Cargo.toml") && path != root_cargo {
            let crate_dir = match path.parent() {
                Some(d) => d,
                None => continue,
            };
            let src = crate_dir.join("src");
            if src.is_dir() {
                let name = crate_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".into());
                roots.push((name, crate_dir.to_path_buf()));
            }
        }
    }

    // Fallback: if no Cargo.toml was found but src/ exists, still include it
    if roots.is_empty() && root_src.is_dir() {
        let name = workspace_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "root".into());
        roots.push((name, workspace_root.to_path_buf()));
    }

    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_registry_rejects_filesystem_root_workspace() {
        let err = match CrateRegistry::open(Path::new("/")) {
            Ok(_) => panic!("root must not be a workspace"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("No crates found in workspace: /"),
            "unexpected error: {err:#}"
        );
    }
}
