use anyhow::{Context, Result};
use std::path::Path;
use syn::spanned::Spanned;
use syn::visit::Visit;

use super::analyze::{
    CallInfo, DiscoveredEnum, DiscoveredMethod, DiscoveredModule, DiscoveredStruct, LiveDependency,
    ScanResult,
};
use super::model::Field;
use super::scanner::AstScanner;

pub struct RustSynScanner;

impl AstScanner for RustSynScanner {
    fn extract_live_dependencies(
        &self,
        file_path: &Path,
        source_code: &str,
    ) -> Result<Vec<LiveDependency>> {
        let syntax_tree = syn::parse_file(source_code)
            .with_context(|| format!("Failed to parse rust file: {}", file_path.display()))?;

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

        let mut visitor = ImportVisitor { imports: vec![] };
        visitor.visit_file(&syntax_tree);

        let from_file = file_path.to_string_lossy().to_string();
        let deps = visitor
            .imports
            .into_iter()
            .map(|to_module| LiveDependency {
                from_file: from_file.clone(),
                to_module,
            })
            .collect();

        Ok(deps)
    }

    fn scan_file(&self, file_path: &Path, source_code: &str) -> Result<ScanResult> {
        let syntax_tree = syn::parse_file(source_code)
            .with_context(|| format!("Failed to parse: {}", file_path.display()))?;

        let mut visitor = StructMethodVisitor {
            structs: vec![],
            enums: vec![],
            methods: vec![],
            modules: vec![],
            trait_impls: vec![],
            file_path: file_path.to_string_lossy().to_string(),
        };
        visitor.visit_file(&syntax_tree);

        // Backfill implements on structs from trait impls
        for (type_name, trait_name) in &visitor.trait_impls {
            for s in &mut visitor.structs {
                if s.name == *type_name {
                    s.implements.push(trait_name.clone());
                }
            }
            for e in &mut visitor.enums {
                if e.name == *type_name {
                    e.implements.push(trait_name.clone());
                }
            }
        }

        Ok((
            visitor.structs,
            visitor.enums,
            visitor.methods,
            visitor.modules,
        ))
    }

    fn extract_calls(&self, file_path: &Path, source_code: &str) -> Result<Vec<CallInfo>> {
        let syntax_tree = syn::parse_file(source_code)
            .with_context(|| format!("Failed to parse: {}", file_path.display()))?;

        let mut calls = Vec::new();
        for item in &syntax_tree.items {
            if let syn::Item::Impl(imp) = item {
                let owner = type_to_string(&imp.self_ty);
                // Skip trait impls — they mirror trait definitions
                if imp.trait_.is_some() {
                    continue;
                }
                for impl_item in &imp.items {
                    if let syn::ImplItem::Fn(method) = impl_item {
                        let caller = format!("{}::{}", owner, method.sig.ident);
                        collect_calls_from_block(&method.block, &caller, &mut calls);
                    }
                }
            } else if let syn::Item::Fn(func) = item {
                let caller = func.sig.ident.to_string();
                collect_calls_from_block(&func.block, &caller, &mut calls);
            }
        }

        Ok(calls)
    }
}

/// Recursively walk a block's expressions collecting call sites.
fn collect_calls_from_block(block: &syn::Block, caller: &str, calls: &mut Vec<CallInfo>) {
    for stmt in &block.stmts {
        match stmt {
            syn::Stmt::Expr(expr, _) => {
                collect_calls_from_expr(expr, caller, calls);
            }
            syn::Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    collect_calls_from_expr(&init.expr, caller, calls);
                }
            }
            _ => {}
        }
    }
}

