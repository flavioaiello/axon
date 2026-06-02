//! rust-analyzer-backed resolved call graph, using rust-analyzer as an in-process
//! library instead of spawning the `rust-analyzer` binary.
//!
//! The fast `syn` scanner records call sites by name (`self.save()` -> `save`),
//! but only rust-analyzer's semantic model can resolve receiver methods, traits,
//! aliases, and inference-sensitive calls to concrete definitions. This module
//! keeps Axon's public contract (`resolve_calls`) while loading the workspace via
//! the `ra_ap_*` crates and querying call hierarchy directly.

use anyhow::{Context, Result, anyhow, bail};
use proc_macro2::LineColumn;
use quote::ToTokens;
use ra_ap_ide::{
    AnalysisHost, CallHierarchyConfig, FileId, FilePosition, NavigationTarget, RaFixtureConfig,
    TextSize,
};
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use syn::parse::Parser;

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

/// A callable definition discovered syntactically, with enough position data to
/// ask rust-analyzer for its semantic outgoing call hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Callable {
    file_rel: String,
    qualified: String,
    line: usize,
    column: usize,
}

/// Resolve the workspace's intra-crate call edges via rust-analyzer libraries.
///
/// The implementation uses two cooperating layers:
///
/// 1. A lightweight `syn` pass finds callable definition positions and Axon's
///    preferred qualified caller names (`Owner::method`, free `function`).
/// 2. `ra_ap_load_cargo` loads rust-analyzer's semantic database, and
///    `Analysis::outgoing_calls` resolves each callable's outgoing calls to
///    concrete workspace definitions.
pub fn resolve_calls(workspace_root: &Path) -> Result<Vec<ResolvedCall>> {
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

    let mut callables = Vec::new();
    let mut source_by_file = HashMap::new();
    for file in &files {
        let rel = rel_path(&root, file);
        let source = std::fs::read_to_string(file)
            .with_context(|| format!("read Rust source: {}", file.display()))?;
        callables.extend(collect_callables(&rel, &source)?);
        source_by_file.insert(rel, source);
    }
    if callables.is_empty() {
        return Ok(Vec::new());
    }

    let cargo_config = CargoConfig {
        all_targets: true,
        set_test: true,
        ..CargoConfig::default()
    };
    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: false,
        num_worker_threads: 1,
        proc_macro_processes: 1,
    };
    let (db, vfs, _proc_macro) = load_workspace_at(&root, &cargo_config, &load_config, &|_| {})
        .context("load workspace through rust-analyzer libraries")?;
    let analysis = AnalysisHost::with_database(db).analysis();
    let file_ids = rust_file_ids(&root, &vfs);

    let defs_by_line: HashMap<(String, usize), String> = callables
        .iter()
        .map(|callable| {
            (
                (callable.file_rel.clone(), callable.line.saturating_sub(1)),
                callable.qualified.clone(),
            )
        })
        .collect();
    let config = CallHierarchyConfig {
        exclude_tests: false,
        ra_fixture: RaFixtureConfig::default(),
    };

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for callable in &callables {
        let Some(file_id) = file_ids.get(&callable.file_rel).copied() else {
            continue;
        };
        let Some(source) = source_by_file.get(&callable.file_rel) else {
            continue;
        };
        let Some(offset) = offset_for_line_column(source, callable.line, callable.column) else {
            continue;
        };
        let calls = analysis
            .outgoing_calls(&config, FilePosition { file_id, offset })
            .map_err(|_| anyhow!("rust-analyzer outgoing call query was cancelled"))?
            .unwrap_or_default();

        for call in calls {
            let target = call.target;
            let Some((callee_file, line0)) = target_location(&root, &vfs, &analysis, &target)
            else {
                continue;
            };
            let callee = defs_by_line
                .get(&(callee_file.clone(), line0))
                .cloned()
                .unwrap_or_else(|| qualified_target_name(&target));
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
                    callee_line: line0 + 1,
                });
            }
        }
    }

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

fn rust_file_ids(root: &Path, vfs: &ra_ap_vfs::Vfs) -> HashMap<String, FileId> {
    vfs.iter()
        .filter_map(|(file_id, path)| {
            if path.name_and_extension().and_then(|(_, ext)| ext) != Some("rs") {
                return None;
            }
            let abs = PathBuf::from(path.to_string());
            let rel = abs.strip_prefix(root).ok()?.to_string_lossy().to_string();
            Some((rel, file_id))
        })
        .collect()
}

fn target_location(
    root: &Path,
    vfs: &ra_ap_vfs::Vfs,
    analysis: &ra_ap_ide::Analysis,
    target: &NavigationTarget,
) -> Option<(String, usize)> {
    let abs = PathBuf::from(vfs.file_path(target.file_id).to_string());
    let rel = abs.strip_prefix(root).ok()?.to_string_lossy().to_string();
    let line_index = analysis.file_line_index(target.file_id).ok()?;
    let line_col = line_index.try_line_col(target.focus_or_full_range().start())?;
    Some((rel, line_col.line as usize))
}

fn qualified_target_name(target: &NavigationTarget) -> String {
    match &target.container_name {
        Some(container) => format!("{}::{}", container, target.name),
        None => target.name.to_string(),
    }
}

