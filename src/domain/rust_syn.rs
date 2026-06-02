use anyhow::{Context, Result};
use std::path::Path;
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::visit::Visit;

use super::ast::{
    CallInfo, CodeReference, DiscoveredDirective, DiscoveredEnum, DiscoveredFunction,
    DiscoveredMethod, DiscoveredModule, DiscoveredStruct, DiscoveredTrait, FileScan,
    LiveDependency, ScanResult,
};
use super::model::Field;
use super::scanner::AstScanner;

pub struct RustSynScanner;

/// Parse a Rust source file, attributing parse errors to its path.
fn parse_file(file_path: &Path, source_code: &str) -> Result<syn::File> {
    syn::parse_file(source_code)
        .with_context(|| format!("Failed to parse: {}", file_path.display()))
}

struct ImportVisitor {
    imports: Vec<String>,
}

impl<'ast> Visit<'ast> for ImportVisitor {
    fn visit_use_tree(&mut self, node: &'ast syn::UseTree) {
        fn extract_paths(tree: &syn::UseTree, prefix: &str) -> Vec<String> {
            fn join(prefix: &str, segment: &str) -> String {
                if prefix.is_empty() {
                    segment.to_string()
                } else {
                    format!("{prefix}::{segment}")
                }
            }

            match tree {
                syn::UseTree::Path(path) => {
                    extract_paths(&path.tree, &join(prefix, &path.ident.to_string()))
                }
                syn::UseTree::Name(name) => vec![join(prefix, &name.ident.to_string())],
                syn::UseTree::Rename(rename) => {
                    vec![join(prefix, &rename.ident.to_string())]
                }
                syn::UseTree::Glob(_) => vec![join(prefix, "*")],
                syn::UseTree::Group(group) => {
                    let mut paths = vec![];
                    for item in &group.items {
                        paths.extend(extract_paths(item, prefix));
                    }
                    paths
                }
            }
        }
        self.imports.extend(extract_paths(node, ""));
    }
}

/// Extract `use`-path import strings from an already-parsed file.
fn collect_imports(syntax_tree: &syn::File) -> Vec<String> {
    let mut visitor = ImportVisitor { imports: vec![] };
    visitor.visit_file(syntax_tree);
    visitor.imports
}

struct ReferenceVisitor {
    references: Vec<CodeReference>,
}

impl ReferenceVisitor {
    fn record_path(&mut self, path: &syn::Path, reference_kind: &str, line: usize) {
        let to_path = path_to_string(path);
        if should_record_reference(&to_path) {
            self.references.push(CodeReference {
                to_path,
                reference_kind: reference_kind.to_string(),
                line,
            });
        }
    }
}

impl<'ast> Visit<'ast> for ReferenceVisitor {
    fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
        self.record_path(&node.path, "type", node.path.span().start().line);
        syn::visit::visit_type_path(self, node);
    }

    fn visit_type_param_bound(&mut self, node: &'ast syn::TypeParamBound) {
        if let syn::TypeParamBound::Trait(bound) = node {
            self.record_path(&bound.path, "trait_bound", bound.path.span().start().line);
        }
        syn::visit::visit_type_param_bound(self, node);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        self.record_path(&node.path, "expr_path", node.path.span().start().line);
        syn::visit::visit_expr_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.record_path(&node.path, "macro", node.path.span().start().line);
        syn::visit::visit_macro(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if node.ident == "tests" || has_cfg_test(&node.attrs) {
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }
}

fn collect_references(syntax_tree: &syn::File) -> Vec<CodeReference> {
    use std::collections::HashSet;

    let mut visitor = ReferenceVisitor { references: vec![] };
    visitor.visit_file(syntax_tree);

    let mut seen = HashSet::new();
    visitor
        .references
        .into_iter()
        .filter(|reference| {
            seen.insert((
                reference.to_path.clone(),
                reference.reference_kind.clone(),
                reference.line,
            ))
        })
        .collect()
}

fn path_to_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn should_record_reference(path: &str) -> bool {
    let segments: Vec<&str> = path
        .split("::")
        .filter(|segment| !segment.is_empty())
        .collect();
    let Some(first) = segments.first() else {
        return false;
    };
    if *first == "self" || *first == "Self" || *first == "crate" || *first == "super" {
        return true;
    }
    if segments.len() > 1 {
        return true;
    }
    let single = *first;
    if is_primitive_or_prelude_type(single) {
        return false;
    }
    single
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn is_primitive_or_prelude_type(name: &str) -> bool {
    matches!(
        name,
        "str"
            | "bool"
            | "char"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
            | "String"
            | "Option"
            | "Result"
            | "Vec"
            | "Box"
            | "Arc"
            | "Rc"
            | "Pin"
    )
}

/// Extract structs, enums, methods, modules, and traits from an already-parsed
/// file (including the trait-impl `implements` backfill).
fn collect_items(syntax_tree: &syn::File, file_path: &str) -> ScanResult {
    let mut visitor = StructMethodVisitor {
        structs: vec![],
        enums: vec![],
        methods: vec![],
        modules: vec![],
        traits: vec![],
        trait_impls: vec![],
        file_path: file_path.to_string(),
    };
    visitor.visit_file(syntax_tree);

    // Backfill `implements` on the implementing types from recorded trait impls.
    // Indexed by type name so this is O(types + impls), not O(types × impls).
    use std::collections::HashMap;
    let mut struct_idx: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, s) in visitor.structs.iter().enumerate() {
        struct_idx.entry(s.name.as_str()).or_default().push(i);
    }
    let mut enum_idx: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, e) in visitor.enums.iter().enumerate() {
        enum_idx.entry(e.name.as_str()).or_default().push(i);
    }
    let backfill: Vec<(Vec<usize>, Vec<usize>, String)> = visitor
        .trait_impls
        .iter()
        .map(|(type_name, trait_name)| {
            (
                struct_idx
                    .get(type_name.as_str())
                    .cloned()
                    .unwrap_or_default(),
                enum_idx
                    .get(type_name.as_str())
                    .cloned()
                    .unwrap_or_default(),
                trait_name.clone(),
            )
        })
        .collect();
    for (struct_hits, enum_hits, trait_name) in backfill {
        for i in struct_hits {
            visitor.structs[i].implements.push(trait_name.clone());
        }
        for i in enum_hits {
            visitor.enums[i].implements.push(trait_name.clone());
        }
    }

    (
        visitor.structs,
        visitor.enums,
        visitor.methods,
        visitor.modules,
        visitor.traits,
    )
}