fn collect_calls_from_expr(expr: &syn::Expr, caller: &str, calls: &mut Vec<CallInfo>) {
    match expr {
        syn::Expr::Call(call) => {
            let callee = match call.func.as_ref() {
                syn::Expr::Path(path) => path
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::"),
                _ => return,
            };
            calls.push(CallInfo {
                caller: caller.to_string(),
                callee,
                line: call.paren_token.span.open().start().line,
            });
            for arg in &call.args {
                collect_calls_from_expr(arg, caller, calls);
            }
        }
        syn::Expr::MethodCall(mc) => {
            calls.push(CallInfo {
                caller: caller.to_string(),
                callee: mc.method.to_string(),
                line: mc.method.span().start().line,
            });
            collect_calls_from_expr(&mc.receiver, caller, calls);
            for arg in &mc.args {
                collect_calls_from_expr(arg, caller, calls);
            }
        }
        syn::Expr::Block(b) => collect_calls_from_block(&b.block, caller, calls),
        syn::Expr::If(i) => {
            collect_calls_from_expr(&i.cond, caller, calls);
            collect_calls_from_block(&i.then_branch, caller, calls);
            if let Some((_, else_branch)) = &i.else_branch {
                collect_calls_from_expr(else_branch, caller, calls);
            }
        }
        syn::Expr::Match(m) => {
            collect_calls_from_expr(&m.expr, caller, calls);
            for arm in &m.arms {
                collect_calls_from_expr(&arm.body, caller, calls);
            }
        }
        syn::Expr::Closure(c) => {
            collect_calls_from_expr(&c.body, caller, calls);
        }
        syn::Expr::Return(r) => {
            if let Some(e) = &r.expr {
                collect_calls_from_expr(e, caller, calls);
            }
        }
        syn::Expr::Try(t) => collect_calls_from_expr(&t.expr, caller, calls),
        syn::Expr::Paren(p) => collect_calls_from_expr(&p.expr, caller, calls),
        syn::Expr::Reference(r) => collect_calls_from_expr(&r.expr, caller, calls),
        syn::Expr::Unary(u) => collect_calls_from_expr(&u.expr, caller, calls),
        syn::Expr::Binary(b) => {
            collect_calls_from_expr(&b.left, caller, calls);
            collect_calls_from_expr(&b.right, caller, calls);
        }
        syn::Expr::Let(l) => collect_calls_from_expr(&l.expr, caller, calls),
        syn::Expr::Tuple(t) => {
            for e in &t.elems {
                collect_calls_from_expr(e, caller, calls);
            }
        }
        syn::Expr::Array(a) => {
            for e in &a.elems {
                collect_calls_from_expr(e, caller, calls);
            }
        }
        syn::Expr::Field(f) => collect_calls_from_expr(&f.base, caller, calls),
        syn::Expr::Index(i) => {
            collect_calls_from_expr(&i.expr, caller, calls);
            collect_calls_from_expr(&i.index, caller, calls);
        }
        syn::Expr::Await(a) => collect_calls_from_expr(&a.base, caller, calls),
        syn::Expr::Unsafe(u) => collect_calls_from_block(&u.block, caller, calls),
        syn::Expr::Loop(l) => collect_calls_from_block(&l.body, caller, calls),
        syn::Expr::While(w) => {
            collect_calls_from_expr(&w.cond, caller, calls);
            collect_calls_from_block(&w.body, caller, calls);
        }
        syn::Expr::ForLoop(f) => {
            collect_calls_from_expr(&f.expr, caller, calls);
            collect_calls_from_block(&f.body, caller, calls);
        }
        syn::Expr::Struct(s) => {
            for field in &s.fields {
                collect_calls_from_expr(&field.expr, caller, calls);
            }
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
        attr.parse_args::<syn::Ident>()
            .map(|ident| ident == "test")
            .unwrap_or(false)
    })
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
            let prev_is_lifetime = result.chars().last().map_or(false, |c| c.is_alphanumeric());
            let next_is_ident = chars
                .peek()
                .map_or(false, |c| c.is_alphabetic() || *c == '_');
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

/// Extract derive macros and other proc-macro attributes as decorator names.
fn extract_decorators(attrs: &[syn::Attribute]) -> Vec<String> {
    let mut decorators = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("derive") {
            // Parse derive(A, B, C) into individual decorator names
            let _ = attr.parse_nested_meta(|meta| {
                let path = meta
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::");
                decorators.push(path);
                Ok(())
            });
        } else {
            // Other attributes like #[serde(...)], #[tokio::main], etc.
            let path = attr
                .path()
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            if !path.is_empty() && path != "doc" && path != "cfg" && path != "allow" {
                decorators.push(path);
            }
        }
    }
    decorators
}

struct StructMethodVisitor {
    structs: Vec<DiscoveredStruct>,
    enums: Vec<DiscoveredEnum>,
    methods: Vec<DiscoveredMethod>,
    modules: Vec<DiscoveredModule>,
    trait_impls: Vec<(String, String)>, // (type_name, trait_name)
    file_path: String,
}

impl<'ast> Visit<'ast> for StructMethodVisitor {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if !is_public(&node.vis) {
            return;
        }

        let name = node.ident.to_string();

        // Extract derive macros and other attributes as decorators
        let decorators = extract_decorators(&node.attrs);

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
            syn::Fields::Unnamed(_) => {
                tracing::warn!(
                    "Tuple struct {} encountered in {}, tuple structs are currently mapped as empty-field records",
                    name,
                    self.file_path
                );
                vec![]
            }
            syn::Fields::Unit => {
                tracing::warn!(
                    "Unit struct {} encountered in {}, mapping as zero-field record",
                    name,
                    self.file_path
                );
                vec![]
            }
        };

        self.structs.push(DiscoveredStruct {
            name,
            start_line: node.span().start().line,
            end_line: node.span().end().line,
            fields,
            file_path: self.file_path.clone(),
            extends: vec![],
            implements: vec![],
            decorators,
        });

        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if !is_public(&node.vis) || has_cfg_test(&node.attrs) {
            return;
        }

        let name = node.ident.to_string();

        // Extract derive macros and other attributes as decorators
        let decorators = extract_decorators(&node.attrs);
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
            decorators,
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
            decorators: vec![],
        });
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
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
                if !is_public(&method.vis) {
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
                    decorators: vec![],
                });
            }
        }

        syn::visit::visit_item_impl(self, node);
    }
}