fn collect_callables(file_rel: &str, source: &str) -> Result<Vec<Callable>> {
    let syntax_tree = syn::parse_file(source).with_context(|| format!("parse {file_rel}"))?;
    let mut callables = Vec::new();
    collect_callables_from_items(file_rel, &syntax_tree.items, &mut callables);
    Ok(callables)
}

fn collect_callables_from_items(
    file_rel: &str,
    items: &[syn::Item],
    callables: &mut Vec<Callable>,
) {
    for item in items {
        match item {
            syn::Item::Fn(func) => {
                if has_cfg_test(&func.attrs) {
                    continue;
                }
                push_callable(
                    file_rel,
                    func.sig.ident.to_string(),
                    func.sig.ident.span().start(),
                    callables,
                );
            }
            syn::Item::Impl(imp) => {
                if has_cfg_test(&imp.attrs) {
                    continue;
                }
                let owner = type_to_string(&imp.self_ty);
                for impl_item in &imp.items {
                    let syn::ImplItem::Fn(method) = impl_item else {
                        continue;
                    };
                    if has_cfg_test(&method.attrs) {
                        continue;
                    }
                    push_callable(
                        file_rel,
                        format!("{}::{}", owner, method.sig.ident),
                        method.sig.ident.span().start(),
                        callables,
                    );
                }
            }
            syn::Item::Trait(tr) => {
                if has_cfg_test(&tr.attrs) {
                    continue;
                }
                for trait_item in &tr.items {
                    let syn::TraitItem::Fn(method) = trait_item else {
                        continue;
                    };
                    if has_cfg_test(&method.attrs) {
                        continue;
                    }
                    push_callable(
                        file_rel,
                        format!("{}::{}", tr.ident, method.sig.ident),
                        method.sig.ident.span().start(),
                        callables,
                    );
                }
            }
            syn::Item::Mod(module) => {
                if module.ident == "tests" || has_cfg_test(&module.attrs) {
                    continue;
                }
                if let Some((_, nested)) = &module.content {
                    collect_callables_from_items(file_rel, nested, callables);
                }
            }
            _ => {}
        }
    }
}

fn push_callable(
    file_rel: &str,
    qualified: String,
    location: LineColumn,
    callables: &mut Vec<Callable>,
) {
    callables.push(Callable {
        file_rel: file_rel.to_string(),
        qualified,
        line: location.line,
        column: location.column,
    });
}

fn offset_for_line_column(source: &str, line: usize, column: usize) -> Option<TextSize> {
    let line_start = line_start_offset(source, line)?;
    let line_end = source[line_start..]
        .find('\n')
        .map(|i| line_start + i)
        .unwrap_or(source.len());
    let offset = std::cmp::min(line_start + column, line_end);
    u32::try_from(offset).ok().map(TextSize::new)
}

fn line_start_offset(source: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return None;
    }
    if line == 1 {
        return Some(0);
    }
    let mut current_line = 1;
    for (idx, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            current_line += 1;
            if current_line == line {
                return Some(idx + 1);
            }
        }
    }
    None
}

/// Workspace-relative path string for a file under `root`.
fn rel_path(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .to_string()
}

fn has_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        attr.parse_args::<syn::Meta>()
            .map(|meta| cfg_meta_contains_test(&meta))
            .unwrap_or(false)
    })
}

fn cfg_meta_contains_test(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::NameValue(_) => false,
        syn::Meta::List(list) => {
            if list.path.is_ident("test") {
                return true;
            }
            let parser = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated;
            parser
                .parse2(list.tokens.clone())
                .map(|nested| nested.iter().any(cfg_meta_contains_test))
                .unwrap_or(false)
        }
    }
}

fn type_to_string(ty: &syn::Type) -> String {
    let mut tokens = proc_macro2::TokenStream::new();
    ty.to_tokens(&mut tokens);
    let raw = tokens.to_string();
    let mut result = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == ' ' {
            while chars.peek() == Some(&' ') {
                chars.next();
            }
            let prev_is_lifetime = result.chars().last().is_some_and(|c| c.is_alphanumeric());
            let next_is_ident = chars.peek().is_some_and(|c| c.is_alphabetic() || *c == '_');
            if prev_is_lifetime && next_is_ident && result.contains('\'') {
                result.push(' ');
            }
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_callables_qualifies_impl_and_trait_methods() {
        let source = r#"
fn free_fn() {}

impl Store {
    fn save(&self) {}
}

trait Port {
    fn send(&self);
}
"#;
        let callables = collect_callables("src/store/cozo.rs", source).unwrap();
        let names: Vec<&str> = callables.iter().map(|c| c.qualified.as_str()).collect();
        assert!(names.contains(&"free_fn"));
        assert!(names.contains(&"Store::save"));
        assert!(names.contains(&"Port::send"));
    }

    #[test]
    fn offset_for_line_column_uses_one_based_lines() {
        let source = "abc\n    fn save() {}\n";
        let offset = offset_for_line_column(source, 2, 7).unwrap();
        assert_eq!(u32::from(offset), 11);
    }

    #[test]
    fn type_to_string_extracts_impl_owner_text() {
        let ty: syn::Type = syn::parse_str("Store").unwrap();
        assert_eq!(type_to_string(&ty), "Store");

        let ty: syn::Type = syn::parse_str("Foo<T>").unwrap();
        assert_eq!(type_to_string(&ty), "Foo<T>");
    }
}
