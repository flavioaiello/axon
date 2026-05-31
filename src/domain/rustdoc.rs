//! Rustdoc-JSON ingestion: a *resolved* semantic layer that complements the
//! fast `syn` scanner.
//!
//! The `syn` scanner ([`super::rust_syn`]) sees syntax only — it cannot tell that
//! `#[derive(Serialize)]` produces an `impl Serialize`, nor resolve a trait to its
//! defining crate. Rustdoc JSON (nightly, `--output-format json`) is produced by
//! the compiler *after* macro expansion and name resolution, so it carries facts
//! `syn` structurally cannot:
//!
//! - **Derive- and macro-generated trait impls** (`impl serde::Serialize for X`).
//! - **Fully-qualified, resolved paths** for both the implementing type and the
//!   trait (`axon::domain::model::DomainModel`, `serde_core::de::Deserialize`).
//!
//! This module is deliberately a thin, opt-in enrichment: it hand-rolls a
//! *minimal* deserializer for only the rustdoc-JSON fields it needs (resilient to
//! unrelated schema churn, no heavy `rustdoc-types` dependency), and generation is
//! gated behind an explicit call because it needs a nightly toolchain and a full
//! compile. `syn` remains the default ground-truth scanner.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

// ─── Minimal rustdoc-JSON schema (only the fields we consume) ───────────────
//
// Unknown fields are ignored by serde, so this tolerates the bulk of the schema
// changing between nightly versions as long as these few stable fields survive.

#[derive(Debug, Deserialize)]
struct RustdocCrate {
    format_version: u32,
    /// id → item. Keys are stringified integers; serde_json parses them to u32.
    index: HashMap<u32, Item>,
    /// id → resolved path summary (module path + defining crate).
    paths: HashMap<u32, ItemSummary>,
}

#[derive(Debug, Deserialize)]
struct Item {
    inner: ItemInner,
}

#[derive(Debug, Deserialize)]
struct ItemInner {
    /// Present only for `impl` items. `inner` is an internally-tagged object
    /// (`{"impl": {...}}`, `{"struct": {...}}`, …); we only care about impls.
    #[serde(rename = "impl", default)]
    impl_block: Option<ImplBlock>,
}

#[derive(Debug, Deserialize)]
struct ImplBlock {
    /// The implemented trait, or `None` for an inherent `impl`.
    #[serde(rename = "trait")]
    trait_: Option<PathRef>,
    /// The type the impl is `for`.
    #[serde(rename = "for")]
    for_: TypeRef,
    /// Auto-trait impls synthesized by rustdoc (Send/Sync/Unpin/…). Noise.
    #[serde(default)]
    is_synthetic: bool,
    /// Set when this impl comes from a blanket `impl<T> Trait for T`. Noise.
    #[serde(default)]
    blanket_impl: Option<serde_json::Value>,
}

/// A `{path, id, args}` reference (used for the `trait` field).
#[derive(Debug, Deserialize)]
struct PathRef {
    path: String,
    id: u32,
}

/// A rustdoc `Type`. We only consume the `resolved_path` variant (named types);
/// generics/primitives/refs are left as `None`.
#[derive(Debug, Deserialize)]
struct TypeRef {
    #[serde(default)]
    resolved_path: Option<PathRef>,
}

#[derive(Debug, Deserialize)]
struct ItemSummary {
    /// Fully-qualified path segments, e.g. `["serde_core", "de", "Deserialize"]`.
    path: Vec<String>,
    /// 0 is the local crate; others index `external_crates`.
    crate_id: u32,
}

// ─── Extracted facts ────────────────────────────────────────────────────────

/// A resolved trait implementation on a local type — including impls generated
/// by derives or other macros, which `syn` cannot see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedImpl {
    /// Fully-qualified path of the implementing type (`axon::domain::model::DomainModel`).
    pub type_path: String,
    /// Short type name (`DomainModel`).
    pub type_name: String,
    /// Fully-qualified path of the trait (`serde_core::de::Deserialize`), or the
    /// bare trait name when the trait's definition isn't in the index.
    pub trait_path: String,
    /// Short trait name (`Deserialize`).
    pub trait_name: String,
}

