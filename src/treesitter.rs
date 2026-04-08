//! Tree-sitter integration for AST-aware code chunking and entity extraction.
//!
//! Provides:
//! - Language detection from file extensions (`get_language`)
//! - AST-aware chunking (`chunk_code_ast`) — replaces regex `is_code_boundary()`
//!   for `type = "code"` folders
//! - Entity + relationship extraction (`extract_code_entities`) — replaces LLM
//!   extraction for code files at zero cost

use tree_sitter::{Language, Node, Parser};

use crate::chunk::{MAX_CHUNK_CHARS, MIN_CHUNK_CHARS};

// ─── Public types ─────────────────────────────────────────────────────────────

/// A single semantic chunk produced by AST-aware splitting.
#[derive(Debug, Clone)]
pub struct AstChunk {
    pub content: String,
    /// 1-indexed, inclusive
    pub line_start: usize,
    /// 1-indexed, inclusive
    pub line_end: usize,
    /// E.g. "function", "impl", "struct", "class", "import_block", "module_level"
    pub chunk_type: String,
    /// Identifier name when available (fn name, struct name, …)
    pub name: Option<String>,
}

/// A code entity extracted from the AST (function, struct, class, …).
#[derive(Debug, Clone)]
pub struct CodeEntity {
    pub name: String,
    pub entity_type: String,
    /// The signature line (first line of the definition).
    pub description: String,
    pub file_path: String,
    pub line_start: usize,
    pub line_end: usize,
}

/// A directed relationship between code entities.
#[derive(Debug, Clone)]
pub struct CodeRelationship {
    pub source: String,
    pub target: String,
    /// One of: "calls", "imports", "implements", "extends", "contains", "uses_type"
    pub relation: String,
    pub file_path: String,
}

// ─── Language detection ───────────────────────────────────────────────────────

/// Return the tree-sitter `Language` for the given file extension, or `None`
/// if the extension is not supported by the active feature flags.
pub fn get_language(file_ext: &str) -> Option<Language> {
    match file_ext {
        // ts-core languages
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "js" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" | "tsx" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "c" | "h" => Some(tree_sitter_c::LANGUAGE.into()),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => Some(tree_sitter_cpp::LANGUAGE.into()),
        "cs" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "rb" => Some(tree_sitter_ruby::LANGUAGE.into()),
        "php" => Some(tree_sitter_php::LANGUAGE_PHP.into()),
        "sh" | "bash" => Some(tree_sitter_bash::LANGUAGE.into()),
        "kt" | "kts" => Some(tree_sitter_kotlin_ng::LANGUAGE.into()),
        "swift" => Some(tree_sitter_swift::LANGUAGE.into()),
        "ex" | "exs" => Some(tree_sitter_elixir::LANGUAGE.into()),
        "lua" => Some(tree_sitter_lua::LANGUAGE.into()),
        "hs" => Some(tree_sitter_haskell::LANGUAGE.into()),
        "html" | "htm" => Some(tree_sitter_html::LANGUAGE.into()),
        "css" => Some(tree_sitter_css::LANGUAGE.into()),
        "toml" => Some(tree_sitter_toml_ng::LANGUAGE.into()),
        "yml" | "yaml" => Some(tree_sitter_yaml::LANGUAGE.into()),
        "json" => Some(tree_sitter_json::LANGUAGE.into()),
        "ml" | "mli" => Some(tree_sitter_ocaml::LANGUAGE_OCAML.into()),
        "dart" => Some(tree_sitter_dart::LANGUAGE.into()),
        _ => None,
    }
}

// ─── AST-aware chunking ───────────────────────────────────────────────────────

/// Top-level node kinds that are treated as independent chunks per language.
fn top_level_kinds(file_ext: &str) -> &'static [&'static str] {
    match file_ext {
        "rs" => &[
            "function_item",
            "impl_item",
            "struct_item",
            "enum_item",
            "trait_item",
            "mod_item",
            "use_declaration",
            "const_item",
            "static_item",
            "type_item",
            "macro_definition",
            "attribute_item",
        ],
        "py" => &[
            "function_definition",
            "class_definition",
            "import_statement",
            "import_from_statement",
            "decorated_definition",
        ],
        "js" | "jsx" => &[
            "function_declaration",
            "class_declaration",
            "export_statement",
            "import_statement",
            "lexical_declaration",
            "variable_declaration",
        ],
        "ts" | "tsx" => &[
            "function_declaration",
            "class_declaration",
            "export_statement",
            "import_statement",
            "lexical_declaration",
            "variable_declaration",
            "interface_declaration",
            "type_alias_declaration",
            "enum_declaration",
        ],
        "go" => &[
            "function_declaration",
            "method_declaration",
            "type_declaration",
            "import_declaration",
            "const_declaration",
            "var_declaration",
        ],
        "c" | "h" => &[
            "function_definition",
            "struct_specifier",
            "enum_specifier",
            "type_definition",
            "preproc_include",
            "declaration",
        ],
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => &[
            "function_definition",
            "struct_specifier",
            "enum_specifier",
            "type_definition",
            "preproc_include",
            "declaration",
            "class_specifier",
            "namespace_definition",
            "template_declaration",
        ],
        "cs" => &[
            "class_declaration",
            "method_declaration",
            "namespace_declaration",
            "interface_declaration",
            "enum_declaration",
            "struct_declaration",
            "using_directive",
        ],
        "java" => &[
            "class_declaration",
            "method_declaration",
            "interface_declaration",
            "enum_declaration",
            "import_declaration",
            "package_declaration",
        ],
        "rb" => &["method", "class", "module", "assignment"],
        "php" => &[
            "function_definition",
            "class_declaration",
            "method_declaration",
            "namespace_definition",
            "use_declaration",
        ],
        "sh" | "bash" => &["function_definition", "command"],
        "kt" | "kts" => &[
            "function_declaration",
            "class_declaration",
            "object_declaration",
            "interface_declaration",
            "import_header",
        ],
        "swift" => &[
            "function_declaration",
            "class_declaration",
            "struct_declaration",
            "enum_declaration",
            "protocol_declaration",
            "import_declaration",
        ],
        "ex" | "exs" => &[
            "call",  // def, defp, defmodule, alias, import, use are all "call" nodes in Elixir
        ],
        "lua" => &["function_declaration", "local_function", "variable_declaration"],
        "hs" => &[
            "function",
            "type_synonym",
            "data_declaration",
            "class_declaration",
            "instance_declaration",
            "import_declaration",
        ],
        "html" | "htm" | "css" | "toml" | "yml" | "yaml" | "json" => &[],
        "ml" | "mli" => &[
            "let_binding",
            "type_definition",
            "module_definition",
            "open_statement",
        ],
        "dart" => &[
            "function_signature",
            "class_declaration",
            "method_signature",
            "import_or_export",
            "enum_declaration",
        ],
        _ => &[],
    }
}