/// Visitor that collects public free (module-level) functions.
struct FnVisitor {
    functions: Vec<DiscoveredFunction>,
    file_path: String,
}

impl<'ast> Visit<'ast> for FnVisitor {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if is_public(&node.vis) && !has_cfg_test(&node.attrs) {
            self.functions.push(DiscoveredFunction {
                name: node.sig.ident.to_string(),
                start_line: node.sig.span().start().line,
                end_line: node.span().end().line,
                file_path: self.file_path.clone(),
            });
        }
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if node.ident == "tests" || has_cfg_test(&node.attrs) {
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }
}

/// Extract public free functions from an already-parsed file. Methods (inside
/// `impl`/trait blocks) are not free functions and are not collected here.
fn collect_functions(syntax_tree: &syn::File, file_path: &str) -> Vec<DiscoveredFunction> {
    let mut visitor = FnVisitor {
        functions: vec![],
        file_path: file_path.to_string(),
    };
    visitor.visit_file(syntax_tree);
    visitor.functions
}

/// Extract function/method call edges from an already-parsed file.
///
/// Walks every nesting level — including inline `mod` blocks — and both inherent
/// **and** trait impls, because the method bodies hold real call sites regardless
/// of whether the signature mirrors a trait. `self`/`Self` receivers are resolved
/// against the enclosing impl's type, so `self.save()` inside `impl Store` is
/// recorded as a call to `Store::save`, matching the method-symbol naming. This
/// is a static approximation: non-`self` receivers need type inference (a
/// rustc/rust-analyzer concern) and stay name-only.
fn collect_calls(syntax_tree: &syn::File) -> Vec<CallInfo> {
    let mut calls = Vec::new();
    collect_calls_from_items(&syntax_tree.items, &mut calls);
    calls
}

/// Recursively walk a list of items (a file or a module body) for call sites.
fn collect_calls_from_items(items: &[syn::Item], calls: &mut Vec<CallInfo>) {
    for item in items {
        match item {
            syn::Item::Impl(imp) => {
                if has_cfg_test(&imp.attrs) {
                    continue;
                }
                let owner = type_to_string(&imp.self_ty);
                for impl_item in &imp.items {
                    if let syn::ImplItem::Fn(method) = impl_item {
                        if has_cfg_test(&method.attrs) {
                            continue;
                        }
                        let caller = format!("{}::{}", owner, method.sig.ident);
                        collect_calls_from_block(&method.block, &caller, Some(&owner), calls);
                    }
                }
            }
            syn::Item::Fn(func) => {
                if has_cfg_test(&func.attrs) {
                    continue;
                }
                let caller = func.sig.ident.to_string();
                collect_calls_from_block(&func.block, &caller, None, calls);
            }
            syn::Item::Trait(tr) => {
                if has_cfg_test(&tr.attrs) {
                    continue;
                }
                // Trait *default* method bodies contain calls too. `Self` here is
                // the (unknown) implementor, so self-resolution is left off.
                for trait_item in &tr.items {
                    let syn::TraitItem::Fn(method) = trait_item else {
                        continue;
                    };
                    if has_cfg_test(&method.attrs) {
                        continue;
                    }
                    let Some(block) = &method.default else {
                        continue;
                    };
                    let caller = format!("{}::{}", tr.ident, method.sig.ident);
                    collect_calls_from_block(block, &caller, None, calls);
                }
            }
            syn::Item::Mod(m) => {
                if m.ident == "tests" || has_cfg_test(&m.attrs) {
                    continue;
                }
                if let Some((_, mod_items)) = &m.content {
                    collect_calls_from_items(mod_items, calls);
                }
            }
            _ => {}
        }
    }
}

impl AstScanner for RustSynScanner {
    fn extract_live_dependencies(
        &self,
        file_path: &Path,
        source_code: &str,
    ) -> Result<Vec<LiveDependency>> {
        let syntax_tree = parse_file(file_path, source_code)?;
        let from_file = file_path.to_string_lossy().to_string();
        Ok(collect_imports(&syntax_tree)
            .into_iter()
            .map(|to_module| LiveDependency {
                from_file: from_file.clone(),
                to_module,
            })
            .collect())
    }

    fn scan_file(&self, file_path: &Path, source_code: &str) -> Result<ScanResult> {
        let syntax_tree = parse_file(file_path, source_code)?;
        Ok(collect_items(&syntax_tree, &file_path.to_string_lossy()))
    }