impl RustdocCrate {
    /// Resolve an id to its path summary, returning `(joined_path, short_name, crate_id)`.
    fn resolve(&self, id: u32) -> Option<(String, String, u32)> {
        let summary = self.paths.get(&id)?;
        let joined = summary.path.join("::");
        let short = summary
            .path
            .last()
            .cloned()
            .unwrap_or_else(|| joined.clone());
        Some((joined, short, summary.crate_id))
    }
}

/// Parse rustdoc JSON and extract resolved trait impls on **local** types
/// (`crate_id == 0`), skipping rustdoc-synthesized auto-trait impls and blanket
/// impls. Results are sorted and de-duplicated for stable output.
pub fn parse_resolved_impls(json: &str) -> Result<Vec<ResolvedImpl>> {
    let krate: RustdocCrate =
        serde_json::from_str(json).context("Failed to parse rustdoc JSON")?;

    let mut impls = Vec::new();
    for item in krate.index.values() {
        let Some(block) = &item.inner.impl_block else {
            continue;
        };
        if block.is_synthetic || block.blanket_impl.is_some() {
            continue;
        }
        let Some(trait_ref) = &block.trait_ else {
            continue; // inherent impl — no trait to record
        };
        let Some(for_path) = &block.for_.resolved_path else {
            continue; // impl for a generic/primitive/reference — skip
        };

        // Only keep impls *on* this crate's own types.
        let Some((type_path, type_name, crate_id)) = krate.resolve(for_path.id) else {
            continue;
        };
        if crate_id != 0 {
            continue;
        }

        // Resolve the trait fully; fall back to its bare name if it's not in the
        // index (e.g. some std/builtin traits).
        let (trait_path, trait_name) = krate
            .resolve(trait_ref.id)
            .map(|(p, n, _)| (p, n))
            .unwrap_or_else(|| (trait_ref.path.clone(), trait_ref.path.clone()));

        impls.push(ResolvedImpl {
            type_path,
            type_name,
            trait_path,
            trait_name,
        });
    }

    impls.sort_by(|a, b| {
        (&a.type_path, &a.trait_path).cmp(&(&b.type_path, &b.trait_path))
    });
    impls.dedup();
    let _ = krate.format_version; // reserved for future compatibility checks
    Ok(impls)
}

// ─── Generation (opt-in; needs a nightly toolchain) ─────────────────────────