fn is_import_kind(kind: &str) -> bool {
    matches!(
        kind,
        "use_declaration"
            | "import_statement"
            | "import_from_statement"
            | "import_declaration"
            | "using_directive"
            | "preproc_include"
            | "package_declaration"
            | "import_header"
            | "import_or_export"
            | "open_statement"
    )
}

/// Split `content` into AST-aware chunks. Returns an empty vec if the language
/// is unsupported or parsing fails (caller should fall back to regex chunker).
pub fn chunk_code_ast(content: &str, file_ext: &str) -> Vec<AstChunk> {
    let lang = match get_language(file_ext) {
        Some(l) => l,
        None => return Vec::new(),
    };

    let mut parser = Parser::new();
    if parser.set_language(&lang).is_err() {
        return Vec::new();
    }

    let source = content.as_bytes();
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let root = tree.root_node();
    let lines: Vec<&str> = content.lines().collect();
    let top_kinds = top_level_kinds(file_ext);

    let mut chunks: Vec<AstChunk> = Vec::new();
    let mut import_buf: Vec<Node> = Vec::new(); // pending consecutive imports

    let flush_imports = |buf: &mut Vec<Node>, chunks: &mut Vec<AstChunk>, source: &[u8], lines: &[&str]| {
        if buf.is_empty() {
            return;
        }
        let first = buf[0];
        let last = *buf.last().unwrap();
        let line_start = first.start_position().row + 1;
        let line_end = last.end_position().row + 1;
        let text = slice_lines(lines, line_start, line_end);
        if text.trim().len() >= MIN_CHUNK_CHARS {
            let name = extract_identifier(buf[0], source);
            chunks.push(AstChunk {
                content: text,
                line_start,
                line_end,
                chunk_type: "import_block".to_string(),
                name,
            });
        }
        buf.clear();
    };

    // Track regions not covered by top-level defs (module-level code)
    let mut prev_end_line = 1usize; // 1-indexed

    for i in 0..root.child_count() {
        let child = match root.child(i) {
            Some(c) => c,
            None => continue,
        };
        let kind = child.kind();

        if !top_kinds.contains(&kind) {
            // Not a boundary node — flush pending imports, then skip
            flush_imports(&mut import_buf, &mut chunks, source, &lines);
            continue;
        }

        let node_start_line = child.start_position().row + 1; // 1-indexed
        let node_end_line = child.end_position().row + 1;

        // Emit any module-level code that comes before this node
        if node_start_line > prev_end_line + 1 {
            let gap_text = slice_lines(&lines, prev_end_line, node_start_line - 1);
            if gap_text.trim().len() >= MIN_CHUNK_CHARS {
                chunks.push(AstChunk {
                    content: gap_text,
                    line_start: prev_end_line,
                    line_end: node_start_line - 1,
                    chunk_type: "module_level".to_string(),
                    name: None,
                });
            }
        }

        if is_import_kind(kind) {
            import_buf.push(child);
            prev_end_line = node_end_line + 1;
            continue;
        }

        // Flush any pending import block before a non-import definition
        flush_imports(&mut import_buf, &mut chunks, source, &lines);

        let name = extract_identifier(child, source);
        let chunk_type = kind_to_chunk_type(kind);
        let text = slice_lines(&lines, node_start_line, node_end_line);

        // If the node exceeds MAX_CHUNK_CHARS, split at child boundaries
        if text.len() > MAX_CHUNK_CHARS {
            let sub_chunks = split_large_node(child, source, &lines, &chunk_type, name.as_deref());
            chunks.extend(sub_chunks);
        } else if text.trim().len() >= MIN_CHUNK_CHARS {
            chunks.push(AstChunk {
                content: text,
                line_start: node_start_line,
                line_end: node_end_line,
                chunk_type,
                name,
            });
        }

        prev_end_line = node_end_line + 1;
    }

    // Flush any trailing imports
    flush_imports(&mut import_buf, &mut chunks, source, &lines);

    // Fallback: return the whole file as one chunk
    if chunks.is_empty() && !content.trim().is_empty() {
        chunks.push(AstChunk {
            content: content.to_string(),
            line_start: 1,
            line_end: lines.len().max(1),
            chunk_type: "module_level".to_string(),
            name: None,
        });
    }

    chunks
}

/// Split a node that exceeds MAX_CHUNK_CHARS by grouping its named children.
fn split_large_node(
    node: Node,
    _source: &[u8],
    lines: &[&str],
    parent_type: &str,
    parent_name: Option<&str>,
) -> Vec<AstChunk> {
    let mut result = Vec::new();
    let mut group_start: Option<usize> = None; // 1-indexed line
    let mut group_end = 0usize;
    let mut group_text = String::new();

    let flush_group = |start: usize, end: usize, text: &str, result: &mut Vec<AstChunk>| {
        if text.trim().len() >= MIN_CHUNK_CHARS {
            result.push(AstChunk {
                content: text.to_string(),
                line_start: start,
                line_end: end,
                chunk_type: parent_type.to_string(),
                name: parent_name.map(str::to_string),
            });
        }
    };

    for i in 0..node.named_child_count() {
        let child = match node.named_child(i) {
            Some(c) => c,
            None => continue,
        };
        let start_line = child.start_position().row + 1;
        let end_line = child.end_position().row + 1;
        let child_text = slice_lines(lines, start_line, end_line);

        if group_start.is_none() {
            group_start = Some(start_line);
        }

        group_text.push_str(&child_text);
        group_text.push('\n');
        group_end = end_line;

        if group_text.len() > MAX_CHUNK_CHARS {
            if let Some(gs) = group_start.take() {
                flush_group(gs, group_end, &group_text, &mut result);
            }
            group_text.clear();
        }
    }

    // Flush remainder
    if let Some(gs) = group_start {
        flush_group(gs, group_end, &group_text, &mut result);
    }

    // If no children split, just return the whole node truncated
    if result.is_empty() {
        let start = node.start_position().row + 1;
        let end = node.end_position().row + 1;
        let text = slice_lines(lines, start, end);
        if text.trim().len() >= MIN_CHUNK_CHARS {
            result.push(AstChunk {
                content: text,
                line_start: start,
                line_end: end,
                chunk_type: parent_type.to_string(),
                name: parent_name.map(str::to_string),
            });
        }
    }

    result
}

