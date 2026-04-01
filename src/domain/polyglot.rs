use anyhow::{Context, Result};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator};

use super::analyze::{CallInfo, DiscoveredMethod, DiscoveredStruct, LiveDependency, ScanResult};
use super::model::Field;
use super::scanner::AstScanner;

/// Identifies which language family a file belongs to for query selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LangFamily {
    TypeScript,
    Python,
    Go,
    Java,
}

/// A polyglot scanner using Tree-Sitter to parse non-Rust files.
pub struct TreeSitterScanner;

impl Default for TreeSitterScanner {
    fn default() -> Self {
        Self
    }
}

impl TreeSitterScanner {
    pub fn new() -> Self {
        Self
    }

    fn get_language(&self, file_path: &Path) -> Option<(Language, LangFamily)> {
        let ext = file_path.extension()?.to_str()?;
        match ext {
            "ts" => Some((
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                LangFamily::TypeScript,
            )),
            "tsx" => Some((
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                LangFamily::TypeScript,
            )),
            "py" => Some((tree_sitter_python::LANGUAGE.into(), LangFamily::Python)),
            "go" => Some((tree_sitter_go::LANGUAGE.into(), LangFamily::Go)),
            "java" => Some((tree_sitter_java::LANGUAGE.into(), LangFamily::Java)),
            _ => None,
        }
    }

    fn parse_tree(
        &self,
        file_path: &Path,
        source_code: &str,
    ) -> Result<Option<(tree_sitter::Tree, LangFamily)>> {
        let (language, family) = match self.get_language(file_path) {
            Some(pair) => pair,
            None => {
                tracing::debug!(
                    "Unsupported file extension for tree-sitter: {:?}",
                    file_path
                );
                return Ok(None);
            }
        };

        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .map_err(|e| anyhow::anyhow!("Failed to set tree-sitter language: {:?}", e))?;

        let tree = parser
            .parse(source_code, None)
            .context("Failed to parse code with tree-sitter")?;

        Ok(Some((tree, family)))
    }
}

// ─── Helper: extract node text ─────────────────────────────────────────────

fn node_text<'a>(node: Node<'a>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

/// Iterate children of a node (tree-sitter 0.26 uses u32 indices).
fn children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> + '_ {
    (0..node.child_count() as u32).filter_map(move |i| node.child(i))
}

// ─── Python extraction ─────────────────────────────────────────────────────

fn python_extract_imports(source: &str, tree: &tree_sitter::Tree) -> Vec<String> {
    // Match `import X` and `from X import Y`
    let query_src = r#"
        (import_statement
          name: (dotted_name) @module)
        (import_from_statement
          module_name: (dotted_name) @module)
    "#;

    let language = tree.language();
    let query = match Query::new(&language, query_src) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!("Failed to compile Python import query: {e}");
            return vec![];
        }
    };

    let mut cursor = QueryCursor::new();
    let root = tree.root_node();
    let mut imports = Vec::new();

    let mut matches = cursor.matches(&query, root, source.as_bytes());
    while let Some(m) = matches.next() {
        for cap in m.captures {
            imports.push(node_text(cap.node, source).to_string());
        }
    }

    imports
}

fn python_extract_classes(
    source: &str,
    tree: &tree_sitter::Tree,
    file_path: &str,
) -> (Vec<DiscoveredStruct>, Vec<DiscoveredMethod>) {
    let query_src = r#"
        (class_definition
          name: (identifier) @class_name
          body: (block) @class_body) @class_node
    "#;

    let language = tree.language();
    let query = match Query::new(&language, query_src) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!("Failed to compile Python class query: {e}");
            return (vec![], vec![]);
        }
    };

    let mut cursor = QueryCursor::new();
    let root = tree.root_node();
    let mut structs = Vec::new();
    let mut methods = Vec::new();

    let class_name_idx = query.capture_index_for_name("class_name").unwrap();
    let class_body_idx = query.capture_index_for_name("class_body").unwrap();
    let class_node_idx = query.capture_index_for_name("class_node").unwrap();

    let mut matches = cursor.matches(&query, root, source.as_bytes());
    while let Some(m) = matches.next() {
        let mut name = "";
        let mut body_node = None;
        let mut class_node = None;

        for cap in m.captures {
            if cap.index == class_name_idx {
                name = node_text(cap.node, source);
            } else if cap.index == class_body_idx {
                body_node = Some(cap.node);
            } else if cap.index == class_node_idx {
                class_node = Some(cap.node);
            }
        }

        if name.is_empty() {
            continue;
        }

        // Extract __init__ fields from type annotations in class body
        let fields = body_node
            .map(|body| python_extract_init_fields(source, body))
            .unwrap_or_default();

        let class_nd = match class_node {
            Some(n) => n,
            None => continue,
        };

        // Extract superclasses from argument_list child (e.g. class Foo(Bar, Baz):)
        let mut extends = Vec::new();
        if let Some(cn) = class_node {
            for child in children(cn) {
                if child.kind() == "argument_list" {
                    for arg in children(child) {
                        let kind = arg.kind();
                        if kind == "identifier" {
                            extends.push(node_text(arg, source).to_string());
                        } else if kind == "attribute" {
                            // e.g. module.ClassName
                            extends.push(node_text(arg, source).to_string());
                        } else if kind == "keyword_argument" {
                            // e.g. metaclass=ABCMeta — skip
                        }
                    }
                }
            }
        }

        // Extract decorators from decorator children (e.g. @dataclass)
        let mut decorators = Vec::new();
        if let Some(cn) = class_node {
            for child in children(cn) {
                if child.kind() == "decorator" {
                    // decorator node has an expression child (identifier or call)
                    if let Some(expr) = child.child(1) {
                        let dec_text = if expr.kind() == "call" {
                            // @decorator(args) — extract just the function name
                            expr.child_by_field_name("function")
                                .map(|f| node_text(f, source))
                                .unwrap_or_else(|| node_text(expr, source))
                        } else {
                            node_text(expr, source)
                        };
                        decorators.push(dec_text.to_string());
                    }
                }
            }
        }

        structs.push(DiscoveredStruct {
            name: name.to_string(),
            start_line: class_nd.start_position().row + 1,
            end_line: class_nd.end_position().row + 1,
            fields,
            file_path: file_path.to_string(),
            extends,
            implements: vec![],
            decorators,
        });

        // Extract methods from class body
        if let Some(body) = body_node {
            let class_methods = python_extract_methods(source, body, name, file_path);
            methods.extend(class_methods);
        }
    }

    (structs, methods)
}

/// Extracts fields from `self.x = ...` assignments in `__init__`, using type annotations if present.
fn python_extract_init_fields(source: &str, class_body: Node) -> Vec<Field> {
    let mut fields = Vec::new();

    // Walk the body for function_definition named __init__
    // then find `self.x` attribute assignments
    for child in children(class_body) {
        // Typed class-level field: `name: str = "default"` or `name: int`
        if child.kind() == "expression_statement"
            && let Some(assign) = child.child(0)
        {
            if assign.kind() == "assignment" {
                if let (Some(left), Some(right)) = (
                    assign.child_by_field_name("left"),
                    assign.child_by_field_name("type"),
                ) && left.kind() == "identifier"
                {
                    let field_name = node_text(left, source).to_string();
                    let field_type = node_text(right, source).to_string();
                    // Skip private/dunder unless it's a known pattern
                    if !field_name.starts_with('_') {
                        fields.push(Field {
                            name: field_name,
                            field_type,
                            required: true,
                            description: String::new(),
                        });
                    }
                }
            } else if assign.kind() == "type" {
                // bare type annotation: `name: str`
                if let (Some(ident), Some(ty)) = (
                    assign.child_by_field_name("identifier"),
                    assign.child_by_field_name("type"),
                ) {
                    let field_name = node_text(ident, source).to_string();
                    let field_type = node_text(ty, source).to_string();
                    if !field_name.starts_with('_') {
                        fields.push(Field {
                            name: field_name,
                            field_type,
                            required: true,
                            description: String::new(),
                        });
                    }
                }
            }
        }

        // Also scan __init__ for `self.x: type = ...` or `self.x = ...`
        if child.kind() == "function_definition" {
            let name_node = child.child_by_field_name("name");
            if name_node.map(|n| node_text(n, source)) == Some("__init__")
                && let Some(body) = child.child_by_field_name("body")
            {
                fields.extend(python_extract_self_assignments(source, body));
            }
        }
    }

    fields
}