    fn extract_calls(&self, file_path: &Path, source_code: &str) -> Result<Vec<CallInfo>> {
        let syntax_tree = parse_file(file_path, source_code)?;
        Ok(collect_calls(&syntax_tree))
    }

    fn scan_source(&self, file_path: &Path, source_code: &str) -> Result<FileScan> {
        // Single parse shared by all three extractors.
        let syntax_tree = parse_file(file_path, source_code)?;
        let from_file = file_path.to_string_lossy().to_string();
        let (structs, enums, methods, modules, traits) = collect_items(&syntax_tree, &from_file);
        let functions = collect_functions(&syntax_tree, &from_file);
        let imports = collect_imports(&syntax_tree)
            .into_iter()
            .map(|to_module| LiveDependency {
                from_file: from_file.clone(),
                to_module,
            })
            .collect();
        let calls = collect_calls(&syntax_tree);
        let references = collect_references(&syntax_tree);
        let directives = collect_directives(&syntax_tree);
        Ok(FileScan {
            structs,
            enums,
            methods,
            modules,
            traits,
            functions,
            imports,
            calls,
            references,
            directives,
        })
    }
}

/// Recursively walk a block's expressions collecting call sites.
///
/// `owner` is the `Self` type of the enclosing impl (if any), used to resolve
/// `self.method()` / `Self::assoc()` to the concrete `Owner::name`.
fn collect_calls_from_block(
    block: &syn::Block,
    caller: &str,
    owner: Option<&str>,
    calls: &mut Vec<CallInfo>,
) {
    for stmt in &block.stmts {
        match stmt {
            syn::Stmt::Expr(expr, _) => {
                collect_calls_from_expr(expr, caller, owner, calls);
            }
            syn::Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    collect_calls_from_expr(&init.expr, caller, owner, calls);
                }
            }
            _ => {}
        }
    }
}

fn collect_calls_from_expr(
    expr: &syn::Expr,
    caller: &str,
    owner: Option<&str>,
    calls: &mut Vec<CallInfo>,
) {
    match expr {
        syn::Expr::Call(call) => {
            if let syn::Expr::Path(path) = call.func.as_ref() {
                let mut segments: Vec<String> = path
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                // Resolve a leading `Self::` to the concrete impl type.
                match (owner, segments.first_mut()) {
                    (Some(o), Some(first)) if first == "Self" => *first = o.to_string(),
                    _ => {}
                }
                if !segments.is_empty() {
                    calls.push(CallInfo {
                        caller: caller.to_string(),
                        callee: segments.join("::"),
                        line: call.paren_token.span.open().start().line,
                    });
                }
            }
            // Walk the callee expression and arguments regardless, so calls
            // nested inside a non-path callee or the args are not lost.
            collect_calls_from_expr(&call.func, caller, owner, calls);
            for arg in &call.args {
                collect_calls_from_expr(arg, caller, owner, calls);
            }
        }
        syn::Expr::MethodCall(mc) => {
            // `self.method()` resolves to `Owner::method`; any other receiver
            // would need type inference, so it stays name-only.
            let receiver_is_self = matches!(
                mc.receiver.as_ref(),
                syn::Expr::Path(p) if p.path.is_ident("self")
            );
            let callee = match owner {
                Some(o) if receiver_is_self => format!("{o}::{}", mc.method),
                _ => mc.method.to_string(),
            };
            calls.push(CallInfo {
                caller: caller.to_string(),
                callee,
                line: mc.method.span().start().line,
            });
            collect_calls_from_expr(&mc.receiver, caller, owner, calls);
            for arg in &mc.args {
                collect_calls_from_expr(arg, caller, owner, calls);
            }
        }
        syn::Expr::Block(b) => collect_calls_from_block(&b.block, caller, owner, calls),
        syn::Expr::If(i) => {
            collect_calls_from_expr(&i.cond, caller, owner, calls);
            collect_calls_from_block(&i.then_branch, caller, owner, calls);
            if let Some((_, else_branch)) = &i.else_branch {
                collect_calls_from_expr(else_branch, caller, owner, calls);
            }
        }
        syn::Expr::Match(m) => {
            collect_calls_from_expr(&m.expr, caller, owner, calls);
            for arm in &m.arms {
                collect_calls_from_expr(&arm.body, caller, owner, calls);
            }
        }
        syn::Expr::Closure(c) => {
            collect_calls_from_expr(&c.body, caller, owner, calls);
        }
        syn::Expr::Return(r) => {
            if let Some(e) = &r.expr {
                collect_calls_from_expr(e, caller, owner, calls);
            }
        }
        syn::Expr::Try(t) => collect_calls_from_expr(&t.expr, caller, owner, calls),
        syn::Expr::Paren(p) => collect_calls_from_expr(&p.expr, caller, owner, calls),
        syn::Expr::Reference(r) => collect_calls_from_expr(&r.expr, caller, owner, calls),
        syn::Expr::Unary(u) => collect_calls_from_expr(&u.expr, caller, owner, calls),
        syn::Expr::Binary(b) => {
            collect_calls_from_expr(&b.left, caller, owner, calls);
            collect_calls_from_expr(&b.right, caller, owner, calls);
        }
        syn::Expr::Let(l) => collect_calls_from_expr(&l.expr, caller, owner, calls),
        syn::Expr::Tuple(t) => {
            for e in &t.elems {
                collect_calls_from_expr(e, caller, owner, calls);
            }
        }
        syn::Expr::Array(a) => {
            for e in &a.elems {
                collect_calls_from_expr(e, caller, owner, calls);
            }
        }
        syn::Expr::Field(f) => collect_calls_from_expr(&f.base, caller, owner, calls),
        syn::Expr::Index(i) => {
            collect_calls_from_expr(&i.expr, caller, owner, calls);
            collect_calls_from_expr(&i.index, caller, owner, calls);
        }
        syn::Expr::Await(a) => collect_calls_from_expr(&a.base, caller, owner, calls),
        syn::Expr::Unsafe(u) => collect_calls_from_block(&u.block, caller, owner, calls),
        syn::Expr::Loop(l) => collect_calls_from_block(&l.body, caller, owner, calls),
        syn::Expr::While(w) => {
            collect_calls_from_expr(&w.cond, caller, owner, calls);
            collect_calls_from_block(&w.body, caller, owner, calls);
        }
        syn::Expr::ForLoop(f) => {
            collect_calls_from_expr(&f.expr, caller, owner, calls);
            collect_calls_from_block(&f.body, caller, owner, calls);
        }
        syn::Expr::Struct(s) => {
            for field in &s.fields {
                collect_calls_from_expr(&field.expr, caller, owner, calls);
            }
        }
        syn::Expr::Group(g) => collect_calls_from_expr(&g.expr, caller, owner, calls),
        syn::Expr::Cast(c) => collect_calls_from_expr(&c.expr, caller, owner, calls),
        syn::Expr::Async(a) => collect_calls_from_block(&a.block, caller, owner, calls),
        syn::Expr::TryBlock(t) => collect_calls_from_block(&t.block, caller, owner, calls),
        syn::Expr::Range(r) => {
            if let Some(s) = &r.start {
                collect_calls_from_expr(s, caller, owner, calls);
            }
            if let Some(e) = &r.end {
                collect_calls_from_expr(e, caller, owner, calls);
            }
        }
        syn::Expr::Assign(a) => {
            collect_calls_from_expr(&a.left, caller, owner, calls);
            collect_calls_from_expr(&a.right, caller, owner, calls);
        }
        syn::Expr::Repeat(r) => {
            collect_calls_from_expr(&r.expr, caller, owner, calls);
            collect_calls_from_expr(&r.len, caller, owner, calls);
        }
        _ => {}
    }
}