// ─── Entity extraction ────────────────────────────────────────────────────────

/// Extract code entities and relationships from `content` using tree-sitter.
/// Returns `(entities, relationships)`. Both vecs are empty if the language
/// is unsupported or parsing fails.
pub fn extract_code_entities(
    content: &str,
    file_ext: &str,
    file_path: &str,
) -> (Vec<CodeEntity>, Vec<CodeRelationship>) {
    let lang = match get_language(file_ext) {
        Some(l) => l,
        None => return (Vec::new(), Vec::new()),
    };

    let mut parser = Parser::new();
    if parser.set_language(&lang).is_err() {
        return (Vec::new(), Vec::new());
    }

    let source = content.as_bytes();
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return (Vec::new(), Vec::new()),
    };

    let lines: Vec<&str> = content.lines().collect();
    let root = tree.root_node();

    match file_ext {
        // Full per-language extractors
        "rs" => extract_rust(root, source, &lines, file_path),
        "py" => extract_python(root, source, &lines, file_path),
        "js" | "jsx" => extract_js(root, source, &lines, file_path),
        "ts" | "tsx" => extract_ts(root, source, &lines, file_path),
        "go" => extract_go(root, source, &lines, file_path),
        // Generic extractor for ts-core languages
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh" | "cs" | "java" | "rb" | "php"
        | "sh" | "bash" | "kt" | "kts" | "swift" | "ex" | "exs" | "lua" | "hs"
        | "html" | "htm" | "css" | "toml" | "yml" | "yaml" | "json" | "ml" | "mli"
        | "dart" => extract_generic(root, source, &lines, file_ext, file_path),
        _ => (Vec::new(), Vec::new()),
    }
}

// ─── Rust extraction ──────────────────────────────────────────────────────────

fn extract_rust<'a>(
    root: Node<'a>,
    source: &[u8],
    lines: &[&str],
    file_path: &str,
) -> (Vec<CodeEntity>, Vec<CodeRelationship>) {
    let mut entities = Vec::new();
    let mut rels = Vec::new();

    walk_rust_node(root, source, lines, file_path, None, &mut entities, &mut rels);

    (entities, rels)
}

fn walk_rust_node<'a>(
    node: Node<'a>,
    source: &[u8],
    lines: &[&str],
    file_path: &str,
    parent_name: Option<&str>,
    entities: &mut Vec<CodeEntity>,
    rels: &mut Vec<CodeRelationship>,
) {
    let kind = node.kind();

    match kind {
        "function_item" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                let sig = first_line(lines, start);
                entities.push(CodeEntity {
                    name: name.clone(),
                    entity_type: "function".to_string(),
                    description: sig,
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
                // If inside an impl, emit contains relationship
                if let Some(parent) = parent_name {
                    rels.push(CodeRelationship {
                        source: parent.to_string(),
                        target: name.clone(),
                        relation: "contains".to_string(),
                        file_path: file_path.to_string(),
                    });
                }
                // Walk body for calls
                extract_rust_calls(node, source, file_path, &name, rels);
            }
        }
        "struct_item" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                entities.push(CodeEntity {
                    name: name.clone(),
                    entity_type: "struct".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
                extract_rust_field_types(node, source, file_path, &name, rels);
            }
        }
        "enum_item" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                entities.push(CodeEntity {
                    name: name.clone(),
                    entity_type: "enum".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
            }
        }
        "trait_item" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                entities.push(CodeEntity {
                    name: name.clone(),
                    entity_type: "trait".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
                // Recurse into trait methods
                for i in 0..node.named_child_count() {
                    if let Some(c) = node.named_child(i) {
                        walk_rust_node(c, source, lines, file_path, Some(&name), entities, rels);
                    }
                }
            }
        }
        "impl_item" => {
            // impl SomeType / impl SomeTrait for SomeType
            let impl_name = get_impl_name(node, source);
            let trait_name = get_impl_trait(node, source);
            let start = node.start_position().row + 1;
            let end = node.end_position().row + 1;

            if let Some(ref iname) = impl_name {
                entities.push(CodeEntity {
                    name: iname.clone(),
                    entity_type: "impl".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
                if let Some(tname) = trait_name {
                    rels.push(CodeRelationship {
                        source: iname.clone(),
                        target: tname,
                        relation: "implements".to_string(),
                        file_path: file_path.to_string(),
                    });
                }
            }
            // Recurse into methods
            for i in 0..node.named_child_count() {
                if let Some(c) = node.named_child(i) {
                    walk_rust_node(
                        c,
                        source,
                        lines,
                        file_path,
                        impl_name.as_deref(),
                        entities,
                        rels,
                    );
                }
            }
        }
        "mod_item" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                entities.push(CodeEntity {
                    name: name.clone(),
                    entity_type: "module".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
            }
        }
        "type_item" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                entities.push(CodeEntity {
                    name: name.clone(),
                    entity_type: "type_alias".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
            }
        }
        "const_item" | "static_item" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                entities.push(CodeEntity {
                    name: name.clone(),
                    entity_type: "constant".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
            }
        }
        "use_declaration" => {
            // emit imports as relationships from file → imported symbol
            if let Ok(text) = node.utf8_text(source) {
                let imported = text
                    .trim_start_matches("use ")
                    .trim_end_matches(';')
                    .trim()
                    .to_string();
                if !imported.is_empty() {
                    rels.push(CodeRelationship {
                        source: file_path.to_string(),
                        target: imported,
                        relation: "imports".to_string(),
                        file_path: file_path.to_string(),
                    });
                }
            }
        }
        // Recurse into all other nodes at the root level
        "source_file" => {
            for i in 0..node.named_child_count() {
                if let Some(c) = node.named_child(i) {
                    walk_rust_node(c, source, lines, file_path, None, entities, rels);
                }
            }
        }
        _ => {}
    }
}

/// Extract function call sites inside a function body.
fn extract_rust_calls(
    fn_node: Node,
    source: &[u8],
    file_path: &str,
    caller: &str,
    rels: &mut Vec<CodeRelationship>,
) {
    visit_all(fn_node, &mut |n| {
        if n.kind() == "call_expression" {
            let callee = get_call_callee(n, source);
            if let Some(callee_name) = callee {
                rels.push(CodeRelationship {
                    source: caller.to_string(),
                    target: callee_name,
                    relation: "calls".to_string(),
                    file_path: file_path.to_string(),
                });
            }
        }
    });
}

/// Extract `uses_type` relationships from struct fields.
fn extract_rust_field_types(
    struct_node: Node,
    source: &[u8],
    file_path: &str,
    struct_name: &str,
    rels: &mut Vec<CodeRelationship>,
) {
    visit_all(struct_node, &mut |n| {
        if n.kind() == "type_identifier"
            && let Ok(text) = n.utf8_text(source) {
                let t = text.trim().to_string();
                // Skip primitive-like type names that are all lowercase
                if t.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    rels.push(CodeRelationship {
                        source: struct_name.to_string(),
                        target: t,
                        relation: "uses_type".to_string(),
                        file_path: file_path.to_string(),
                    });
                }
            }
    });
}

// ─── Python extraction ────────────────────────────────────────────────────────

fn extract_python<'a>(
    root: Node<'a>,
    source: &[u8],
    lines: &[&str],
    file_path: &str,
) -> (Vec<CodeEntity>, Vec<CodeRelationship>) {
    let mut entities = Vec::new();
    let mut rels = Vec::new();

    for i in 0..root.named_child_count() {
        let child = match root.named_child(i) {
            Some(c) => c,
            None => continue,
        };
        extract_python_node(child, source, lines, file_path, None, &mut entities, &mut rels);
    }

    (entities, rels)
}