fn python_extract_self_assignments(source: &str, init_body: Node) -> Vec<Field> {
    let mut fields = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for stmt in children(init_body) {
        if stmt.kind() != "expression_statement" {
            continue;
        }
        let assign = match stmt.child(0) {
            Some(a) if a.kind() == "assignment" => a,
            _ => continue,
        };
        let left = match assign.child_by_field_name("left") {
            Some(l) => l,
            None => continue,
        };
        if left.kind() != "attribute" {
            continue;
        }
        // Check it starts with `self.`
        let obj = match left.child_by_field_name("object") {
            Some(o) if node_text(o, source) == "self" => o,
            _ => continue,
        };
        let _ = obj;
        let attr = match left.child_by_field_name("attribute") {
            Some(a) => a,
            None => continue,
        };
        let field_name = node_text(attr, source).to_string();
        if field_name.starts_with('_') || !seen.insert(field_name.clone()) {
            continue;
        }

        // Try to get type annotation: `self.x: Type = ...`
        let field_type = assign
            .child_by_field_name("type")
            .map(|t| node_text(t, source).to_string())
            .unwrap_or_default();

        fields.push(Field {
            name: field_name,
            field_type,
            required: true,
            description: String::new(),
        });
    }

    fields
}

fn python_extract_methods(
    source: &str,
    class_body: Node,
    owner: &str,
    file_path: &str,
) -> Vec<DiscoveredMethod> {
    let mut methods = Vec::new();

    for child in children(class_body) {
        if child.kind() != "function_definition" {
            continue;
        }
        let name_node = match child.child_by_field_name("name") {
            Some(n) => n,
            None => continue,
        };
        let method_name = node_text(name_node, source);
        // Skip dunder methods except __init__ is already handled separately
        if method_name.starts_with("__") && method_name.ends_with("__") {
            continue;
        }
        // Skip private methods
        if method_name.starts_with('_') {
            continue;
        }

        let params = child
            .child_by_field_name("parameters")
            .map(|pnode| python_parse_params(source, pnode))
            .unwrap_or_default();

        let return_type = child
            .child_by_field_name("return_type")
            .map(|rt| node_text(rt, source).to_string())
            .unwrap_or_default();

        methods.push(DiscoveredMethod {
            owner: owner.to_string(),
            name: method_name.to_string(),
            start_line: child.start_position().row + 1,
            end_line: child.end_position().row + 1,
            parameters: params,
            return_type,
            file_path: file_path.to_string(),
            extends: vec![],
            implements: vec![],
            decorators: vec![],
        });
    }

    methods
}

fn python_parse_params(source: &str, params_node: Node) -> Vec<Field> {
    let mut fields = Vec::new();
    for param in children(params_node) {
        match param.kind() {
            "identifier" => {
                let name = node_text(param, source);
                if name == "self" || name == "cls" {
                    continue;
                }
                fields.push(Field {
                    name: name.to_string(),
                    field_type: String::new(),
                    required: true,
                    description: String::new(),
                });
            }
            "typed_parameter" => {
                let name = param
                    .child_by_field_name("name")
                    .map(|n| node_text(n, source))
                    .unwrap_or("");
                if name == "self" || name == "cls" {
                    continue;
                }
                let ty = param
                    .child_by_field_name("type")
                    .map(|t| node_text(t, source).to_string())
                    .unwrap_or_default();
                fields.push(Field {
                    name: name.to_string(),
                    field_type: ty,
                    required: true,
                    description: String::new(),
                });
            }
            "typed_default_parameter" | "default_parameter" => {
                let name = param
                    .child_by_field_name("name")
                    .map(|n| node_text(n, source))
                    .unwrap_or("");
                if name == "self" || name == "cls" {
                    continue;
                }
                let ty = param
                    .child_by_field_name("type")
                    .map(|t| node_text(t, source).to_string())
                    .unwrap_or_default();
                fields.push(Field {
                    name: name.to_string(),
                    field_type: ty,
                    required: false,
                    description: String::new(),
                });
            }
            _ => {}
        }
    }
    fields
}

// ─── TypeScript/TSX extraction ─────────────────────────────────────────────

fn ts_extract_imports(source: &str, tree: &tree_sitter::Tree) -> Vec<String> {
    let query_src = r#"
        (import_statement
          source: (string) @module)
    "#;

    let language = tree.language();
    let query = match Query::new(&language, query_src) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!("Failed to compile TS import query: {e}");
            return vec![];
        }
    };

    let mut cursor = QueryCursor::new();
    let root = tree.root_node();
    let mut imports = Vec::new();

    let mut matches = cursor.matches(&query, root, source.as_bytes());
    while let Some(m) = matches.next() {
        for cap in m.captures {
            // Strip quotes from the string literal
            let raw = node_text(cap.node, source);
            let cleaned = raw.trim_matches(|c| c == '\'' || c == '"');
            imports.push(cleaned.to_string());
        }
    }

    imports
}