fn is_public(vis: &syn::Visibility) -> bool {
    matches!(vis, syn::Visibility::Public(_))
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
    quote::ToTokens::to_tokens(ty, &mut tokens);
    // Normalize whitespace: collapse runs of spaces but preserve a single space
    // after lifetime tokens (e.g., `& 'a  DomainModel` → `&'a DomainModel`).
    let raw = tokens.to_string();
    let mut result = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == ' ' {
            // Peek at what follows: keep exactly one space before identifiers
            // that follow a lifetime (apostrophe + ident sequence).
            while chars.peek() == Some(&' ') {
                chars.next();
            }
            // Check if previous token ended with a lifetime identifier char
            // and next is an alpha/underscore (type name after lifetime).
            let prev_is_lifetime = result.chars().last().is_some_and(|c| c.is_alphanumeric());
            let next_is_ident = chars.peek().is_some_and(|c| c.is_alphabetic() || *c == '_');
            if prev_is_lifetime && next_is_ident {
                // Check if we're after a lifetime ('a, 'b, etc.)
                let has_lifetime = result.contains('\'');
                if has_lifetime {
                    result.push(' ');
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

fn is_option_type(ty: &syn::Type) -> bool {
    let type_str = type_to_string(ty);
    type_str.starts_with("Option<") || type_str.starts_with("std::option::Option<")
}

/// Lint-level attributes whose arguments are individual lint names.
const LINT_LEVELS: [&str; 4] = ["allow", "deny", "warn", "forbid"];

/// Render an attribute path (`derive`, `serde`, `clippy::needless_return`, …)
/// as a `::`-joined string.
fn attr_path_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// Normalize a single attribute into zero or more directive strings.
///
/// Lint directives are expanded one entry per lint so a single lint name is
/// directly queryable through the AST-edge graph — e.g. `#[allow(dead_code)]`
/// becomes `"allow(dead_code)"` and `#[deny(unused, missing_docs)]` becomes
/// `"deny(unused)"` + `"deny(missing_docs)"`. Conditional-compilation predicates
/// are captured verbatim as `"cfg(<predicate>)"` and derives are split into
/// individual names. `#[doc]` is dropped as noise.
fn directive_strings(attr: &syn::Attribute) -> Vec<String> {
    let mut out = Vec::new();
    let path = attr_path_string(attr.path());

    if path == "derive" {
        // Parse derive(A, B, C) into individual decorator names.
        let _ = attr.parse_nested_meta(|meta| {
            let dpath = attr_path_string(&meta.path);
            if !dpath.is_empty() {
                out.push(dpath);
            }
            Ok(())
        });
    } else if LINT_LEVELS.contains(&path.as_str()) {
        // Expand each lint into `level(lint)`, skipping a trailing
        // `reason = "..."` (Rust 1.81+ lint reasons carry a value).
        let _ = attr.parse_nested_meta(|meta| {
            if meta.input.peek(syn::Token![=]) {
                let value = meta.value()?;
                let _: syn::Expr = value.parse()?;
            } else {
                let lint = attr_path_string(&meta.path);
                if !lint.is_empty() {
                    out.push(format!("{path}({lint})"));
                }
            }
            Ok(())
        });
    } else if path == "cfg" {
        // Conditional compilation: capture the full predicate verbatim.
        if let Ok(list) = attr.meta.require_list() {
            out.push(format!("cfg({})", list.tokens));
        }
    } else if !path.is_empty() && path != "doc" {
        // Other attributes like #[serde(...)], #[tokio::main], #[inline], etc.
        out.push(path);
    }
    out
}

/// Extract directives from an attribute list, tagging each with the 1-based line
/// of its attribute so the graph can point at the exact declaration.
fn directives_of(attrs: &[syn::Attribute]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for attr in attrs {
        let line = attr.span().start().line;
        for directive in directive_strings(attr) {
            out.push((directive, line));
        }
    }
    out
}

/// Collect compiler directives from a parsed file, independent of the public-API
/// symbol scan.
///
/// Unlike [`StructMethodVisitor`], this walks items of **any** visibility (plus
/// struct fields and enum variants), because directives like `#[allow(dead_code)]`
/// almost always sit on private/unused items. `#[cfg(test)]`-gated items and
/// `tests` modules are skipped, matching the rest of the scan.
struct DirectiveVisitor {
    directives: Vec<DiscoveredDirective>,
}

impl DirectiveVisitor {
    fn record(&mut self, owner: &str, attrs: &[syn::Attribute]) {
        for (directive, line) in directives_of(attrs) {
            self.directives.push(DiscoveredDirective {
                owner: owner.to_string(),
                directive,
                line,
            });
        }
    }
}

impl<'ast> Visit<'ast> for DirectiveVisitor {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        let name = node.ident.to_string();
        self.record(&name, &node.attrs);
        match &node.fields {
            syn::Fields::Named(named) => {
                for f in &named.named {
                    if let Some(ident) = &f.ident {
                        self.record(&format!("{name}.{ident}"), &f.attrs);
                    }
                }
            }
            syn::Fields::Unnamed(unnamed) => {
                for (i, f) in unnamed.unnamed.iter().enumerate() {
                    self.record(&format!("{name}.{i}"), &f.attrs);
                }
            }
            syn::Fields::Unit => {}
        }
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        let name = node.ident.to_string();
        self.record(&name, &node.attrs);
        for v in &node.variants {
            self.record(&format!("{name}.{}", v.ident), &v.attrs);
        }
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.record(&node.sig.ident.to_string(), &node.attrs);
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        let name = node.ident.to_string();
        self.record(&name, &node.attrs);
        for item in &node.items {
            if let syn::TraitItem::Fn(method) = item {
                if has_cfg_test(&method.attrs) {
                    continue;
                }
                self.record(&format!("{name}::{}", method.sig.ident), &method.attrs);
            }
        }
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        // Methods carry their own directives; attribute them with `Owner::method`
        // to mirror the method-symbol naming used elsewhere in the graph. Both
        // inherent and trait impls are covered.
        let owner = type_to_string(&node.self_ty);
        for item in &node.items {
            if let syn::ImplItem::Fn(method) = item {
                if has_cfg_test(&method.attrs) {
                    continue;
                }
                self.record(&format!("{owner}::{}", method.sig.ident), &method.attrs);
            }
        }
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let name = node.ident.to_string();
        if name == "tests" || has_cfg_test(&node.attrs) {
            return;
        }
        self.record(&name, &node.attrs);
        // Descend so nested items (impls, fns, structs) inside the module are seen.
        syn::visit::visit_item_mod(self, node);
    }
}

/// Extract compiler directives from an already-parsed file.
fn collect_directives(syntax_tree: &syn::File) -> Vec<DiscoveredDirective> {
    let mut visitor = DirectiveVisitor { directives: vec![] };
    visitor.visit_file(syntax_tree);
    visitor.directives
}

struct StructMethodVisitor {
    structs: Vec<DiscoveredStruct>,
    enums: Vec<DiscoveredEnum>,
    methods: Vec<DiscoveredMethod>,
    modules: Vec<DiscoveredModule>,
    traits: Vec<DiscoveredTrait>,
    trait_impls: Vec<(String, String)>, // (type_name, trait_name)
    file_path: String,
}

impl<'ast> Visit<'ast> for StructMethodVisitor {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if !is_public(&node.vis) || has_cfg_test(&node.attrs) {
            return;
        }

        let name = node.ident.to_string();

        let fields = match &node.fields {
            syn::Fields::Named(named) => named
                .named
                .iter()
                .filter_map(|f| {
                    let field_name = f.ident.as_ref()?.to_string();
                    let field_type = type_to_string(&f.ty);
                    Some(Field {
                        name: field_name,
                        field_type,
                        required: !is_option_type(&f.ty),
                        description: String::new(),
                    })
                })
                .collect(),
            // Tuple structs (newtypes like `struct UserId(Uuid)`) are the most
            // common Rust value-object idiom — preserve their inner types as
            // positional fields ("0", "1", …) rather than discarding them.
            syn::Fields::Unnamed(unnamed) => unnamed
                .unnamed
                .iter()
                .enumerate()
                .map(|(i, f)| Field {
                    name: i.to_string(),
                    field_type: type_to_string(&f.ty),
                    required: !is_option_type(&f.ty),
                    description: String::new(),
                })
                .collect(),
            // A unit struct genuinely has no fields (marker type).
            syn::Fields::Unit => vec![],
        };

        self.structs.push(DiscoveredStruct {
            name,
            start_line: node.span().start().line,
            end_line: node.span().end().line,
            fields,
            file_path: self.file_path.clone(),
            extends: vec![],
            implements: vec![],
        });

        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if !is_public(&node.vis) || has_cfg_test(&node.attrs) {
            return;
        }

        let name = node.ident.to_string();

        let variants = node
            .variants
            .iter()
            .map(|v| {
                let variant_name = v.ident.to_string();
                let variant_type = match &v.fields {
                    syn::Fields::Unit => "()".to_string(),
                    syn::Fields::Unnamed(u) => {
                        let types: Vec<String> =
                            u.unnamed.iter().map(|f| type_to_string(&f.ty)).collect();
                        types.join(", ")
                    }
                    syn::Fields::Named(n) => {
                        let parts: Vec<String> = n
                            .named
                            .iter()
                            .filter_map(|f| {
                                let fname = f.ident.as_ref()?.to_string();
                                Some(format!("{}: {}", fname, type_to_string(&f.ty)))
                            })
                            .collect();
                        format!("{{ {} }}", parts.join(", "))
                    }
                };
                Field {
                    name: variant_name,
                    field_type: variant_type,
                    required: true,
                    description: String::new(),
                }
            })
            .collect();

        self.enums.push(DiscoveredEnum {
            name,
            start_line: node.span().start().line,
            end_line: node.span().end().line,
            variants,
            file_path: self.file_path.clone(),
            extends: vec![],
            implements: vec![],
        });

        syn::visit::visit_item_enum(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let name = node.ident.to_string();
        if name == "tests" || has_cfg_test(&node.attrs) {
            return;
        }
        self.modules.push(DiscoveredModule {
            name,
            public: is_public(&node.vis),
            file_path: self.file_path.clone(),
            extends: vec![],
            implements: vec![],
        });
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        if !is_public(&node.vis) || has_cfg_test(&node.attrs) {
            return;
        }

        let name = node.ident.to_string();

        // Supertraits: `trait Foo: Bar + Baz` → ["Bar", "Baz"].
        let supertraits = node
            .supertraits
            .iter()
            .filter_map(|bound| match bound {
                syn::TypeParamBound::Trait(t) => Some(
                    t.path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::"),
                ),
                _ => None,
            })
            .collect();

        // The trait's declared method signatures are the port's operations.
        let methods = node
            .items
            .iter()
            .filter_map(|item| {
                let syn::TraitItem::Fn(method) = item else {
                    return None;
                };
                let return_type = match &method.sig.output {
                    syn::ReturnType::Default => String::new(),
                    syn::ReturnType::Type(_, ty) => type_to_string(ty),
                };
                let parameters = method
                    .sig
                    .inputs
                    .iter()
                    .filter_map(|arg| match arg {
                        syn::FnArg::Typed(pat_type) => {
                            let param_name = match pat_type.pat.as_ref() {
                                syn::Pat::Ident(ident) => ident.ident.to_string(),
                                _ => return None,
                            };
                            Some(Field {
                                name: param_name,
                                field_type: type_to_string(&pat_type.ty),
                                required: true,
                                description: String::new(),
                            })
                        }
                        syn::FnArg::Receiver(_) => None,
                    })
                    .collect();
                Some(DiscoveredMethod {
                    owner: name.clone(),
                    name: method.sig.ident.to_string(),
                    start_line: method.sig.span().start().line,
                    end_line: method.sig.span().end().line,
                    parameters,
                    return_type,
                    file_path: self.file_path.clone(),
                    extends: vec![],
                    implements: vec![],
                })
            })
            .collect();

        self.traits.push(DiscoveredTrait {
            name,
            start_line: node.span().start().line,
            end_line: node.span().end().line,
            methods,
            supertraits,
            file_path: self.file_path.clone(),
        });

        syn::visit::visit_item_trait(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if has_cfg_test(&node.attrs) {
            return;
        }

        let owner = type_to_string(&node.self_ty);

        // Record trait impl relationship
        if let Some((_, ref trait_path, _)) = node.trait_ {
            let trait_name = trait_path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            self.trait_impls.push((owner.clone(), trait_name));
        }

        // Skip method extraction for trait impls (they mirror the trait definition)
        if node.trait_.is_some() {
            syn::visit::visit_item_impl(self, node);
            return;
        }

        for item in &node.items {
            if let syn::ImplItem::Fn(method) = item {
                if !is_public(&method.vis) || has_cfg_test(&method.attrs) {
                    continue;
                }

                let name = method.sig.ident.to_string();
                let return_type = match &method.sig.output {
                    syn::ReturnType::Default => String::new(),
                    syn::ReturnType::Type(_, ty) => type_to_string(ty),
                };

                let parameters: Vec<Field> = method
                    .sig
                    .inputs
                    .iter()
                    .filter_map(|arg| match arg {
                        syn::FnArg::Typed(pat_type) => {
                            let param_name = match pat_type.pat.as_ref() {
                                syn::Pat::Ident(ident) => ident.ident.to_string(),
                                _ => {
                                    tracing::warn!("Unrecognized param pattern in method {}::{}, skipping param", owner, name);
                                    return None;
                                }
                            };
                            let param_type = type_to_string(&pat_type.ty);
                            Some(Field {
                                name: param_name,
                                field_type: param_type,
                                required: true,
                                description: String::new(),
                            })
                        }
                        syn::FnArg::Receiver(_) => None, // skip &self
                    })
                    .collect();

                self.methods.push(DiscoveredMethod {
                    owner: owner.clone(),
                    name,
                    start_line: method.span().start().line,
                    end_line: method.span().end().line,
                    parameters,
                    return_type,
                    file_path: self.file_path.clone(),
                    extends: vec![],
                    implements: vec![],
                });
            }
        }

        syn::visit::visit_item_impl(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ast::DiscoveredDirective;
    use crate::domain::scanner::AstScanner;

    fn scan(source: &str) -> super::super::ast::FileScan {
        RustSynScanner
            .scan_source(Path::new("test.rs"), source)
            .expect("scan should succeed")
    }

    /// Directive strings recorded for a given owner.
    fn directives_for<'a>(ds: &'a [DiscoveredDirective], owner: &str) -> Vec<&'a str> {
        ds.iter()
            .filter(|d| d.owner == owner)
            .map(|d| d.directive.as_str())
            .collect()
    }

    #[test]
    fn lint_directives_expand_per_lint_across_all_levels() {
        let scan = scan(
            r#"
            #[allow(dead_code, unused)]
            #[deny(missing_docs)]
            pub struct Widget {
                pub id: u32,
            }
            "#,
        );
        let ds = directives_for(&scan.directives, "Widget");
        assert!(ds.contains(&"allow(dead_code)"));
        assert!(ds.contains(&"allow(unused)"));
        assert!(ds.contains(&"deny(missing_docs)"));
    }

    #[test]
    fn lint_reason_value_is_skipped_not_captured() {
        // Rust 1.81+ lint reasons carry a `reason = "..."` value; only the
        // lint name should become a directive.
        let scan = scan(
            r#"
            #[allow(dead_code, reason = "kept for the public API")]
            pub struct Kept;
            "#,
        );
        assert_eq!(
            directives_for(&scan.directives, "Kept"),
            vec!["allow(dead_code)"]
        );
    }

    #[test]
    fn cfg_predicate_is_captured_verbatim() {
        let scan = scan(
            r#"
            #[cfg(feature = "async")]
            pub struct AsyncOnly;
            "#,
        );
        assert!(
            directives_for(&scan.directives, "AsyncOnly")
                .iter()
                .any(|d| d.starts_with("cfg(") && d.contains("async"))
        );
    }

    #[test]
    fn cfg_test_items_are_excluded_from_production_scan() {
        let scan = scan(
            r#"
            pub trait Port {}
            pub struct Adapter;

            impl Adapter {
                pub fn live(&self) {}
            }

            #[cfg(any(test, feature = "fixtures"))]
            pub struct TestOnly {
                pub id: u64,
            }

            #[cfg(test)]
            impl Adapter {
                pub fn helper(&self) { self.danger(); }
                pub fn danger(&self) {}
            }

            #[cfg(test)]
            impl Port for Adapter {}
            "#,
        );

        let structs: Vec<&str> = scan.structs.iter().map(|s| s.name.as_str()).collect();
        assert!(structs.contains(&"Adapter"));
        assert!(
            !structs.contains(&"TestOnly"),
            "cfg(test) struct leaked: {structs:?}"
        );

        let methods: Vec<String> = scan
            .methods
            .iter()
            .map(|m| format!("{}::{}", m.owner, m.name))
            .collect();
        assert!(methods.contains(&"Adapter::live".to_string()));
        assert!(
            !methods
                .iter()
                .any(|m| m == "Adapter::helper" || m == "Adapter::danger"),
            "cfg(test) impl method leaked: {methods:?}"
        );

        let adapter = scan
            .structs
            .iter()
            .find(|s| s.name == "Adapter")
            .expect("Adapter struct");
        assert!(
            adapter.implements.is_empty(),
            "cfg(test) trait impl leaked: {:?}",
            adapter.implements
        );

        let pairs = call_pairs(&scan);
        assert!(
            !pairs.contains(&("Adapter::helper".into(), "Adapter::danger".into())),
            "cfg(test) impl call leaked: {pairs:?}"
        );
    }

    #[test]
    fn code_references_capture_types_bounds_and_qualified_paths() {
        let scan = scan(
            r#"
            pub trait Port: crate::domain::ports::BasePort {}

            pub struct Handler<T: crate::domain::ports::Port> {
                repo: crate::store::CozoStore,
                payload: T,
            }

            impl Handler<crate::store::CozoStore> {
                pub fn load(&self) -> crate::domain::model::DomainModel {
                    crate::server::daemon::start();
                    crate::domain::model::DomainModel::default()
                }
            }
            "#,
        );

        let refs: Vec<(&str, &str)> = scan
            .references
            .iter()
            .map(|reference| {
                (
                    reference.to_path.as_str(),
                    reference.reference_kind.as_str(),
                )
            })
            .collect();

        assert!(refs.contains(&("crate::domain::ports::BasePort", "trait_bound")));
        assert!(refs.contains(&("crate::domain::ports::Port", "trait_bound")));
        assert!(refs.contains(&("crate::store::CozoStore", "type")));
        assert!(refs.contains(&("crate::domain::model::DomainModel", "type")));
        assert!(refs.contains(&("crate::server::daemon::start", "expr_path")));
    }

    #[test]
    fn dead_code_is_captured_on_functions_methods_traits_and_private_items() {
        // The whole point: `dead_code` lives mostly on behavioural and *private*
        // symbols, so a single substring scan for "dead_code" must reach them all
        // regardless of visibility.
        let scan = scan(
            r#"
            #[allow(dead_code)]
            fn orphan() {}

            #[allow(dead_code)]
            pub trait Port {
                #[allow(dead_code)]
                fn op(&self);
            }

            #[allow(dead_code)]
            struct Hidden;
            impl Svc {
                #[allow(dead_code)]
                fn helper(&self) {}
            }
            "#,
        );

        let owners_with_dead_code: Vec<&str> = scan
            .directives
            .iter()
            .filter(|d| d.directive.contains("dead_code"))
            .map(|d| d.owner.as_str())
            .collect();

        assert!(owners_with_dead_code.contains(&"orphan"), "private free fn");
        assert!(owners_with_dead_code.contains(&"Port"), "trait");
        assert!(owners_with_dead_code.contains(&"Port::op"), "trait method");
        assert!(owners_with_dead_code.contains(&"Hidden"), "private struct");
        assert!(
            owners_with_dead_code.contains(&"Svc::helper"),
            "private impl method"
        );
    }

    #[test]
    fn field_level_directives_are_captured_with_owner_dot_field() {
        let scan = scan(
            r#"
            pub struct JsonRpcRequest {
                #[allow(dead_code)]
                pub jsonrpc: String,
                pub id: u32,
            }
            "#,
        );
        assert_eq!(
            directives_for(&scan.directives, "JsonRpcRequest.jsonrpc"),
            vec!["allow(dead_code)"]
        );
    }

    #[test]
    fn directives_carry_their_source_line() {
        // Line 1 is the empty line after the raw-string quote.
        let scan = scan("\n#[allow(dead_code)]\nfn orphan() {}\n");
        let d = scan
            .directives
            .iter()
            .find(|d| d.owner == "orphan")
            .expect("orphan directive");
        assert_eq!(d.directive, "allow(dead_code)");
        assert_eq!(d.line, 2, "attribute is on the second line");
    }

    #[test]
    fn derives_remain_individual_and_doc_is_dropped() {
        let scan = scan(
            r#"
            /// docs here
            #[derive(Debug, Clone)]
            pub struct Plain {
                pub n: u8,
            }
            "#,
        );
        let ds = directives_for(&scan.directives, "Plain");
        assert!(ds.contains(&"Debug"));
        assert!(ds.contains(&"Clone"));
        assert!(!ds.contains(&"doc"));
    }

    // ── Call graph ──────────────────────────────────────────────────────────

    /// (caller, callee) pairs collected from the scan.
    fn call_pairs(scan: &super::super::ast::FileScan) -> Vec<(String, String)> {
        scan.calls
            .iter()
            .map(|c| (c.caller.clone(), c.callee.clone()))
            .collect()
    }

    #[test]
    fn self_method_call_resolves_to_owner_method() {
        let scan = scan(
            r#"
            pub struct Store;
            impl Store {
                pub fn save_desired(&self) { self.save_state(); }
                pub fn save_state(&self) {}
            }
            "#,
        );
        let pairs = call_pairs(&scan);
        assert!(
            pairs.contains(&("Store::save_desired".into(), "Store::save_state".into())),
            "self.save_state() should resolve to Store::save_state: {pairs:?}"
        );
    }

    #[test]
    fn self_assoc_call_resolves_to_owner() {
        let scan = scan(
            r#"
            pub struct Widget;
            impl Widget {
                pub fn make() -> Self { Self::new() }
                pub fn new() -> Self { Widget }
            }
            "#,
        );
        let pairs = call_pairs(&scan);
        assert!(
            pairs.contains(&("Widget::make".into(), "Widget::new".into())),
            "Self::new() should resolve to Widget::new: {pairs:?}"
        );
    }

    #[test]
    fn trait_impl_method_bodies_are_collected() {
        // Trait impls were previously skipped entirely for call extraction.
        let scan = scan(
            r#"
            pub trait Repo { fn save(&self); }
            pub struct SqlStore;
            impl Repo for SqlStore {
                fn save(&self) { self.flush(); }
            }
            impl SqlStore { fn flush(&self) {} }
            "#,
        );
        let pairs = call_pairs(&scan);
        assert!(
            pairs.contains(&("SqlStore::save".into(), "SqlStore::flush".into())),
            "calls inside trait-impl bodies must be collected & self-resolved: {pairs:?}"
        );
    }

    #[test]
    fn calls_inside_inline_modules_are_collected() {
        let scan = scan(
            r#"
            pub mod inner {
                pub fn helper() {}
                pub fn run() { helper(); }
            }
            "#,
        );
        let pairs = call_pairs(&scan);
        assert!(
            pairs.contains(&("run".into(), "helper".into())),
            "calls nested in inline modules must be collected: {pairs:?}"
        );
    }

    #[test]
    fn non_self_method_calls_stay_name_only() {
        // We cannot resolve a receiver's type without inference, so a call on a
        // field/local stays unqualified — documented as a known limitation.
        let scan = scan(
            r#"
            pub struct Svc;
            impl Svc {
                pub fn go(&self) { self.dep.execute(); }
            }
            "#,
        );
        let pairs = call_pairs(&scan);
        assert!(
            pairs.contains(&("Svc::go".into(), "execute".into())),
            "self.dep.execute() is name-only (receiver type unknown): {pairs:?}"
        );
        assert!(
            !pairs.iter().any(|(_, callee)| callee == "Svc::execute"),
            "must not falsely resolve a non-self receiver to the owner"
        );
    }
}