fn extract_python_node<'a>(
    node: Node<'a>,
    source: &[u8],
    lines: &[&str],
    file_path: &str,
    parent_name: Option<&str>,
    entities: &mut Vec<CodeEntity>,
    rels: &mut Vec<CodeRelationship>,
) {
    match node.kind() {
        "function_definition" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                entities.push(CodeEntity {
                    name: name.clone(),
                    entity_type: "function".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
                if let Some(parent) = parent_name {
                    rels.push(CodeRelationship {
                        source: parent.to_string(),
                        target: name.clone(),
                        relation: "contains".to_string(),
                        file_path: file_path.to_string(),
                    });
                }
                // Walk body for calls
                extract_python_calls(node, source, file_path, &name, rels);
            }
        }
        "class_definition" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                entities.push(CodeEntity {
                    name: name.clone(),
                    entity_type: "class".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
                // Base classes → extends
                extract_python_bases(node, source, file_path, &name, rels);
                // Recurse into body for methods
                for i in 0..node.named_child_count() {
                    if let Some(c) = node.named_child(i) {
                        extract_python_node(c, source, lines, file_path, Some(&name), entities, rels);
                    }
                }
            }
        }
        "decorated_definition" => {
            // Recurse into the actual definition
            for i in 0..node.named_child_count() {
                if let Some(c) = node.named_child(i)
                    && (c.kind() == "function_definition" || c.kind() == "class_definition") {
                        extract_python_node(c, source, lines, file_path, parent_name, entities, rels);
                    }
            }
        }
        "import_statement" | "import_from_statement" => {
            if let Ok(text) = node.utf8_text(source) {
                let imported = text.trim().to_string();
                if !imported.is_empty() {
                    rels.push(CodeRelationship {
                        source: file_path.to_string(),
                        target: imported,
                        relation: "imports".to_string(),
                        file_path: file_path.to_string(),
                    });
                }
            }
        }
        _ => {}
    }
}

fn extract_python_calls(
    node: Node,
    source: &[u8],
    file_path: &str,
    caller: &str,
    rels: &mut Vec<CodeRelationship>,
) {
    visit_all(node, &mut |n| {
        if n.kind() == "call"
            && let Some(func) = n.child_by_field_name("function")
                && let Ok(name) = func.utf8_text(source) {
                    let name = name.trim().to_string();
                    if !name.is_empty() && !name.contains('\n') {
                        rels.push(CodeRelationship {
                            source: caller.to_string(),
                            target: name,
                            relation: "calls".to_string(),
                            file_path: file_path.to_string(),
                        });
                    }
                }
    });
}

fn extract_python_bases(
    class_node: Node,
    source: &[u8],
    file_path: &str,
    class_name: &str,
    rels: &mut Vec<CodeRelationship>,
) {
    for i in 0..class_node.named_child_count() {
        let child = match class_node.named_child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "argument_list" {
            for j in 0..child.named_child_count() {
                if let Some(base) = child.named_child(j)
                    && let Ok(base_name) = base.utf8_text(source) {
                        let base_name = base_name.trim().to_string();
                        if !base_name.is_empty() {
                            rels.push(CodeRelationship {
                                source: class_name.to_string(),
                                target: base_name,
                                relation: "extends".to_string(),
                                file_path: file_path.to_string(),
                            });
                        }
                    }
            }
        }
    }
}

// ─── JavaScript/TypeScript extraction ────────────────────────────────────────

fn extract_js<'a>(
    root: Node<'a>,
    source: &[u8],
    lines: &[&str],
    file_path: &str,
) -> (Vec<CodeEntity>, Vec<CodeRelationship>) {
    let mut entities = Vec::new();
    let mut rels = Vec::new();

    for i in 0..root.named_child_count() {
        let child = match root.named_child(i) {
            Some(c) => c,
            None => continue,
        };
        extract_js_node(child, source, lines, file_path, &mut entities, &mut rels);
    }

    (entities, rels)
}

fn extract_ts<'a>(
    root: Node<'a>,
    source: &[u8],
    lines: &[&str],
    file_path: &str,
) -> (Vec<CodeEntity>, Vec<CodeRelationship>) {
    // TypeScript parsing is the same as JS at the entity level
    extract_js(root, source, lines, file_path)
}