fn ts_extract_classes(
    source: &str,
    tree: &tree_sitter::Tree,
    file_path: &str,
) -> (Vec<DiscoveredStruct>, Vec<DiscoveredMethod>) {
    let mut structs = Vec::new();
    let mut methods = Vec::new();

    // Extract classes
    let class_query_src = r#"
        (class_declaration
          name: (type_identifier) @class_name
          body: (class_body) @class_body) @class_node
    "#;

    let language = tree.language();
    let query = match Query::new(&language, class_query_src) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!("Failed to compile TS class query: {e}");
            return (vec![], vec![]);
        }
    };

    let class_name_idx = query.capture_index_for_name("class_name").unwrap();
    let class_body_idx = query.capture_index_for_name("class_body").unwrap();
    let class_node_idx = query.capture_index_for_name("class_node").unwrap();

    let mut cursor = QueryCursor::new();
    let root = tree.root_node();

    let mut matches = cursor.matches(&query, root, source.as_bytes());
    while let Some(m) = matches.next() {
        let mut name = "";
        let mut body_node = None;
        let mut class_node = None;

        for cap in m.captures {
            if cap.index == class_name_idx {
                name = node_text(cap.node, source);
            } else if cap.index == class_body_idx {
                body_node = Some(cap.node);
            } else if cap.index == class_node_idx {
                class_node = Some(cap.node);
            }
        }

        if name.is_empty() {
            continue;
        }

        let body = match body_node {
            Some(b) => b,
            None => continue,
        };

        let class_nd = match class_node {
            Some(n) => n,
            None => continue,
        };

        let fields = ts_extract_class_fields(source, body);

        // Extract heritage clauses (extends/implements) from class node
        let mut extends = Vec::new();
        let mut implements = Vec::new();
        for child in children(class_nd) {
            if child.kind() == "class_heritage" {
                for heritage in children(child) {
                    if heritage.kind() == "extends_clause" {
                        for type_node in children(heritage) {
                            if type_node.kind() != "extends" {
                                // Could be a type_identifier or generic_type
                                let text = node_text(type_node, source).trim().to_string();
                                if !text.is_empty() {
                                    extends.push(text);
                                }
                            }
                        }
                    } else if heritage.kind() == "implements_clause" {
                        for type_node in children(heritage) {
                            if type_node.kind() != "implements" {
                                let text = node_text(type_node, source).trim().to_string();
                                if !text.is_empty() {
                                    implements.push(text);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Extract decorators
        let mut decorators = Vec::new();
        if let Some(cn) = class_node {
            // In TS, decorators are siblings before the class or children of a decorated declaration
            if let Some(parent) = cn.parent() {
                if parent.kind() == "export_statement" || parent.kind() == "program" {
                    // Look for decorator siblings before this node
                    let idx = cn.start_position();
                    if let Some(pp) = cn.parent() {
                        for sib in children(pp) {
                            if sib.start_position() >= idx {
                                break;
                            }
                            if sib.kind() == "decorator" {
                                if let Some(expr) = sib.child(1) {
                                    let dec_text = if expr.kind() == "call_expression" {
                                        expr.child_by_field_name("function")
                                            .map(|f| node_text(f, source))
                                            .unwrap_or_else(|| node_text(expr, source))
                                    } else {
                                        node_text(expr, source)
                                    };
                                    decorators.push(dec_text.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        structs.push(DiscoveredStruct {
            name: name.to_string(),
            start_line: class_nd.start_position().row + 1,
            end_line: class_nd.end_position().row + 1,
            fields,
            file_path: file_path.to_string(),
            extends,
            implements,
            decorators,
        });

        let class_methods = ts_extract_class_methods(source, body, name, file_path);
        methods.extend(class_methods);
    }

    // Also extract interfaces as structs
    let iface_query_src = r#"
        (interface_declaration
          name: (type_identifier) @iface_name
          body: (interface_body) @iface_body) @iface_node
    "#;

    if let Ok(iface_query) = Query::new(&language, iface_query_src) {
        let iface_name_idx = iface_query.capture_index_for_name("iface_name").unwrap();
        let iface_body_idx = iface_query.capture_index_for_name("iface_body").unwrap();
        let iface_node_idx = iface_query.capture_index_for_name("iface_node").unwrap();
        let mut cursor2 = QueryCursor::new();

        let mut iface_matches = cursor2.matches(&iface_query, root, source.as_bytes());
        while let Some(m) = iface_matches.next() {
            let mut name = "";
            let mut body_node = None;
            let mut iface_node = None;

            for cap in m.captures {
                if cap.index == iface_name_idx {
                    name = node_text(cap.node, source);
                } else if cap.index == iface_body_idx {
                    body_node = Some(cap.node);
                } else if cap.index == iface_node_idx {
                    iface_node = Some(cap.node);
                }
            }

            if name.is_empty() {
                continue;
            }

            let fields = body_node
                .map(|b| ts_extract_interface_fields(source, b))
                .unwrap_or_default();

            // Extract extends from interface heritage (interfaces can extend other interfaces)
            let mut iface_extends = Vec::new();
            if let Some(inode) = iface_node {
                for child in children(inode) {
                    if child.kind() == "extends_type_clause" {
                        for type_node in children(child) {
                            let kind = type_node.kind();
                            if kind == "type_identifier"
                                || kind == "generic_type"
                                || kind == "nested_type_identifier"
                            {
                                iface_extends.push(node_text(type_node, source).to_string());
                            }
                        }
                    }
                }
            }

            let nd = iface_node.unwrap_or_else(|| body_node.unwrap());
            structs.push(DiscoveredStruct {
                name: name.to_string(),
                start_line: nd.start_position().row + 1,
                end_line: nd.end_position().row + 1,
                fields,
                file_path: file_path.to_string(),
                extends: iface_extends,
                implements: vec![],
                decorators: vec![],
            });
        }
    }

    (structs, methods)
}

fn ts_extract_class_fields(source: &str, class_body: Node) -> Vec<Field> {
    let mut fields = Vec::new();

    for child in children(class_body) {
        // public_field_definition or property_declaration with type annotation
        let kind = child.kind();
        if kind != "public_field_definition" && kind != "property_declaration" {
            continue;
        }

        // Skip private/protected
        let mut is_private = false;
        for modifier in children(child) {
            let txt = node_text(modifier, source);
            if txt == "private" || txt == "protected" {
                is_private = true;
                break;
            }
        }
        if is_private {
            continue;
        }

        let field_name = child
            .child_by_field_name("name")
            .map(|n| node_text(n, source).to_string())
            .unwrap_or_default();

        if field_name.is_empty() || field_name.starts_with('#') {
            continue;
        }

        let field_type = child
            .child_by_field_name("type")
            .map(|t| node_text(t, source).to_string())
            .unwrap_or_default();

        let required = !field_name.ends_with('?');
        let clean_name = field_name.trim_end_matches('?').to_string();

        fields.push(Field {
            name: clean_name,
            field_type,
            required,
            description: String::new(),
        });
    }

    fields
}

fn ts_extract_interface_fields(source: &str, iface_body: Node) -> Vec<Field> {
    let mut fields = Vec::new();

    for child in children(iface_body) {
        if child.kind() != "property_signature" {
            continue;
        }

        let field_name = child
            .child_by_field_name("name")
            .map(|n| node_text(n, source).to_string())
            .unwrap_or_default();

        if field_name.is_empty() {
            continue;
        }

        let field_type = child
            .child_by_field_name("type")
            .map(|t| node_text(t, source).to_string())
            .unwrap_or_default();

        let required = !field_name.ends_with('?');
        let clean_name = field_name.trim_end_matches('?').to_string();

        fields.push(Field {
            name: clean_name,
            field_type,
            required,
            description: String::new(),
        });
    }

    fields
}

fn ts_extract_class_methods(
    source: &str,
    class_body: Node,
    owner: &str,
    file_path: &str,
) -> Vec<DiscoveredMethod> {
    let mut methods = Vec::new();

    for child in children(class_body) {
        if child.kind() != "method_definition" {
            continue;
        }

        // Skip private/protected
        let mut is_private = false;
        for modifier in children(child) {
            let txt = node_text(modifier, source);
            if txt == "private" || txt == "protected" {
                is_private = true;
                break;
            }
        }
        if is_private {
            continue;
        }

        let method_name = child
            .child_by_field_name("name")
            .map(|n| node_text(n, source))
            .unwrap_or("");

        // Skip constructor – it's a lifecycle method, not domain behavior
        if method_name.is_empty() || method_name == "constructor" {
            continue;
        }

        let params = child
            .child_by_field_name("parameters")
            .map(|p| ts_parse_params(source, p))
            .unwrap_or_default();

        let return_type = child
            .child_by_field_name("return_type")
            .map(|rt| node_text(rt, source).to_string())
            // Strip leading ": " from type annotation
            .map(|s| s.trim_start_matches(':').trim().to_string())
            .unwrap_or_default();

        methods.push(DiscoveredMethod {
            owner: owner.to_string(),
            name: method_name.to_string(),
            start_line: child.start_position().row + 1,
            end_line: child.end_position().row + 1,
            parameters: params,
            return_type,
            file_path: file_path.to_string(),
            extends: vec![],
            implements: vec![],
            decorators: vec![],
        });
    }

    methods
}

fn ts_parse_params(source: &str, params_node: Node) -> Vec<Field> {
    let mut fields = Vec::new();

    for param in children(params_node) {
        match param.kind() {
            "required_parameter" | "optional_parameter" => {
                let name = param
                    .child_by_field_name("pattern")
                    .map(|n| node_text(n, source))
                    .unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                let ty = param
                    .child_by_field_name("type")
                    .map(|t| node_text(t, source).to_string())
                    // Strip leading ": "
                    .map(|s| s.trim_start_matches(':').trim().to_string())
                    .unwrap_or_default();

                let required = param.kind() == "required_parameter";
                fields.push(Field {
                    name: name.to_string(),
                    field_type: ty,
                    required,
                    description: String::new(),
                });
            }
            _ => {}
        }
    }

    fields
}

// ─── Go extraction ─────────────────────────────────────────────────────────

fn go_extract_imports(source: &str, tree: &tree_sitter::Tree) -> Vec<String> {
    let query_src = r#"
        (import_spec
          path: (interpreted_string_literal) @module)
    "#;

    let language = tree.language();
    let query = match Query::new(&language, query_src) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!("Failed to compile Go import query: {e}");
            return vec![];
        }
    };

    let mut cursor = QueryCursor::new();
    let root = tree.root_node();
    let mut imports = Vec::new();

    let mut matches = cursor.matches(&query, root, source.as_bytes());
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let raw = node_text(cap.node, source);
            let cleaned = raw.trim_matches('"');
            imports.push(cleaned.to_string());
        }
    }

    imports
}

fn go_extract_structs_and_methods(
    source: &str,
    tree: &tree_sitter::Tree,
    file_path: &str,
) -> (Vec<DiscoveredStruct>, Vec<DiscoveredMethod>) {
    let mut structs = Vec::new();
    let mut methods = Vec::new();

    // Extract struct type declarations: `type Foo struct { ... }`
    let struct_query_src = r#"
        (type_declaration
          (type_spec
            name: (type_identifier) @struct_name
            type: (struct_type
              (field_declaration_list) @field_list)) @type_spec)
    "#;

    let language = tree.language();

    if let Ok(query) = Query::new(&language, struct_query_src) {
        let name_idx = query.capture_index_for_name("struct_name").unwrap();
        let field_list_idx = query.capture_index_for_name("field_list").unwrap();
        let type_spec_idx = query.capture_index_for_name("type_spec").unwrap();
        let mut cursor = QueryCursor::new();
        let root = tree.root_node();

        let mut matches = cursor.matches(&query, root, source.as_bytes());
        while let Some(m) = matches.next() {
            let mut name = "";
            let mut field_list_node = None;
            let mut spec_node = None;

            for cap in m.captures {
                if cap.index == name_idx {
                    name = node_text(cap.node, source);
                } else if cap.index == field_list_idx {
                    field_list_node = Some(cap.node);
                } else if cap.index == type_spec_idx {
                    spec_node = Some(cap.node);
                }
            }

            if name.is_empty() || !name.starts_with(|c: char| c.is_uppercase()) {
                continue;
            }

            let (fields, embedded) = field_list_node
                .map(|fl| go_extract_struct_fields(source, fl))
                .unwrap_or_default();

            let nd = spec_node.unwrap();
            structs.push(DiscoveredStruct {
                name: name.to_string(),
                start_line: nd.start_position().row + 1,
                end_line: nd.end_position().row + 1,
                fields,
                file_path: file_path.to_string(),
                extends: embedded,
                implements: vec![],
                decorators: vec![],
            });
        }
    }

    // Extract interface type declarations: `type Reader interface { ... }`
    let iface_query_src = r#"
        (type_declaration
          (type_spec
            name: (type_identifier) @iface_name
            type: (interface_type) @iface_body) @iface_spec)
    "#;

    if let Ok(query) = Query::new(&language, iface_query_src) {
        let name_idx = query.capture_index_for_name("iface_name").unwrap();
        let body_idx = query.capture_index_for_name("iface_body").unwrap();
        let spec_idx = query.capture_index_for_name("iface_spec").unwrap();
        let mut cursor = QueryCursor::new();
        let root = tree.root_node();

        let mut matches = cursor.matches(&query, root, source.as_bytes());
        while let Some(m) = matches.next() {
            let mut name = "";
            let mut body_node = None;
            let mut spec_node = None;

            for cap in m.captures {
                if cap.index == name_idx {
                    name = node_text(cap.node, source);
                } else if cap.index == body_idx {
                    body_node = Some(cap.node);
                } else if cap.index == spec_idx {
                    spec_node = Some(cap.node);
                }
            }

            if name.is_empty() || !name.starts_with(|c: char| c.is_uppercase()) {
                continue;
            }

            let (fields, iface_embedded) = body_node
                .map(|b| go_extract_interface_methods(source, b))
                .unwrap_or_default();

            let nd = spec_node.unwrap();
            structs.push(DiscoveredStruct {
                name: name.to_string(),
                start_line: nd.start_position().row + 1,
                end_line: nd.end_position().row + 1,
                fields,
                file_path: file_path.to_string(),
                extends: iface_embedded,
                implements: vec![],
                decorators: vec![],
            });
        }
    }

    // Extract method declarations: `func (r *Repo) Method(...) ReturnType { ... }`
    let method_query_src = r#"
        (method_declaration
          receiver: (parameter_list) @receiver
          name: (field_identifier) @method_name
          parameters: (parameter_list) @params) @method_node
    "#;

    if let Ok(query) = Query::new(&language, method_query_src) {
        let receiver_idx = query.capture_index_for_name("receiver").unwrap();
        let name_idx = query.capture_index_for_name("method_name").unwrap();
        let params_idx = query.capture_index_for_name("params").unwrap();
        let method_node_idx = query.capture_index_for_name("method_node").unwrap();
        let mut cursor = QueryCursor::new();
        let root = tree.root_node();

        let mut matches = cursor.matches(&query, root, source.as_bytes());
        while let Some(m) = matches.next() {
            let mut owner = String::new();
            let mut method_name = "";
            let mut params_node = None;
            let mut method_node = None;

            for cap in m.captures {
                if cap.index == receiver_idx {
                    owner = go_extract_receiver_type(source, cap.node);
                } else if cap.index == name_idx {
                    method_name = node_text(cap.node, source);
                } else if cap.index == params_idx {
                    params_node = Some(cap.node);
                } else if cap.index == method_node_idx {
                    method_node = Some(cap.node);
                }
            }

            // Only exported methods (capitalized)
            if method_name.is_empty() || !method_name.starts_with(|c: char| c.is_uppercase()) {
                continue;
            }

            let parameters = params_node
                .map(|p| go_parse_params(source, p))
                .unwrap_or_default();

            let return_type = method_node
                .and_then(|n| n.child_by_field_name("result"))
                .map(|rt| node_text(rt, source).to_string())
                .unwrap_or_default();

            let nd = method_node.unwrap();
            methods.push(DiscoveredMethod {
                owner,
                name: method_name.to_string(),
                start_line: nd.start_position().row + 1,
                end_line: nd.end_position().row + 1,
                parameters,
                return_type,
                file_path: file_path.to_string(),
                extends: vec![],
                implements: vec![],
                decorators: vec![],
            });
        }
    }

    (structs, methods)
}

fn go_extract_struct_fields(source: &str, field_list: Node) -> (Vec<Field>, Vec<String>) {
    let mut fields = Vec::new();
    let mut embedded = Vec::new();

    for child in children(field_list) {
        if child.kind() != "field_declaration" {
            continue;
        }

        let field_type = child
            .child_by_field_name("type")
            .map(|t| node_text(t, source).to_string())
            .unwrap_or_default();

        // A field_declaration may have multiple names: `X, Y int`
        let name_node = child.child_by_field_name("name");
        if let Some(n) = name_node {
            let field_name = node_text(n, source);
            // Only exported fields (capitalized)
            if field_name.starts_with(|c: char| c.is_uppercase()) {
                fields.push(Field {
                    name: field_name.to_string(),
                    field_type: field_type.clone(),
                    required: true,
                    description: String::new(),
                });
            }
        } else if !field_type.is_empty() {
            // Embedded field (no name, just a type) — this is Go struct embedding
            // Strip pointer prefix for the type name
            let embed_name = field_type.trim_start_matches('*').to_string();
            embedded.push(embed_name);
        }
    }

    (fields, embedded)
}

fn go_extract_interface_methods(source: &str, iface_body: Node) -> (Vec<Field>, Vec<String>) {
    let mut fields = Vec::new();
    let mut embedded = Vec::new();

    for child in children(iface_body) {
        if child.kind() == "method_elem" || child.kind() == "method_spec" {
            let name = child
                .child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_default();
            if !name.is_empty() {
                let params = child
                    .child_by_field_name("parameters")
                    .map(|p| node_text(p, source).to_string())
                    .unwrap_or_default();
                fields.push(Field {
                    name,
                    field_type: params,
                    required: true,
                    description: String::new(),
                });
            }
        } else if child.kind() == "type_identifier" || child.kind() == "qualified_type" {
            // Embedded interface (e.g. `io.Reader` inside an interface body)
            embedded.push(node_text(child, source).to_string());
        }
    }

    (fields, embedded)
}

fn go_extract_receiver_type(source: &str, receiver_list: Node) -> String {
    // receiver is `(r *Type)` or `(r Type)` — extract the type name
    for child in children(receiver_list) {
        if child.kind() == "parameter_declaration"
            && let Some(ty) = child.child_by_field_name("type")
        {
            let type_text = node_text(ty, source);
            // Strip pointer prefix
            return type_text.trim_start_matches('*').to_string();
        }
    }
    String::new()
}

fn go_parse_params(source: &str, params_node: Node) -> Vec<Field> {
    let mut fields = Vec::new();

    for child in children(params_node) {
        if child.kind() != "parameter_declaration" {
            continue;
        }

        let field_type = child
            .child_by_field_name("type")
            .map(|t| node_text(t, source).to_string())
            .unwrap_or_default();

        let name_node = child.child_by_field_name("name");
        if let Some(n) = name_node {
            let name = node_text(n, source);
            fields.push(Field {
                name: name.to_string(),
                field_type,
                required: true,
                description: String::new(),
            });
        }
    }

    fields
}

// ─── Call extraction helpers ───────────────────────────────────────────────

/// Find the enclosing function/method name for a tree-sitter node.
fn find_enclosing_function<'a>(mut node: Node<'a>, source: &str, family: LangFamily) -> String {
    loop {
        match node.parent() {
            None => return "<module>".to_string(),
            Some(parent) => {
                let kind = parent.kind();
                match family {
                    LangFamily::Python => {
                        if kind == "function_definition" {
                            if let Some(name_node) = parent.child_by_field_name("name") {
                                // Check if this is a method (inside a class)
                                if let Some(class_parent) = find_ancestor_class(parent, source) {
                                    return format!(
                                        "{}::{}",
                                        class_parent,
                                        node_text(name_node, source)
                                    );
                                }
                                return node_text(name_node, source).to_string();
                            }
                        }
                    }
                    LangFamily::TypeScript => {
                        if kind == "method_definition"
                            || kind == "function_declaration"
                            || kind == "arrow_function"
                        {
                            if let Some(name_node) = parent.child_by_field_name("name") {
                                if let Some(class_parent) = find_ancestor_class(parent, source) {
                                    return format!(
                                        "{}::{}",
                                        class_parent,
                                        node_text(name_node, source)
                                    );
                                }
                                return node_text(name_node, source).to_string();
                            }
                        }
                    }
                    LangFamily::Go => {
                        if kind == "function_declaration" {
                            if let Some(name_node) = parent.child_by_field_name("name") {
                                return node_text(name_node, source).to_string();
                            }
                        } else if kind == "method_declaration" {
                            let receiver = parent
                                .child_by_field_name("receiver")
                                .map(|r| go_extract_receiver_type(source, r))
                                .unwrap_or_default();
                            if let Some(name_node) = parent.child_by_field_name("name") {
                                if receiver.is_empty() {
                                    return node_text(name_node, source).to_string();
                                }
                                return format!("{}::{}", receiver, node_text(name_node, source));
                            }
                        }
                    }
                    LangFamily::Java => {
                        if kind == "method_declaration" || kind == "constructor_declaration" {
                            if let Some(name_node) = parent.child_by_field_name("name") {
                                if let Some(class_parent) = find_ancestor_class(parent, source) {
                                    return format!(
                                        "{}::{}",
                                        class_parent,
                                        node_text(name_node, source)
                                    );
                                }
                                return node_text(name_node, source).to_string();
                            }
                        }
                    }
                }
                node = parent;
            }
        }
    }
}

fn find_ancestor_class(node: Node, source: &str) -> Option<String> {
    let mut current = node.parent();
    while let Some(p) = current {
        if p.kind() == "class_definition" || p.kind() == "class_declaration" {
            if let Some(name_node) = p.child_by_field_name("name") {
                return Some(node_text(name_node, source).to_string());
            }
        }
        current = p.parent();
    }
    None
}

fn python_extract_calls(source: &str, tree: &tree_sitter::Tree) -> Vec<CallInfo> {
    let query_str =
        "(call function: [(identifier) @fn (attribute attribute: (identifier) @method)])";
    let lang = tree.language();
    let query = match Query::new(&lang, query_str) {
        Ok(q) => q,
        Err(_) => return vec![],
    };
    let mut cursor = QueryCursor::new();
    let mut calls = Vec::new();
    let mut iter = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = iter.next() {
        for cap in m.captures {
            let callee = node_text(cap.node, source).to_string();
            let line = cap.node.start_position().row + 1;
            let caller = find_enclosing_function(cap.node, source, LangFamily::Python);
            calls.push(CallInfo {
                caller,
                callee,
                line,
            });
        }
    }
    calls
}

fn ts_extract_calls(source: &str, tree: &tree_sitter::Tree) -> Vec<CallInfo> {
    let query_str = "(call_expression function: [(identifier) @fn (member_expression property: (property_identifier) @method)])";
    let lang = tree.language();
    let query = match Query::new(&lang, query_str) {
        Ok(q) => q,
        Err(_) => return vec![],
    };
    let mut cursor = QueryCursor::new();
    let mut calls = Vec::new();
    let mut iter = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = iter.next() {
        for cap in m.captures {
            let callee = node_text(cap.node, source).to_string();
            let line = cap.node.start_position().row + 1;
            let caller = find_enclosing_function(cap.node, source, LangFamily::TypeScript);
            calls.push(CallInfo {
                caller,
                callee,
                line,
            });
        }
    }
    calls
}

fn go_extract_calls(source: &str, tree: &tree_sitter::Tree) -> Vec<CallInfo> {
    let query_str = "(call_expression function: [(identifier) @fn (selector_expression field: (field_identifier) @method)])";
    let lang = tree.language();
    let query = match Query::new(&lang, query_str) {
        Ok(q) => q,
        Err(_) => return vec![],
    };
    let mut cursor = QueryCursor::new();
    let mut calls = Vec::new();
    let mut iter = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = iter.next() {
        for cap in m.captures {
            let callee = node_text(cap.node, source).to_string();
            let line = cap.node.start_position().row + 1;
            let caller = find_enclosing_function(cap.node, source, LangFamily::Go);
            calls.push(CallInfo {
                caller,
                callee,
                line,
            });
        }
    }
    calls
}

// ─── Java extraction ───────────────────────────────────────────────────────────────

fn java_extract_imports(source: &str, tree: &tree_sitter::Tree) -> Vec<String> {
    let query_src = "(import_declaration) @import";
    let language = tree.language();
    let query = match Query::new(&language, query_src) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!("Failed to compile Java import query: {e}");
            return vec![];
        }
    };
    let mut cursor = QueryCursor::new();
    let mut imports = Vec::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let raw = node_text(cap.node, source);
            // Strip `import ` prefix and trailing `;`
            let cleaned = raw
                .trim_start_matches("import ")
                .trim_start_matches("static ")
                .trim_end_matches(';')
                .trim();
            if !cleaned.is_empty() {
                imports.push(cleaned.to_string());
            }
        }
    }
    imports
}

fn java_extract_classes(
    source: &str,
    tree: &tree_sitter::Tree,
    file_path: &str,
) -> (Vec<DiscoveredStruct>, Vec<DiscoveredMethod>) {
    let mut structs = Vec::new();
    let mut methods = Vec::new();
    let language = tree.language();

    // Extract classes
    let class_query_src = r#"
        (class_declaration
          name: (identifier) @class_name
          body: (class_body) @class_body) @class_node
    "#;

    if let Ok(query) = Query::new(&language, class_query_src) {
        let name_idx = query.capture_index_for_name("class_name").unwrap();
        let body_idx = query.capture_index_for_name("class_body").unwrap();
        let class_node_idx = query.capture_index_for_name("class_node").unwrap();
        let mut cursor = QueryCursor::new();

        let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
        while let Some(m) = matches.next() {
            let mut name = "";
            let mut body_node = None;
            let mut class_node = None;

            for cap in m.captures {
                if cap.index == name_idx {
                    name = node_text(cap.node, source);
                } else if cap.index == body_idx {
                    body_node = Some(cap.node);
                } else if cap.index == class_node_idx {
                    class_node = Some(cap.node);
                }
            }

            if name.is_empty() {
                continue;
            }

            let cn = match class_node {
                Some(n) => n,
                None => continue,
            };

            // Extract extends/implements
            let mut extends = Vec::new();
            let mut implements = Vec::new();
            for child in children(cn) {
                if child.kind() == "superclass" {
                    if let Some(type_node) = child.child_by_field_name("type").or_else(|| child.child(1)) {
                        extends.push(node_text(type_node, source).to_string());
                    }
                } else if child.kind() == "super_interfaces" {
                    for iface_child in children(child) {
                        if iface_child.kind() == "type_list" {
                            for type_node in children(iface_child) {
                                let iface_name = node_text(type_node, source).to_string();
                                if !iface_name.is_empty() && iface_name != "," {
                                    implements.push(iface_name);
                                }
                            }
                        }
                    }
                }
            }

            // Extract annotations as decorators
            let decorators = java_extract_annotations(cn, source);

            // Extract fields and methods from class body
            let fields = body_node
                .map(|b| java_extract_fields(source, b))
                .unwrap_or_default();

            let class_methods = body_node
                .map(|b| java_extract_methods(source, b, name, file_path))
                .unwrap_or_default();
            methods.extend(class_methods);

            structs.push(DiscoveredStruct {
                name: name.to_string(),
                start_line: cn.start_position().row + 1,
                end_line: cn.end_position().row + 1,
                fields,
                file_path: file_path.to_string(),
                extends,
                implements,
                decorators,
            });
        }
    }

    // Extract interfaces
    let iface_query_src = r#"
        (interface_declaration
          name: (identifier) @iface_name
          body: (interface_body) @iface_body) @iface_node
    "#;

    if let Ok(query) = Query::new(&language, iface_query_src) {
        let name_idx = query.capture_index_for_name("iface_name").unwrap();
        let body_idx = query.capture_index_for_name("iface_body").unwrap();
        let iface_node_idx = query.capture_index_for_name("iface_node").unwrap();
        let mut cursor = QueryCursor::new();

        let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
        while let Some(m) = matches.next() {
            let mut name = "";
            let mut body_node = None;
            let mut iface_node = None;

            for cap in m.captures {
                if cap.index == name_idx {
                    name = node_text(cap.node, source);
                } else if cap.index == body_idx {
                    body_node = Some(cap.node);
                } else if cap.index == iface_node_idx {
                    iface_node = Some(cap.node);
                }
            }

            if name.is_empty() {
                continue;
            }

            let nd = match iface_node {
                Some(n) => n,
                None => continue,
            };

            // Extract extends for interfaces
            let mut extends = Vec::new();
            for child in children(nd) {
                if child.kind() == "extends_interfaces" {
                    for type_list in children(child) {
                        if type_list.kind() == "type_list" {
                            for type_node in children(type_list) {
                                let iface_name = node_text(type_node, source).to_string();
                                if !iface_name.is_empty() && iface_name != "," {
                                    extends.push(iface_name);
                                }
                            }
                        }
                    }
                }
            }

            let decorators = java_extract_annotations(nd, source);

            // Interface methods become fields (same pattern as Go interfaces)
            let fields = body_node
                .map(|b| java_extract_interface_method_signatures(source, b))
                .unwrap_or_default();

            structs.push(DiscoveredStruct {
                name: name.to_string(),
                start_line: nd.start_position().row + 1,
                end_line: nd.end_position().row + 1,
                fields,
                file_path: file_path.to_string(),
                extends,
                implements: vec![],
                decorators,
            });
        }
    }

    // Extract enum declarations
    let enum_query_src = r#"
        (enum_declaration
          name: (identifier) @enum_name
          body: (enum_body) @enum_body) @enum_node
    "#;

    if let Ok(query) = Query::new(&language, enum_query_src) {
        let name_idx = query.capture_index_for_name("enum_name").unwrap();
        let body_idx = query.capture_index_for_name("enum_body").unwrap();
        let enum_node_idx = query.capture_index_for_name("enum_node").unwrap();
        let mut cursor = QueryCursor::new();

        let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
        while let Some(m) = matches.next() {
            let mut name = "";
            let mut body_node = None;
            let mut enum_node = None;

            for cap in m.captures {
                if cap.index == name_idx {
                    name = node_text(cap.node, source);
                } else if cap.index == body_idx {
                    body_node = Some(cap.node);
                } else if cap.index == enum_node_idx {
                    enum_node = Some(cap.node);
                }
            }

            if name.is_empty() {
                continue;
            }

            let nd = match enum_node {
                Some(n) => n,
                None => continue,
            };

            // Extract enum constants as fields
            let fields = body_node
                .map(|b| {
                    let mut constants = Vec::new();
                    for child in children(b) {
                        if child.kind() == "enum_constant" {
                            if let Some(const_name) = child.child_by_field_name("name") {
                                constants.push(Field {
                                    name: node_text(const_name, source).to_string(),
                                    field_type: "(enum)".to_string(),
                                    required: true,
                                    description: String::new(),
                                });
                            }
                        }
                    }
                    constants
                })
                .unwrap_or_default();

            let decorators = java_extract_annotations(nd, source);

            structs.push(DiscoveredStruct {
                name: name.to_string(),
                start_line: nd.start_position().row + 1,
                end_line: nd.end_position().row + 1,
                fields,
                file_path: file_path.to_string(),
                extends: vec![],
                implements: vec![],
                decorators,
            });
        }
    }

    (structs, methods)
}

fn java_extract_fields(source: &str, class_body: Node) -> Vec<Field> {
    let mut fields = Vec::new();
    for child in children(class_body) {
        if child.kind() != "field_declaration" {
            continue;
        }
        let field_type = child
            .child_by_field_name("type")
            .map(|t| node_text(t, source).to_string())
            .unwrap_or_default();
        // Extract variable declarators
        for decl_child in children(child) {
            if decl_child.kind() == "variable_declarator" {
                if let Some(name_node) = decl_child.child_by_field_name("name") {
                    fields.push(Field {
                        name: node_text(name_node, source).to_string(),
                        field_type: field_type.clone(),
                        required: true,
                        description: String::new(),
                    });
                }
            }
        }
    }
    fields
}

fn java_extract_methods(
    source: &str,
    class_body: Node,
    owner: &str,
    file_path: &str,
) -> Vec<DiscoveredMethod> {
    let mut methods = Vec::new();
    for child in children(class_body) {
        if child.kind() != "method_declaration" {
            continue;
        }
        let name = child
            .child_by_field_name("name")
            .map(|n| node_text(n, source).to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }

        // Check visibility — skip private methods
        let modifiers_text = children(child)
            .filter(|c| c.kind() == "modifiers")
            .map(|m| node_text(m, source))
            .collect::<Vec<_>>()
            .join(" ");
        if modifiers_text.contains("private") {
            continue;
        }

        let return_type = child
            .child_by_field_name("type")
            .map(|t| node_text(t, source).to_string())
            .unwrap_or_else(|| "void".to_string());

        let parameters = child
            .child_by_field_name("parameters")
            .map(|p| java_parse_params(source, p))
            .unwrap_or_default();

        let decorators = java_extract_annotations(child, source);

        methods.push(DiscoveredMethod {
            owner: owner.to_string(),
            name,
            start_line: child.start_position().row + 1,
            end_line: child.end_position().row + 1,
            parameters,
            return_type,
            file_path: file_path.to_string(),
            extends: vec![],
            implements: vec![],
            decorators,
        });
    }
    methods
}

fn java_extract_interface_method_signatures(source: &str, iface_body: Node) -> Vec<Field> {
    let mut fields = Vec::new();
    for child in children(iface_body) {
        if child.kind() == "method_declaration" {
            let name = child
                .child_by_field_name("name")
                .map(|n| node_text(n, source).to_string())
                .unwrap_or_default();
            let return_type = child
                .child_by_field_name("type")
                .map(|t| node_text(t, source).to_string())
                .unwrap_or_default();
            if !name.is_empty() {
                let params = child
                    .child_by_field_name("parameters")
                    .map(|p| node_text(p, source).to_string())
                    .unwrap_or_default();
                fields.push(Field {
                    name,
                    field_type: format!("{params} -> {return_type}"),
                    required: true,
                    description: String::new(),
                });
            }
        }
    }
    fields
}

fn java_extract_annotations(node: Node, source: &str) -> Vec<String> {
    let mut decorators = Vec::new();
    // Look for marker_annotation and annotation siblings before the declaration
    if let Some(parent) = node.parent() {
        for child in children(parent) {
            if child.id() == node.id() {
                break;
            }
            if child.kind() == "marker_annotation" || child.kind() == "annotation" {
                decorators.push(node_text(child, source).to_string());
            }
        }
    }
    // Also check inside modifiers
    for child in children(node) {
        if child.kind() == "modifiers" {
            for mod_child in children(child) {
                if mod_child.kind() == "marker_annotation" || mod_child.kind() == "annotation" {
                    decorators.push(node_text(mod_child, source).to_string());
                }
            }
        }
    }
    decorators
}

fn java_parse_params(source: &str, params_node: Node) -> Vec<Field> {
    let mut fields = Vec::new();
    for child in children(params_node) {
        if child.kind() != "formal_parameter" && child.kind() != "spread_parameter" {
            continue;
        }
        let field_type = child
            .child_by_field_name("type")
            .map(|t| node_text(t, source).to_string())
            .unwrap_or_default();
        let name = child
            .child_by_field_name("name")
            .map(|n| node_text(n, source).to_string())
            .unwrap_or_default();
        if !name.is_empty() {
            fields.push(Field {
                name,
                field_type,
                required: true,
                description: String::new(),
            });
        }
    }
    fields
}

fn java_extract_calls(source: &str, tree: &tree_sitter::Tree) -> Vec<CallInfo> {
    let query_str = "(method_invocation name: (identifier) @method)";
    let lang = tree.language();
    let query = match Query::new(&lang, query_str) {
        Ok(q) => q,
        Err(_) => return vec![],
    };
    let mut cursor = QueryCursor::new();
    let mut calls = Vec::new();
    let mut iter = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = iter.next() {
        for cap in m.captures {
            let callee_name = node_text(cap.node, source).to_string();
            let line = cap.node.start_position().row + 1;
            // Check for object.method() pattern
            let callee = if let Some(invocation) = cap.node.parent() {
                if let Some(obj) = invocation.child_by_field_name("object") {
                    format!("{}.{}", node_text(obj, source), callee_name)
                } else {
                    callee_name
                }
            } else {
                callee_name
            };
            let caller = find_enclosing_function(cap.node, source, LangFamily::Java);
            calls.push(CallInfo {
                caller,
                callee,
                line,
            });
        }
    }
    calls
}

/// Java: extract `package <name>;` declaration as the module.
fn java_extract_modules(source: &str, tree: &tree_sitter::Tree, file_path: &Path) -> Vec<DiscoveredModule> {
    let query_str = "(package_declaration) @pkg";
    let lang = tree.language();
    let query = match Query::new(&lang, query_str) {
        Ok(q) => q,
        Err(_) => return vec![],
    };
    let mut cursor = QueryCursor::new();
    let mut iter = cursor.matches(&query, tree.root_node(), source.as_bytes());
    if let Some(m) = iter.next() {
        if let Some(cap) = m.captures.first() {
            let raw = node_text(cap.node, source);
            // Strip `package ` prefix and trailing `;`
            let pkg_name = raw
                .trim_start_matches("package ")
                .trim_end_matches(';')
                .trim()
                .to_string();
            if !pkg_name.is_empty() {
                return vec![DiscoveredModule {
                    name: pkg_name,
                    public: true,
                    file_path: file_path.to_string_lossy().to_string(),
                    extends: vec![],
                    implements: vec![],
                    decorators: vec![],
                }];
            }
        }
    }
    vec![]
}

// ─── Module extraction ─────────────────────────────────────────────────────────────

use super::analyze::DiscoveredModule;

/// Python: each .py file is a module, inferred from filename.
fn python_extract_modules(file_path: &Path) -> Vec<DiscoveredModule> {
    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if stem.is_empty() || stem == "__init__" {
        return vec![];
    }
    vec![DiscoveredModule {
        name: stem.to_string(),
        public: true,
        file_path: file_path.to_string_lossy().to_string(),
        extends: vec![],
        implements: vec![],
        decorators: vec![],
    }]
}

/// TypeScript: each .ts/.tsx file is a module, inferred from filename.
fn ts_extract_modules(_source: &str, _tree: &tree_sitter::Tree, file_path: &Path) -> Vec<DiscoveredModule> {
    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if stem.is_empty() || stem == "index" {
        return vec![];
    }
    vec![DiscoveredModule {
        name: stem.to_string(),
        public: true,
        file_path: file_path.to_string_lossy().to_string(),
        extends: vec![],
        implements: vec![],
        decorators: vec![],
    }]
}

/// Go: extract `package <name>` declaration.
fn go_extract_modules(source: &str, tree: &tree_sitter::Tree, file_path: &Path) -> Vec<DiscoveredModule> {
    let query_str = "(package_clause (package_identifier) @pkg)";
    let lang = tree.language();
    let query = match Query::new(&lang, query_str) {
        Ok(q) => q,
        Err(_) => return vec![],
    };
    let mut cursor = QueryCursor::new();
    let mut iter = cursor.matches(&query, tree.root_node(), source.as_bytes());
    if let Some(m) = iter.next() {
        if let Some(cap) = m.captures.first() {
            let pkg_name = node_text(cap.node, source).to_string();
            if pkg_name != "main" {
                return vec![DiscoveredModule {
                    name: pkg_name,
                    public: true,
                    file_path: file_path.to_string_lossy().to_string(),
                    extends: vec![],
                    implements: vec![],
                    decorators: vec![],
                }];
            }
        }
    }
    vec![]
}

// ─── AstScanner implementation ─────────────────────────────────────────────

impl AstScanner for TreeSitterScanner {
    fn extract_live_dependencies(
        &self,
        file_path: &Path,
        source_code: &str,
    ) -> Result<Vec<LiveDependency>> {
        let (tree, family) = match self.parse_tree(file_path, source_code)? {
            Some(pair) => pair,
            None => return Ok(vec![]),
        };

        let from_file = file_path.to_string_lossy().to_string();

        let raw_imports = match family {
            LangFamily::Python => python_extract_imports(source_code, &tree),
            LangFamily::TypeScript => ts_extract_imports(source_code, &tree),
            LangFamily::Go => go_extract_imports(source_code, &tree),
            LangFamily::Java => java_extract_imports(source_code, &tree),
        };

        let deps = raw_imports
            .into_iter()
            .map(|to_module| LiveDependency {
                from_file: from_file.clone(),
                to_module,
            })
            .collect();

        Ok(deps)
    }

    fn scan_file(&self, file_path: &Path, source_code: &str) -> Result<ScanResult> {
        let (tree, family) = match self.parse_tree(file_path, source_code)? {
            Some(pair) => pair,
            None => return Ok((vec![], vec![], vec![], vec![])),
        };

        let fp = file_path.to_string_lossy().to_string();

        let (structs, methods) = match family {
            LangFamily::Python => python_extract_classes(source_code, &tree, &fp),
            LangFamily::TypeScript => ts_extract_classes(source_code, &tree, &fp),
            LangFamily::Go => go_extract_structs_and_methods(source_code, &tree, &fp),
            LangFamily::Java => java_extract_classes(source_code, &tree, &fp),
        };

        let modules = match family {
            LangFamily::Python => python_extract_modules(file_path),
            LangFamily::TypeScript => ts_extract_modules(source_code, &tree, file_path),
            LangFamily::Go => go_extract_modules(source_code, &tree, file_path),
            LangFamily::Java => java_extract_modules(source_code, &tree, file_path),
        };

        Ok((structs, vec![], methods, modules))
    }

    fn extract_calls(&self, file_path: &Path, source_code: &str) -> Result<Vec<CallInfo>> {
        let (tree, family) = match self.parse_tree(file_path, source_code)? {
            Some(pair) => pair,
            None => return Ok(vec![]),
        };

        let calls = match family {
            LangFamily::Python => python_extract_calls(source_code, &tree),
            LangFamily::TypeScript => ts_extract_calls(source_code, &tree),
            LangFamily::Go => go_extract_calls(source_code, &tree),
            LangFamily::Java => java_extract_calls(source_code, &tree),
        };

        Ok(calls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::scanner::AstScanner;
    use std::path::Path;

    fn scanner() -> TreeSitterScanner {
        TreeSitterScanner::new()
    }

    // ── Python ─────────────────────────────────────────────────────────────

    const PYTHON_SOURCE: &str = r#"
import os
from collections import OrderedDict
from typing import List

class Order:
    def __init__(self, order_id: str, total: float):
        self.order_id = order_id
        self.total = total

    def apply_discount(self, percent: float) -> float:
        return self.total * (1 - percent)

    def _internal(self):
        pass

class OrderService:
    def process(self, order: Order) -> bool:
        order.apply_discount(0.1)
        return True
"#;

    #[test]
    fn test_python_imports() {
        let deps = scanner()
            .extract_live_dependencies(Path::new("test.py"), PYTHON_SOURCE)
            .unwrap();
        assert!(deps.iter().any(|d| d.to_module == "os"));
        assert!(deps.iter().any(|d| d.to_module == "collections"));
    }

    #[test]
    fn test_python_classes_and_methods() {
        let (structs, _, methods, _) = scanner()
            .scan_file(Path::new("test.py"), PYTHON_SOURCE)
            .unwrap();
        assert_eq!(structs.len(), 2);

        let order = structs.iter().find(|s| s.name == "Order").unwrap();
        assert_eq!(order.fields.len(), 2);
        assert!(order.fields.iter().any(|f| f.name == "order_id"));
        assert!(order.fields.iter().any(|f| f.name == "total"));

        // apply_discount is a method on Order
        let order_methods: Vec<_> = methods.iter().filter(|m| m.owner == "Order").collect();
        assert!(order_methods.iter().any(|m| m.name == "apply_discount"));
    }

    #[test]
    fn test_python_calls() {
        let calls = scanner()
            .extract_calls(Path::new("test.py"), PYTHON_SOURCE)
            .unwrap();
        assert!(calls.iter().any(|c| c.callee == "apply_discount"));
    }

    #[test]
    fn test_python_modules() {
        let (_, _, _, modules) = scanner()
            .scan_file(Path::new("src/billing/order.py"), PYTHON_SOURCE)
            .unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "order");
    }

    #[test]
    fn test_python_modules_init_excluded() {
        let (_, _, _, modules) = scanner()
            .scan_file(Path::new("src/billing/__init__.py"), "")
            .unwrap();
        assert!(modules.is_empty());
    }

    // ── TypeScript ─────────────────────────────────────────────────────────

    const TS_SOURCE: &str = r#"
import { Injectable } from '@nestjs/common';
import { Repository } from './repository';

export interface Identifiable {
    id: string;
}

export class UserService {
    private repo: Repository;

    constructor(repo: Repository) {
        this.repo = repo;
    }

    public findUser(id: string): User {
        return this.repo.findById(id);
    }

    private validate(user: User): boolean {
        return user.id !== '';
    }
}

export class User implements Identifiable {
    id: string;
    name: string;
    email: string;
}
"#;

    #[test]
    fn test_ts_imports() {
        let deps = scanner()
            .extract_live_dependencies(Path::new("test.ts"), TS_SOURCE)
            .unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().any(|d| d.to_module == "@nestjs/common"));
        assert!(deps.iter().any(|d| d.to_module == "./repository"));
    }

    #[test]
    fn test_ts_classes_and_interfaces() {
        let (structs, _, _, _) = scanner()
            .scan_file(Path::new("test.ts"), TS_SOURCE)
            .unwrap();
        // Identifiable (interface), UserService (class), User (class)
        assert!(structs.len() >= 2);

        let user = structs.iter().find(|s| s.name == "User").unwrap();
        assert_eq!(user.fields.len(), 3);
        assert!(user.implements.contains(&"Identifiable".to_string()));

        let svc = structs.iter().find(|s| s.name == "UserService");
        assert!(svc.is_some());
    }

    #[test]
    fn test_ts_calls() {
        let calls = scanner()
            .extract_calls(Path::new("test.ts"), TS_SOURCE)
            .unwrap();
        assert!(calls.iter().any(|c| c.callee.contains("findById")));
    }

    #[test]
    fn test_ts_modules() {
        let (_, _, _, modules) = scanner()
            .scan_file(Path::new("src/users/service.ts"), TS_SOURCE)
            .unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "service");
    }

    #[test]
    fn test_ts_modules_index_excluded() {
        let (_, _, _, modules) = scanner()
            .scan_file(Path::new("src/users/index.ts"), TS_SOURCE)
            .unwrap();
        assert!(modules.is_empty());
    }

    // ── Go ─────────────────────────────────────────────────────────────────

    const GO_SOURCE: &str = r#"
package billing

import (
    "fmt"
    "context"
)

type Invoice struct {
    ID     string
    Amount float64
    items  []Item
}

type Reader interface {
    Read(ctx context.Context) ([]byte, error)
}

func (i *Invoice) Apply(discount float64) float64 {
    return i.Amount * (1 - discount)
}

func NewInvoice(id string) *Invoice {
    fmt.Println("creating invoice")
    return &Invoice{ID: id}
}
"#;

    #[test]
    fn test_go_imports() {
        let deps = scanner()
            .extract_live_dependencies(Path::new("test.go"), GO_SOURCE)
            .unwrap();
        assert!(deps.iter().any(|d| d.to_module == "fmt"));
        assert!(deps.iter().any(|d| d.to_module == "context"));
    }

    #[test]
    fn test_go_structs_and_interfaces() {
        let (structs, _, methods, _) = scanner()
            .scan_file(Path::new("test.go"), GO_SOURCE)
            .unwrap();
        assert!(structs.len() >= 2);

        let invoice = structs.iter().find(|s| s.name == "Invoice").unwrap();
        // Only exported (capitalized) fields: ID, Amount — items is private
        assert_eq!(invoice.fields.len(), 2);
        assert!(invoice.fields.iter().any(|f| f.name == "ID"));
        assert!(invoice.fields.iter().any(|f| f.name == "Amount"));

        let reader = structs.iter().find(|s| s.name == "Reader").unwrap();
        assert!(!reader.fields.is_empty()); // Read method signature as field

        // Apply is a method on Invoice
        let apply = methods.iter().find(|m| m.name == "Apply").unwrap();
        assert_eq!(apply.owner, "Invoice");
    }

    #[test]
    fn test_go_calls() {
        let calls = scanner()
            .extract_calls(Path::new("test.go"), GO_SOURCE)
            .unwrap();
        assert!(calls.iter().any(|c| c.callee == "Println"));
    }

    #[test]
    fn test_go_modules() {
        let (_, _, _, modules) = scanner()
            .scan_file(Path::new("invoice.go"), GO_SOURCE)
            .unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "billing");
    }

    #[test]
    fn test_go_modules_main_excluded() {
        let main_src = "package main\n\nfunc main() {}\n";
        let (_, _, _, modules) = scanner()
            .scan_file(Path::new("main.go"), main_src)
            .unwrap();
        assert!(modules.is_empty());
    }

    // ── Java ───────────────────────────────────────────────────────────────

    const JAVA_SOURCE: &str = r#"
package com.example.orders;

import java.util.List;
import com.example.shared.BaseEntity;

@Entity
public class Order extends BaseEntity implements Serializable {
    private String orderId;
    private List<OrderItem> items;
    private double totalAmount;

    public Order(String orderId) {
        this.orderId = orderId;
    }

    public void addItem(OrderItem item) {
        items.add(item);
        recalculate();
    }

    public double getTotal() {
        return totalAmount;
    }

    private void recalculate() {
        totalAmount = items.stream().mapToDouble(OrderItem.getPrice).sum();
    }
}
"#;

    const JAVA_INTERFACE_SOURCE: &str = r#"
package com.example.orders;

public interface OrderRepository {
    Order findById(String id);
    List<Order> findAll();
    void save(Order order);
}
"#;

    const JAVA_ENUM_SOURCE: &str = r#"
package com.example.orders;

public enum OrderStatus {
    PENDING, CONFIRMED, SHIPPED, DELIVERED, CANCELLED;
}
"#;

    #[test]
    fn test_java_imports() {
        let deps = scanner()
            .extract_live_dependencies(Path::new("Order.java"), JAVA_SOURCE)
            .unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.iter().any(|d| d.to_module == "java.util.List"));
        assert!(deps
            .iter()
            .any(|d| d.to_module == "com.example.shared.BaseEntity"));
    }

    #[test]
    fn test_java_class_extraction() {
        let (structs, _, methods, _) = scanner()
            .scan_file(Path::new("Order.java"), JAVA_SOURCE)
            .unwrap();

        assert_eq!(structs.len(), 1);
        let order = &structs[0];
        assert_eq!(order.name, "Order");
        assert!(order.extends.contains(&"BaseEntity".to_string()));
        assert!(order.implements.contains(&"Serializable".to_string()));

        // Fields: orderId, items, totalAmount
        assert_eq!(order.fields.len(), 3);
        assert!(order.fields.iter().any(|f| f.name == "orderId"));
        assert!(order.fields.iter().any(|f| f.name == "totalAmount"));

        // Annotations captured as decorators
        assert!(order.decorators.iter().any(|d| d.contains("Entity")));

        // Public methods only (recalculate is private → excluded)
        let order_methods: Vec<_> = methods.iter().filter(|m| m.owner == "Order").collect();
        assert!(order_methods.iter().any(|m| m.name == "addItem"));
        assert!(order_methods.iter().any(|m| m.name == "getTotal"));
        assert!(!order_methods.iter().any(|m| m.name == "recalculate"));
    }

    #[test]
    fn test_java_interface_extraction() {
        let (structs, _, _, _) = scanner()
            .scan_file(Path::new("OrderRepository.java"), JAVA_INTERFACE_SOURCE)
            .unwrap();

        assert_eq!(structs.len(), 1);
        let repo = &structs[0];
        assert_eq!(repo.name, "OrderRepository");
        // Interface methods captured as fields (method signatures)
        assert!(repo.fields.len() >= 3);
        assert!(repo.fields.iter().any(|f| f.name == "findById"));
        assert!(repo.fields.iter().any(|f| f.name == "findAll"));
        assert!(repo.fields.iter().any(|f| f.name == "save"));
    }

    #[test]
    fn test_java_enum_extraction() {
        let (structs, _, _, _) = scanner()
            .scan_file(Path::new("OrderStatus.java"), JAVA_ENUM_SOURCE)
            .unwrap();

        assert_eq!(structs.len(), 1);
        let status = &structs[0];
        assert_eq!(status.name, "OrderStatus");
        assert_eq!(status.fields.len(), 5);
        assert!(status.fields.iter().any(|f| f.name == "PENDING"));
        assert!(status.fields.iter().any(|f| f.name == "CANCELLED"));
    }

    #[test]
    fn test_java_calls() {
        let calls = scanner()
            .extract_calls(Path::new("Order.java"), JAVA_SOURCE)
            .unwrap();
        assert!(calls.iter().any(|c| c.callee.contains("add")));
        assert!(calls.iter().any(|c| c.callee.contains("recalculate")));
    }

    #[test]
    fn test_java_modules() {
        let (_, _, _, modules) = scanner()
            .scan_file(Path::new("Order.java"), JAVA_SOURCE)
            .unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "com.example.orders");
    }

    #[test]
    fn test_java_method_params() {
        let (_, _, methods, _) = scanner()
            .scan_file(Path::new("Order.java"), JAVA_SOURCE)
            .unwrap();
        let add_item = methods.iter().find(|m| m.name == "addItem").unwrap();
        assert_eq!(add_item.parameters.len(), 1);
        assert_eq!(add_item.parameters[0].name, "item");
        assert_eq!(add_item.parameters[0].field_type, "OrderItem");
        assert_eq!(add_item.return_type, "void");
    }
}