/// Generate rustdoc JSON for the library target of the crate rooted at
/// `crate_dir` and return the parsed JSON as a string.
///
/// This shells out to a nightly `cargo doc --output-format json`. It is
/// deliberately *not* run as part of the default scan: it needs a nightly
/// toolchain and a full compile. Errors are returned (not panicked) so callers
/// can fall back to the `syn` model.
pub fn generate_rustdoc_json(crate_dir: &Path) -> Result<String> {
    use std::process::Command;

    // Locate a nightly toolchain via rustup. Homebrew's rust may shadow rustup on
    // PATH, so we resolve the nightly binaries explicitly and put them first.
    let nightly_cargo = rustup_which("nightly", "cargo")
        .context("a nightly toolchain is required for rustdoc JSON (install with `rustup toolchain install nightly`)")?;
    let nightly_bin = Path::new(&nightly_cargo)
        .parent()
        .map(|p| p.to_path_buf())
        .context("could not determine nightly toolchain bin directory")?;

    let target_dir = crate_dir.join("target").join("rustdoc-json");
    let path_env = match std::env::var_os("PATH") {
        Some(existing) => {
            let mut paths = vec![nightly_bin.clone()];
            paths.extend(std::env::split_paths(&existing));
            std::env::join_paths(paths).context("failed to compose PATH")?
        }
        None => nightly_bin.clone().into_os_string(),
    };

    let output = Command::new(&nightly_cargo)
        .current_dir(crate_dir)
        .args(["doc", "--lib", "--no-deps"])
        .arg("--target-dir")
        .arg(&target_dir)
        .env("PATH", &path_env)
        .env("RUSTDOCFLAGS", "-Zunstable-options --output-format=json")
        .output()
        .context("failed to invoke nightly `cargo doc`")?;

    if !output.status.success() {
        anyhow::bail!(
            "rustdoc JSON generation failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // `cargo doc --no-deps` writes exactly one crate JSON under <target>/doc/.
    let doc_dir = target_dir.join("doc");
    let json_path = newest_json_in(&doc_dir)
        .with_context(|| format!("no rustdoc JSON produced in {}", doc_dir.display()))?;
    std::fs::read_to_string(&json_path)
        .with_context(|| format!("failed to read rustdoc JSON at {}", json_path.display()))
}

/// Resolve a toolchain binary path via `rustup which`.
fn rustup_which(toolchain: &str, bin: &str) -> Result<String> {
    use std::process::Command;
    let out = Command::new("rustup")
        .args(["which", "--toolchain", toolchain, bin])
        .output()
        .context("`rustup` not found on PATH")?;
    if !out.status.success() {
        anyhow::bail!(
            "`rustup which --toolchain {toolchain} {bin}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The most recently modified `*.json` file directly under `dir`.
fn newest_json_in(dir: &Path) -> Result<std::path::PathBuf> {
    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(modified) = entry.metadata().and_then(|m| m.modified()).ok() else {
            continue;
        };
        if newest.as_ref().is_none_or(|(t, _)| modified >= *t) {
            newest = Some((modified, path));
        }
    }
    newest
        .map(|(_, p)| p)
        .context("directory contains no .json files")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal rustdoc-JSON document exercising the four impl cases:
    /// a derive-generated trait impl (keep), an auto-trait synthetic impl (skip),
    /// a blanket impl (skip), and an inherent impl (skip).
    const FIXTURE: &str = r#"{
        "format_version": 57,
        "root": 1,
        "external_crates": { "30": { "name": "serde_core" } },
        "index": {
            "100": { "inner": { "impl": {
                "trait": { "path": "Deserialize", "id": 489, "args": null },
                "for": { "resolved_path": { "path": "DomainModel", "id": 465, "args": null } },
                "is_synthetic": false, "blanket_impl": null, "items": []
            } } },
            "101": { "inner": { "impl": {
                "trait": { "path": "Send", "id": 5, "args": null },
                "for": { "resolved_path": { "path": "DomainModel", "id": 465, "args": null } },
                "is_synthetic": true, "blanket_impl": null, "items": []
            } } },
            "102": { "inner": { "impl": {
                "trait": { "path": "Borrow", "id": 19, "args": null },
                "for": { "resolved_path": { "path": "DomainModel", "id": 465, "args": null } },
                "is_synthetic": false, "blanket_impl": { "kind": "blanket" }, "items": []
            } } },
            "103": { "inner": { "impl": {
                "trait": null,
                "for": { "resolved_path": { "path": "DomainModel", "id": 465, "args": null } },
                "is_synthetic": false, "blanket_impl": null, "items": []
            } } },
            "104": { "inner": { "struct": { "kind": { "plain": { "fields": [] } } } } }
        },
        "paths": {
            "465": { "path": ["axon", "domain", "model", "DomainModel"], "kind": "struct", "crate_id": 0 },
            "489": { "path": ["serde_core", "de", "Deserialize"], "kind": "trait", "crate_id": 30 }
        }
    }"#;

    #[test]
    fn keeps_only_real_trait_impls_on_local_types() {
        let impls = parse_resolved_impls(FIXTURE).expect("parse");
        assert_eq!(impls.len(), 1, "synthetic, blanket, inherent must be dropped: {impls:?}");
        let imp = &impls[0];
        assert_eq!(imp.type_path, "axon::domain::model::DomainModel");
        assert_eq!(imp.type_name, "DomainModel");
        assert_eq!(imp.trait_path, "serde_core::de::Deserialize");
        assert_eq!(imp.trait_name, "Deserialize");
    }

    #[test]
    fn derive_generated_impl_is_visible() {
        // The whole point: the Deserialize impl exists only because of
        // `#[derive(Deserialize)]` — syn never sees it, rustdoc does.
        let impls = parse_resolved_impls(FIXTURE).expect("parse");
        assert!(impls.iter().any(|i| i.trait_name == "Deserialize"));
    }
}