fn extract_js_node<'a>(
    node: Node<'a>,
    source: &[u8],
    lines: &[&str],
    file_path: &str,
    entities: &mut Vec<CodeEntity>,
    rels: &mut Vec<CodeRelationship>,
) {
    match node.kind() {
        "function_declaration" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                entities.push(CodeEntity {
                    name: name.clone(),
                    entity_type: "function".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
                extract_js_calls(node, source, file_path, &name, rels);
            }
        }
        "class_declaration" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                entities.push(CodeEntity {
                    name: name.clone(),
                    entity_type: "class".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
            }
        }
        "export_statement" => {
            // Recurse into the exported declaration
            for i in 0..node.named_child_count() {
                if let Some(c) = node.named_child(i) {
                    extract_js_node(c, source, lines, file_path, entities, rels);
                }
            }
        }
        "import_statement" => {
            if let Ok(text) = node.utf8_text(source) {
                rels.push(CodeRelationship {
                    source: file_path.to_string(),
                    target: text.trim().to_string(),
                    relation: "imports".to_string(),
                    file_path: file_path.to_string(),
                });
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            // Top-level const/let/var — emit as a constant entity if named
            if let Some(declarator) = node.named_child(0)
                && let Some(name) = get_child_text(declarator, "name", source) {
                    let start = node.start_position().row + 1;
                    let end = node.end_position().row + 1;
                    entities.push(CodeEntity {
                        name,
                        entity_type: "constant".to_string(),
                        description: first_line(lines, start),
                        file_path: file_path.to_string(),
                        line_start: start,
                        line_end: end,
                    });
                }
        }
        "interface_declaration" | "type_alias_declaration" => {
            if let Some(name) = get_child_text(node, "name", source) {
                let start = node.start_position().row + 1;
                let end = node.end_position().row + 1;
                entities.push(CodeEntity {
                    name,
                    entity_type: "type_alias".to_string(),
                    description: first_line(lines, start),
                    file_path: file_path.to_string(),
                    line_start: start,
                    line_end: end,
                });
            }
        }
        _ => {}
    }
}

fn extract_js_calls(
    node: Node,
    source: &[u8],
    file_path: &str,
    caller: &str,
    rels: &mut Vec<CodeRelationship>,
) {
    visit_all(node, &mut |n| {
        if n.kind() == "call_expression" {
            let callee = get_call_callee(n, source);
            if let Some(callee_name) = callee {
                rels.push(CodeRelationship {
                    source: caller.to_string(),
                    target: callee_name,
                    relation: "calls".to_string(),
                    file_path: file_path.to_string(),
                });
            }
        }
    });
}

// ─── Go extraction ────────────────────────────────────────────────────────────

fn extract_go<'a>(
    root: Node<'a>,
    source: &[u8],
    lines: &[&str],
    file_path: &str,
) -> (Vec<CodeEntity>, Vec<CodeRelationship>) {
    let mut entities = Vec::new();
    let mut rels = Vec::new();

    for i in 0..root.named_child_count() {
        let child = match root.named_child(i) {
            Some(c) => c,
            None => continue,
        };
        let kind = child.kind();
        let start = child.start_position().row + 1;
        let end = child.end_position().row + 1;

        match kind {
            "function_declaration" | "method_declaration" => {
                if let Some(name) = get_child_text(child, "name", source) {
                    entities.push(CodeEntity {
                        name: name.clone(),
                        entity_type: "function".to_string(),
                        description: first_line(lines, start),
                        file_path: file_path.to_string(),
                        line_start: start,
                        line_end: end,
                    });
                    // Extract calls
                    visit_all(child, &mut |n| {
                        if n.kind() == "call_expression"
                            && let Some(callee) = get_call_callee(n, source)
                        {
                            rels.push(CodeRelationship {
                                source: name.clone(),
                                target: callee,
                                relation: "calls".to_string(),
                                file_path: file_path.to_string(),
                            });
                        }
                    });
                }
            }
            "type_declaration" => {
                // Walk named children to find type_spec
                for i in 0..child.named_child_count() {
                    if let Some(spec) = child.named_child(i)
                        && spec.kind() == "type_spec"
                        && let Some(name) = get_child_text(spec, "name", source)
                    {
                        entities.push(CodeEntity {
                            name,
                            entity_type: "type_alias".to_string(),
                            description: first_line(lines, start),
                            file_path: file_path.to_string(),
                            line_start: start,
                            line_end: end,
                        });
                    }
                }
            }
            "import_declaration" => {
                if let Ok(text) = child.utf8_text(source) {
                    rels.push(CodeRelationship {
                        source: file_path.to_string(),
                        target: text.trim().to_string(),
                        relation: "imports".to_string(),
                        file_path: file_path.to_string(),
                    });
                }
            }
            _ => {}
        }
    }

    (entities, rels)
}

// ─── Generic extraction ───────────────────────────────────────────────────────

/// Generic entity extractor for languages without a hand-written extractor.
///
/// Walks the top-level nodes matching `top_level_kinds()` for the given
/// extension, pulls out the identifier via the "name" field (or first
/// identifier child), maps the node kind to an entity type, and records
/// import relationships from import-like nodes.
fn extract_generic<'a>(
    root: Node<'a>,
    source: &[u8],
    lines: &[&str],
    file_ext: &str,
    file_path: &str,
) -> (Vec<CodeEntity>, Vec<CodeRelationship>) {
    let mut entities = Vec::new();
    let mut rels = Vec::new();
    let top_kinds = top_level_kinds(file_ext);

    for i in 0..root.named_child_count() {
        let child = match root.named_child(i) {
            Some(c) => c,
            None => continue,
        };
        let kind = child.kind();
        if !top_kinds.contains(&kind) {
            continue;
        }

        let start = child.start_position().row + 1;
        let end = child.end_position().row + 1;

        // Emit import relationships for import-like nodes
        if is_import_kind(kind) {
            if let Ok(text) = child.utf8_text(source) {
                let t = text.trim().to_string();
                if !t.is_empty() {
                    rels.push(CodeRelationship {
                        source: file_path.to_string(),
                        target: t,
                        relation: "imports".to_string(),
                        file_path: file_path.to_string(),
                    });
                }
            }
            continue;
        }

        // Try to find the entity name:
        // 1. "name" field directly on the node (Java, C#, Go, Ruby, etc.)
        // 2. Recursively follow "declarator" fields (C, C++ style)
        // 3. First named child whose kind ends with "identifier"
        let name = get_child_text(child, "name", source)
            .or_else(|| find_name_in_declarator(child, source))
            .or_else(|| {
                (0..child.named_child_count()).find_map(|j| {
                    let nc = child.named_child(j)?;
                    if nc.kind().ends_with("identifier") {
                        nc.utf8_text(source)
                            .ok()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                    } else {
                        None
                    }
                })
            });

        if let Some(name) = name {
            let entity_type = generic_kind_to_entity_type(kind);
            entities.push(CodeEntity {
                name,
                entity_type,
                description: first_line(lines, start),
                file_path: file_path.to_string(),
                line_start: start,
                line_end: end,
            });
        }
    }

    (entities, rels)
}

/// Map a tree-sitter node kind to a simple entity type string for the generic extractor.
fn generic_kind_to_entity_type(kind: &str) -> String {
    match kind {
        k if k.contains("function") || k.contains("method") || k == "def" || k == "defp" => {
            "function"
        }
        k if k.contains("class") => "class",
        k if k.contains("interface") => "interface",
        k if k.contains("struct") => "struct",
        k if k.contains("enum") => "enum",
        k if k.contains("namespace") || k.contains("module") || k == "defmodule" => "module",
        k if k.contains("template") => "template",
        k if k.contains("type") => "type_alias",
        k if k.contains("protocol") => "protocol",
        k if k.contains("object") => "object",
        _ => "declaration",
    }
    .to_string()
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Walk all descendants (DFS) and call `visitor` on each node.
fn visit_all<F>(node: Node, visitor: &mut F)
where
    F: FnMut(Node),
{
    visitor(node);
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            visit_all(cursor.node(), visitor);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// Get the text of the named field `field_name` within `node`.
fn get_child_text(node: Node, field_name: &str, source: &[u8]) -> Option<String> {
    node.child_by_field_name(field_name)
        .and_then(|n| n.utf8_text(source).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// For `impl Foo` or `impl Bar for Foo`, return `"Foo"`.
fn get_impl_name(node: Node, source: &[u8]) -> Option<String> {
    // The type being impl'd is the first named child that is a type identifier
    for i in 0..node.named_child_count() {
        let child = node.named_child(i)?;
        if matches!(child.kind(), "type_identifier" | "generic_type") {
            return child.utf8_text(source).ok().map(|s| s.trim().to_string());
        }
    }
    None
}

/// For `impl TraitName for Foo`, return `Some("TraitName")`.
fn get_impl_trait(node: Node, source: &[u8]) -> Option<String> {
    // Look for "for" keyword; the type before it is the trait
    let mut found_for = false;
    for i in 0..node.child_count() {
        let child = node.child(i)?;
        if child.kind() == "for" {
            found_for = true;
            continue;
        }
        if !found_for && matches!(child.kind(), "type_identifier" | "generic_type") {
            // First type identifier before "for" = the trait
            // But we need to find the FIRST type id, then the second one after "for"
        }
    }
    if !found_for {
        return None;
    }
    // When there's "for", the structure is: impl <TraitType> for <ImplType>
    // The trait is the named child before the "for" keyword
    // We iterate named children to find two type nodes
    let mut type_nodes: Vec<Node> = Vec::new();
    for i in 0..node.named_child_count() {
        let child = node.named_child(i)?;
        if matches!(child.kind(), "type_identifier" | "generic_type") {
            type_nodes.push(child);
            if type_nodes.len() == 2 {
                break;
            }
        }
    }
    if type_nodes.len() >= 2 {
        // First type is the trait
        type_nodes[0].utf8_text(source).ok().map(|s| s.trim().to_string())
    } else {
        None
    }
}

/// Get the callee name from a call expression node.
fn get_call_callee(node: Node, source: &[u8]) -> Option<String> {
    // call_expression has a "function" field in Rust/JS or first child in others
    if let Some(func) = node.child_by_field_name("function")
        && let Ok(text) = func.utf8_text(source) {
            let name = text.trim();
            // Only simple identifiers or method calls (no newlines)
            if !name.is_empty() && !name.contains('\n') && name.len() < 100 {
                // Strip method calls: `foo.bar(` → take the last segment
                let last = name.split('.').next_back().unwrap_or(name);
                let last = last.split("::").last().unwrap_or(last);
                if !last.is_empty() {
                    return Some(last.to_string());
                }
            }
        }
    // Fallback: first named child
    if let Some(first) = node.named_child(0)
        && let Ok(text) = first.utf8_text(source) {
            let t = text.trim();
            if !t.is_empty() && !t.contains('\n') && t.len() < 100 {
                return Some(t.split('.').next_back().unwrap_or(t).to_string());
            }
        }
    None
}

/// Extract text of lines `start..=end` (1-indexed).
fn slice_lines(lines: &[&str], start: usize, end: usize) -> String {
    let start = start.saturating_sub(1); // convert to 0-indexed
    let end = end.min(lines.len());
    lines[start..end].join("\n")
}

/// Return the first line (1-indexed) as a string.
fn first_line(lines: &[&str], line_1indexed: usize) -> String {
    lines
        .get(line_1indexed.saturating_sub(1))
        .map(|l| l.trim().to_string())
        .unwrap_or_default()
}

/// Recursively follow "declarator" fields to find an identifier name.
///
/// Used for C/C++ function definitions where the name is nested:
/// `function_definition → declarator (function_declarator) → declarator (identifier)`
fn find_name_in_declarator(node: Node, source: &[u8]) -> Option<String> {
    let mut current = node.child_by_field_name("declarator")?;
    // Descend through up to 4 levels of declarator nesting
    for _ in 0..4 {
        if current.kind() == "identifier" || current.kind() == "field_identifier" {
            return current
                .utf8_text(source)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
        match current.child_by_field_name("declarator") {
            Some(next) => current = next,
            None => break,
        }
    }
    None
}

/// Try to extract an identifier from a node (best-effort for import grouping).
fn extract_identifier(node: Node, source: &[u8]) -> Option<String> {
    // Try field "name" first
    if let Some(n) = node.child_by_field_name("name")
        && let Ok(text) = n.utf8_text(source) {
            let s = text.trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    None
}

/// Map a tree-sitter node kind to a human-readable chunk_type string.
fn kind_to_chunk_type(kind: &str) -> String {
    match kind {
        "function_item"
        | "function_definition"
        | "function_declaration"
        | "method_declaration"
        | "method"
        | "local_function"
        | "function_signature"
        | "method_signature" => "function",
        "impl_item" => "impl",
        "struct_item" | "struct_specifier" | "struct_declaration" => "struct",
        "enum_item" | "enum_declaration" | "enum_specifier" => "enum",
        "trait_item" => "trait",
        "mod_item" | "module" | "namespace_definition" | "namespace_declaration" => "module",
        "class_definition" | "class_declaration" | "class_specifier" | "object_declaration" => {
            "class"
        }
        "type_item"
        | "type_alias_declaration"
        | "type_declaration"
        | "interface_declaration"
        | "type_definition"
        | "type_synonym"
        | "data_declaration"
        | "protocol_declaration" => "type_alias",
        "const_item"
        | "static_item"
        | "const_declaration"
        | "var_declaration"
        | "lexical_declaration"
        | "variable_declaration"
        | "assignment"
        | "local_variable_declaration" => "constant",
        "macro_definition" => "macro",
        "export_statement" => "export",
        "template_declaration" => "template",
        "use_declaration"
        | "import_statement"
        | "import_from_statement"
        | "import_declaration"
        | "using_directive"
        | "preproc_include"
        | "package_declaration"
        | "import_header"
        | "import_or_export"
        | "open_statement" => "import",
        _ => "module_level",
    }
    .to_string()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Language detection ───────────────────────────────────────────────────

    #[test]
    fn test_get_language_rust() {
        assert!(get_language("rs").is_some());
    }

    #[test]
    fn test_get_language_python() {
        assert!(get_language("py").is_some());
    }

    #[test]
    fn test_get_language_js() {
        assert!(get_language("js").is_some());
    }

    #[test]
    fn test_get_language_ts() {
        assert!(get_language("ts").is_some());
    }

    #[test]
    fn test_get_language_unsupported() {
        assert!(get_language("xyz").is_none());
        assert!(get_language("").is_none());
    }

    #[test]
    fn test_get_language_c() {
        assert!(get_language("c").is_some());
        assert!(get_language("h").is_some());
    }

    #[test]
    fn test_get_language_cpp() {
        assert!(get_language("cpp").is_some());
        assert!(get_language("hpp").is_some());
    }

    #[test]
    fn test_get_language_java() {
        assert!(get_language("java").is_some());
    }

    #[test]
    fn test_get_language_ruby() {
        assert!(get_language("rb").is_some());
    }

    #[test]
    fn test_get_language_go() {
        assert!(get_language("go").is_some());
    }

    // ─── Rust AST chunking ────────────────────────────────────────────────────

    const RUST_SAMPLE: &str = r#"use std::fmt;
use std::collections::HashMap;

/// A point in 2D space.
#[derive(Debug, Clone)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance(&self, other: &Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

pub fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}

pub enum Direction {
    North,
    South,
    East,
    West,
}
"#;

    #[test]
    fn test_rust_chunking_produces_chunks() {
        let chunks = chunk_code_ast(RUST_SAMPLE, "rs");
        assert!(!chunks.is_empty(), "Expected non-empty chunks for Rust sample");
    }

    #[test]
    fn test_rust_chunking_finds_import_block() {
        let chunks = chunk_code_ast(RUST_SAMPLE, "rs");
        let has_import = chunks.iter().any(|c| c.chunk_type == "import_block");
        assert!(has_import, "Expected an import_block chunk for use declarations");
    }

    #[test]
    fn test_rust_chunking_finds_struct() {
        let chunks = chunk_code_ast(RUST_SAMPLE, "rs");
        let has_struct = chunks.iter().any(|c| c.chunk_type == "struct");
        assert!(has_struct, "Expected a struct chunk");
    }

    #[test]
    fn test_rust_chunking_finds_impl() {
        let chunks = chunk_code_ast(RUST_SAMPLE, "rs");
        let has_impl = chunks.iter().any(|c| c.chunk_type == "impl");
        assert!(has_impl, "Expected an impl chunk");
    }

    #[test]
    fn test_rust_chunking_finds_function() {
        let chunks = chunk_code_ast(RUST_SAMPLE, "rs");
        let has_fn = chunks.iter().any(|c| c.chunk_type == "function");
        assert!(has_fn, "Expected a function chunk");
    }

    #[test]
    fn test_rust_chunking_line_numbers_valid() {
        let chunks = chunk_code_ast(RUST_SAMPLE, "rs");
        for chunk in &chunks {
            assert!(chunk.line_start >= 1, "line_start must be >= 1");
            assert!(
                chunk.line_end >= chunk.line_start,
                "line_end must be >= line_start"
            );
        }
    }

    // ─── Python AST chunking ──────────────────────────────────────────────────

    const PYTHON_SAMPLE: &str = r#"import os
import sys
from pathlib import Path

class Greeter:
    """A simple greeter class."""

    def __init__(self, name: str):
        self.name = name

    def greet(self) -> str:
        return f"Hello, {self.name}!"


def standalone_function(x: int, y: int) -> int:
    """Add two numbers."""
    return x + y


CONSTANT = 42
"#;

    #[test]
    fn test_python_chunking_produces_chunks() {
        let chunks = chunk_code_ast(PYTHON_SAMPLE, "py");
        assert!(!chunks.is_empty(), "Expected non-empty chunks for Python sample");
    }

    #[test]
    fn test_python_chunking_finds_class() {
        let chunks = chunk_code_ast(PYTHON_SAMPLE, "py");
        let has_class = chunks.iter().any(|c| c.chunk_type == "class");
        assert!(has_class, "Expected a class chunk");
    }

    #[test]
    fn test_python_chunking_finds_function() {
        let chunks = chunk_code_ast(PYTHON_SAMPLE, "py");
        let has_fn = chunks.iter().any(|c| c.chunk_type == "function");
        assert!(has_fn, "Expected a function chunk");
    }

    #[test]
    fn test_python_chunking_finds_import_block() {
        let chunks = chunk_code_ast(PYTHON_SAMPLE, "py");
        let has_import = chunks.iter().any(|c| c.chunk_type == "import_block");
        assert!(has_import, "Expected an import_block chunk");
    }

    // ─── Entity extraction ────────────────────────────────────────────────────

    #[test]
    fn test_rust_entity_extraction_finds_struct() {
        let (entities, _) = extract_code_entities(RUST_SAMPLE, "rs", "src/point.rs");
        let struct_names: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == "struct")
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            struct_names.contains(&"Point"),
            "Expected 'Point' struct entity, got: {:?}",
            struct_names
        );
    }

    #[test]
    fn test_rust_entity_extraction_finds_function() {
        let (entities, _) = extract_code_entities(RUST_SAMPLE, "rs", "src/point.rs");
        let fn_names: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == "function")
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            fn_names.contains(&"greet"),
            "Expected 'greet' function entity, got: {:?}",
            fn_names
        );
    }

    #[test]
    fn test_rust_entity_extraction_finds_enum() {
        let (entities, _) = extract_code_entities(RUST_SAMPLE, "rs", "src/point.rs");
        let enum_names: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == "enum")
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            enum_names.contains(&"Direction"),
            "Expected 'Direction' enum entity, got: {:?}",
            enum_names
        );
    }

    #[test]
    fn test_rust_entity_extraction_import_relationships() {
        let (_, rels) = extract_code_entities(RUST_SAMPLE, "rs", "src/point.rs");
        let imports: Vec<&str> = rels
            .iter()
            .filter(|r| r.relation == "imports")
            .map(|r| r.target.as_str())
            .collect();
        assert!(!imports.is_empty(), "Expected import relationships");
    }

    #[test]
    fn test_python_entity_extraction_finds_class() {
        let (entities, _) = extract_code_entities(PYTHON_SAMPLE, "py", "src/greeter.py");
        let classes: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == "class")
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            classes.contains(&"Greeter"),
            "Expected 'Greeter' class entity"
        );
    }

    #[test]
    fn test_python_entity_extraction_finds_function() {
        let (entities, _) = extract_code_entities(PYTHON_SAMPLE, "py", "src/greeter.py");
        let fns: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == "function")
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            fns.contains(&"standalone_function"),
            "Expected 'standalone_function' entity, got: {:?}",
            fns
        );
    }

    #[test]
    fn test_unsupported_extension_returns_empty() {
        let (entities, rels) = extract_code_entities("x = 1\n", "xyz", "src/Main.xyz");
        assert!(entities.is_empty());
        assert!(rels.is_empty());

        let chunks = chunk_code_ast("x = 1\n", "xyz");
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_fallback_for_unsupported_ext() {
        use crate::chunk::chunk_file;
        use crate::config::KbType;
        // Unknown extension -> chunk_file should fall back to regex-based chunker
        let content = "some unknown content here that should be handled gracefully\n";
        let chunks = chunk_file("src/Foo.unknownext", content, Some(&KbType::Code));
        // Should not panic
        let _ = chunks;
    }

    // ─── C language tests ─────────────────────────────────────────────────────

    const C_SAMPLE: &str = r#"#include <stdio.h>
#include <stdlib.h>

typedef struct {
    int x;
    int y;
} Point;

int add(int a, int b) {
    return a + b;
}

void greet(const char *name) {
    printf("Hello, %s!\n", name);
}
"#;

    #[test]
    fn test_c_language_detected() {
        assert!(get_language("c").is_some());
        assert!(get_language("h").is_some());
    }

    #[test]
    fn test_c_chunking_produces_chunks() {
        let chunks = chunk_code_ast(C_SAMPLE, "c");
        assert!(!chunks.is_empty(), "Expected non-empty chunks for C sample");
    }

    #[test]
    fn test_c_entity_extraction_finds_function() {
        let (entities, _) = extract_code_entities(C_SAMPLE, "c", "src/main.c");
        let fn_names: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == "function")
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            fn_names.contains(&"add") || fn_names.contains(&"greet"),
            "Expected 'add' or 'greet' function entity, got: {:?}",
            fn_names
        );
    }

    // ─── Java language tests ──────────────────────────────────────────────────

    const JAVA_SAMPLE: &str = r#"package com.example;

import java.util.List;
import java.util.ArrayList;

public class Greeter {
    private String name;

    public Greeter(String name) {
        this.name = name;
    }

    public String greet() {
        return "Hello, " + name + "!";
    }
}
"#;

    #[test]
    fn test_java_language_detected() {
        assert!(get_language("java").is_some());
    }

    #[test]
    fn test_java_chunking_produces_chunks() {
        let chunks = chunk_code_ast(JAVA_SAMPLE, "java");
        assert!(!chunks.is_empty(), "Expected non-empty chunks for Java sample");
    }

    #[test]
    fn test_java_entity_extraction_finds_class() {
        let (entities, _) = extract_code_entities(JAVA_SAMPLE, "java", "src/Greeter.java");
        let class_names: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == "class")
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            class_names.contains(&"Greeter"),
            "Expected 'Greeter' class entity, got: {:?}",
            class_names
        );
    }

    // ─── Ruby language tests ──────────────────────────────────────────────────

    const RUBY_SAMPLE: &str = r#"require 'json'

class Greeter
  def initialize(name)
    @name = name
  end

  def greet
    "Hello, #{@name}!"
  end
end

def standalone_hello(name)
  puts "Hello, #{name}!"
end
"#;

    #[test]
    fn test_ruby_language_detected() {
        assert!(get_language("rb").is_some());
    }

    #[test]
    fn test_ruby_chunking_produces_chunks() {
        let chunks = chunk_code_ast(RUBY_SAMPLE, "rb");
        assert!(!chunks.is_empty(), "Expected non-empty chunks for Ruby sample");
    }

    #[test]
    fn test_ruby_entity_extraction_finds_class() {
        let (entities, _) = extract_code_entities(RUBY_SAMPLE, "rb", "src/greeter.rb");
        let class_names: Vec<&str> = entities
            .iter()
            .filter(|e| e.entity_type == "class")
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            class_names.contains(&"Greeter"),
            "Expected 'Greeter' class entity, got: {:?}",
            class_names
        );
    }
}
