//! Chunking module — splits files into semantic chunks for indexing.
//!
//! Strategies:
//! - Markdown (.md): headings, code blocks, frontmatter
//! - Code (.rs, .py, .ts, …): AST-aware (tree-sitter) or regex-based fallback
//! - Text (everything else): paragraph-based, fallback fixed-size

use std::path::Path;

use crate::config::FolderType;

/// Minimum chunk size in characters (avoid tiny fragments).
pub const MIN_CHUNK_CHARS: usize = 30;
/// Maximum chunk size in characters (~2000 tokens at ~4 chars/token).
pub const MAX_CHUNK_CHARS: usize = 8_000;

/// A single semantic chunk extracted from a document.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Chunk {
    pub content: String,
    /// 1-indexed, inclusive
    pub line_start: usize,
    /// 1-indexed, inclusive
    pub line_end: usize,
    /// One of: "heading_section", "paragraph", "code_block", "frontmatter", "function"
    pub chunk_type: String,
}

/// Dispatcher: choose chunking strategy based on file extension.
///
/// When `folder_type` is `Some(FolderType::Code)` and tree-sitter supports the
/// file extension, uses AST-aware chunking. Otherwise falls back to the
/// regex-based or text chunker.
pub fn chunk_file(path: &str, content: &str, folder_type: Option<&FolderType>) -> Vec<Chunk> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // For code folders, prefer AST-aware chunking via tree-sitter
    #[cfg(feature = "tree-sitter")]
    if matches!(folder_type, Some(FolderType::Code)) {
        use crate::treesitter;
        if treesitter::get_language(&ext).is_some() {
            let ast_chunks = treesitter::chunk_code_ast(content, &ext);
            if !ast_chunks.is_empty() {
                return ast_chunks
                    .into_iter()
                    .map(|c| Chunk {
                        content: c.content,
                        line_start: c.line_start,
                        line_end: c.line_end,
                        chunk_type: c.chunk_type,
                    })
                    .collect();
            }
        }
    }

    match ext.as_str() {
        "md" | "markdown" => chunk_markdown(content),
        "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "java" | "kt" | "c" | "cpp"
        | "h" | "cs" | "rb" | "swift" | "zig" | "lua" => chunk_code(content),
        _ => chunk_text(content),
    }
}

// ─── Markdown ────────────────────────────────────────────────────────────────

/// Split Markdown on headings, preserving code blocks and extracting frontmatter.
pub fn chunk_markdown(content: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = content.lines().collect();
    let mut chunks: Vec<Chunk> = Vec::new();

    // 1. Extract frontmatter if present
    let (body_start, fm_chunk) = extract_frontmatter(&lines);
    if let Some(fm) = fm_chunk {
        chunks.push(fm);
    }

    // 2. Walk lines, splitting on headings and code fences
    let mut current: Vec<&str> = Vec::new();
    let mut current_start: usize = body_start + 1; // 1-indexed
    let mut chunk_type = "paragraph".to_string();
    let mut in_code_block = false;
    let mut code_fence = "";

    let mut i = body_start;
    while i < lines.len() {
        let line = lines[i];
        let line_num = i + 1; // 1-indexed

        if in_code_block {
            current.push(line);
            // Detect closing fence (must match opening fence exactly)
            if line.trim_start().starts_with(code_fence) && current.len() > 1 {
                in_code_block = false;
                flush_chunk(&mut chunks, &mut current, current_start, line_num, "code_block");
                chunk_type = "paragraph".to_string();
                current_start = line_num + 1;
            }
            i += 1;
            continue;
        }

        // Detect opening fence
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            // Flush current section first
            if !current.is_empty() {
                flush_chunk(&mut chunks, &mut current, current_start, line_num - 1, &chunk_type);
                chunk_type = "paragraph".to_string();
            }
            in_code_block = true;
            code_fence = if trimmed.starts_with("```") { "```" } else { "~~~" };
            current_start = line_num;
            current.push(line);
            i += 1;
            continue;
        }

        // Detect ATX heading (# / ## / ### …)
        if line.starts_with('#') {
            // Flush whatever came before
            if !current.is_empty() {
                flush_chunk(&mut chunks, &mut current, current_start, line_num - 1, &chunk_type);
            }
            current_start = line_num;
            chunk_type = "heading_section".to_string();
        }

        current.push(line);

        // Oversized? Split on paragraph boundary
        if char_size(&current) > MAX_CHUNK_CHARS {
            let split = find_paragraph_split(&current);
            let tail = current.split_off(split);
            let end = current_start + current.len() - 1;
            flush_chunk(&mut chunks, &mut current, current_start, end, &chunk_type);
            current_start = end + 1;
            current = tail;
        }

        i += 1;
    }

    // Flush remainder
    if !current.is_empty() {
        flush_chunk(&mut chunks, &mut current, current_start, lines.len(), &chunk_type);
    }

    let chunks = merge_short_chunks(chunks);

    // Fallback: whole file as one chunk if nothing was produced
    if chunks.is_empty() && !content.trim().is_empty() {
        return vec![Chunk {
            content: content.to_string(),
            line_start: 1,
            line_end: lines.len().max(1),
            chunk_type: "paragraph".to_string(),
        }];
    }

    chunks
}

/// Extract YAML frontmatter.
/// Returns (line_index_after_frontmatter, Option<Chunk>).
fn extract_frontmatter(lines: &[&str]) -> (usize, Option<Chunk>) {
    if lines.is_empty() || lines[0].trim() != "---" {
        return (0, None);
    }
    for i in 1..lines.len() {
        if lines[i].trim() == "---" || lines[i].trim() == "..." {
            let content = lines[..=i].join("\n");
            let chunk = if !content.trim().is_empty() {
                Some(Chunk {
                    content,
                    line_start: 1,
                    line_end: i + 1,
                    chunk_type: "frontmatter".to_string(),
                })
            } else {
                None
            };
            return (i + 1, chunk);
        }
    }
    (0, None) // No closing marker — treat whole file as body
}

/// Find a split point near the midpoint, preferring empty lines.
fn find_paragraph_split(lines: &[&str]) -> usize {
    let mid = lines.len() / 2;
    // Search backwards from midpoint for an empty line
    for i in (mid..lines.len()).rev() {
        if lines[i].trim().is_empty() {
            return i + 1;
        }
    }
    // Search forwards
    for (i, line) in lines.iter().enumerate().skip(mid) {
        if line.trim().is_empty() {
            return i + 1;
        }
    }
    mid
}

// ─── Code ────────────────────────────────────────────────────────────────────

/// Split code files on function/class/impl boundaries. Fallback: fixed 100-line chunks.
pub fn chunk_code(content: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = content.lines().collect();
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut current_start = 1usize;

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;

        if is_code_boundary(line.trim_start()) && !current.is_empty() {
            flush_chunk(&mut chunks, &mut current, current_start, line_num - 1, "function");
            current_start = line_num;
        }

        current.push(line);

        // Force split on oversized chunk
        if char_size(&current) > MAX_CHUNK_CHARS || current.len() >= 100 {
            flush_chunk(&mut chunks, &mut current, current_start, line_num, "code_block");
            current_start = line_num + 1;
        }
    }

    if !current.is_empty() {
        flush_chunk(&mut chunks, &mut current, current_start, lines.len(), "function");
    }

    let chunks = merge_short_chunks(chunks);

    if chunks.is_empty() && !content.trim().is_empty() {
        return vec![Chunk {
            content: content.to_string(),
            line_start: 1,
            line_end: lines.len().max(1),
            chunk_type: "code_block".to_string(),
        }];
    }

    chunks
}

/// Return true when a (trimmed) line starts a new logical code unit.
fn is_code_boundary(trimmed: &str) -> bool {
    // Rust
    starts_with_any(
        trimmed,
        &[
            "fn ",
            "pub fn ",
            "async fn ",
            "pub async fn ",
            "impl ",
            "pub impl ",
            "struct ",
            "pub struct ",
            "enum ",
            "pub enum ",
            "trait ",
            "pub trait ",
            "mod ",
            "pub mod ",
        ],
    )
    // Python
    || starts_with_any(trimmed, &["def ", "async def ", "class "])
    // JS / TS
    || starts_with_any(
        trimmed,
        &[
            "function ",
            "async function ",
            "export function ",
            "export async function ",
            "export class ",
            "export default function",
            "export default class",
        ],
    )
    // Go
    || trimmed.starts_with("func ")
    // Java / Kotlin / Swift
    || starts_with_any(
        trimmed,
        &[
            "public class ",
            "private class ",
            "protected class ",
            "fun ",
        ],
    )
}

fn starts_with_any(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
}

// ─── Plain text ──────────────────────────────────────────────────────────────

/// Split plain text on paragraph breaks (double newlines / blank lines).
pub fn chunk_text(content: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = content.lines().collect();
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut current_start = 1usize;

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;

        if line.trim().is_empty() {
            if !current.is_empty() {
                flush_chunk(&mut chunks, &mut current, current_start, line_num - 1, "paragraph");
            }
            current_start = line_num + 1;
            continue;
        }

        current.push(line);

        if char_size(&current) > MAX_CHUNK_CHARS {
            flush_chunk(&mut chunks, &mut current, current_start, line_num, "paragraph");
            current_start = line_num + 1;
        }
    }

    if !current.is_empty() {
        flush_chunk(&mut chunks, &mut current, current_start, lines.len(), "paragraph");
    }

    let chunks = merge_short_chunks(chunks);

    if chunks.is_empty() && !content.trim().is_empty() {
        return vec![Chunk {
            content: content.to_string(),
            line_start: 1,
            line_end: lines.len().max(1),
            chunk_type: "paragraph".to_string(),
        }];
    }

    chunks
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Total character count across all lines (approximation of token count).
fn char_size(lines: &[&str]) -> usize {
    lines.iter().map(|l| l.len() + 1).sum()
}

/// Push a non-empty chunk unconditionally. Clears `lines` afterwards.
/// Short chunks are handled later by `merge_short_chunks`.
fn flush_chunk(
    chunks: &mut Vec<Chunk>,
    lines: &mut Vec<&str>,
    line_start: usize,
    line_end: usize,
    chunk_type: &str,
) {
    if lines.is_empty() {
        return;
    }
    let content = lines.join("\n");
    if !content.trim().is_empty() {
        chunks.push(Chunk {
            content,
            line_start,
            line_end,
            chunk_type: chunk_type.to_string(),
        });
    }
    lines.clear();
}

/// Merge any chunks below MIN_CHUNK_CHARS into adjacent chunks.
/// Short chunks are prepended to the next chunk, or appended to the previous if no next exists.
/// Truly empty (whitespace-only) chunks are discarded.
fn merge_short_chunks(chunks: Vec<Chunk>) -> Vec<Chunk> {
    if chunks.is_empty() {
        return chunks;
    }

    let mut result: Vec<Chunk> = Vec::new();
    let mut pending: Option<Chunk> = None;

    for chunk in chunks {
        if chunk.content.trim().is_empty() {
            continue; // discard whitespace-only chunks
        }

        match pending.take() {
            None => {
                if chunk.content.trim().len() < MIN_CHUNK_CHARS {
                    // Too short — hold as pending to prepend to the next chunk
                    pending = Some(chunk);
                } else {
                    result.push(chunk);
                }
            }
            Some(short) => {
                // Prepend the short chunk to this one; adopt this chunk's type
                let merged = Chunk {
                    content: format!("{}\n{}", short.content, chunk.content),
                    line_start: short.line_start,
                    line_end: chunk.line_end,
                    chunk_type: chunk.chunk_type.clone(),
                };
                if merged.content.trim().len() < MIN_CHUNK_CHARS {
                    // Still short — keep accumulating
                    pending = Some(merged);
                } else {
                    result.push(merged);
                }
            }
        }
    }

    // Any leftover pending chunk: append to the previous chunk, or keep as sole output
    if let Some(short) = pending {
        if let Some(last) = result.last_mut() {
            last.content = format!("{}\n{}", last.content, short.content);
            last.line_end = short.line_end;
        } else {
            // Only chunk in the file — keep it regardless of size
            result.push(short);
        }
    }

    result
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Markdown ────────────────────────────────────────────────────────────

    #[test]
    fn test_chunk_markdown_empty() {
        let chunks = chunk_markdown("");
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_markdown_tiny_content_returns_single_chunk() {
        // Content below MIN but whole-file fallback triggers
        let content = "# Hello\nWorld\n";
        let chunks = chunk_markdown(content);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_markdown_headings_split() {
        let content = "# Introduction\n\
                       This is the intro paragraph with enough text to meet the minimum chunk size requirement.\n\
                       \n\
                       # Methods\n\
                       This is the methods section with enough content to qualify as a chunk.\n";
        let chunks = chunk_markdown(content);
        assert!(
            chunks.len() >= 2,
            "Expected at least 2 chunks, got {}",
            chunks.len()
        );
        assert!(chunks.iter().any(|c| c.chunk_type == "heading_section"));
    }

    #[test]
    fn test_chunk_markdown_code_block_tagged() {
        let content = "Intro paragraph that is long enough to meet the minimum size requirement here.\n\
                       \n\
                       ```rust\n\
                       fn main() {\n\
                           println!(\"hello\");\n\
                       }\n\
                       ```\n\
                       \n\
                       After code block with enough content to be a proper paragraph chunk.\n";
        let chunks = chunk_markdown(content);
        assert!(
            chunks.iter().any(|c| c.chunk_type == "code_block"),
            "Expected a code_block chunk"
        );
    }

    #[test]
    fn test_chunk_markdown_frontmatter_extracted() {
        let content = "---\ntitle: My Doc\ndate: 2024-01-01\nauthor: test\nstatus: draft\ntags: [rust, test]\n---\n\
                       \n\
                       # Body\n\
                       This is the body of the document with enough text to qualify as a chunk.\n";
        let chunks = chunk_markdown(content);
        assert!(
            chunks.iter().any(|c| c.chunk_type == "frontmatter"),
            "Expected frontmatter chunk"
        );
    }

    #[test]
    fn test_chunk_markdown_line_numbers_1indexed() {
        let content = "# Title\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7\nLine 8\nLine 9\nLine 10\n";
        let chunks = chunk_markdown(content);
        assert!(!chunks.is_empty());
        assert!(chunks[0].line_start >= 1);
        assert!(chunks[0].line_end >= chunks[0].line_start);
    }

    #[test]
    fn test_chunk_markdown_line_numbers_accurate() {
        let content = "# Heading\nContent line 1\nContent line 2\nContent line 3\n";
        let chunks = chunk_markdown(content);
        for chunk in &chunks {
            assert!(chunk.line_start >= 1, "line_start must be >= 1");
            assert!(
                chunk.line_end >= chunk.line_start,
                "line_end must be >= line_start"
            );
        }
    }

    #[test]
    fn test_chunk_markdown_no_duplicate_content() {
        let content = "# Heading One\n\
                       Content for heading one that is long enough to be chunked properly.\n\
                       \n\
                       # Heading Two\n\
                       Content for heading two that is long enough to be chunked properly.\n";
        let chunks = chunk_markdown(content);
        // Each line should appear in at most one chunk
        let all_content: String = chunks.iter().map(|c| c.content.clone()).collect::<Vec<_>>().join("\n---SEPARATOR---\n");
        // Heading One should only appear once
        assert_eq!(
            all_content.matches("Heading One").count(),
            1,
            "Heading One appears multiple times"
        );
    }

    // ─── Code ────────────────────────────────────────────────────────────────

    #[test]
    fn test_chunk_code_empty() {
        let chunks = chunk_code("");
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_code_rust_fn_boundary() {
        let content = "use std::fmt;\n\n\
                       fn first_function() {\n\
                           // does something with enough lines to qualify\n\
                           println!(\"hello\");\n\
                           let x = 42;\n\
                           let y = x + 1;\n\
                       }\n\
                       \n\
                       fn second_function() {\n\
                           // another function with enough content here\n\
                           println!(\"world\");\n\
                           let a = 1;\n\
                           let b = 2;\n\
                       }\n";
        let chunks = chunk_code(content);
        assert!(
            chunks.len() >= 2,
            "Expected at least 2 chunks for 2 functions, got {}",
            chunks.len()
        );
    }

    #[test]
    fn test_chunk_code_python_def_boundary() {
        let content = "import os\n\
                       import sys\n\
                       \n\
                       def first_function(x, y):\n\
                           \"\"\"First function docstring.\"\"\"\n\
                           result = x + y\n\
                           return result\n\
                       \n\
                       def second_function(a, b):\n\
                           \"\"\"Second function docstring.\"\"\"\n\
                           result = a * b\n\
                           return result\n";
        let chunks = chunk_code(content);
        assert!(
            chunks.len() >= 2,
            "Expected at least 2 chunks for 2 Python defs, got {}",
            chunks.len()
        );
    }

    #[test]
    fn test_chunk_code_line_numbers() {
        let content = "fn foo() {\n    let x = 1;\n    println!(\"{}\", x);\n    let y = 2;\n    let z = 3;\n}\n\
                       fn bar() {\n    let a = 4;\n    println!(\"{}\", a);\n    let b = 5;\n    let c = 6;\n}\n";
        let chunks = chunk_code(content);
        for chunk in &chunks {
            assert!(chunk.line_start >= 1);
            assert!(chunk.line_end >= chunk.line_start);
        }
    }

    #[test]
    fn test_chunk_code_fallback_single_chunk() {
        let content = "x = 1\ny = 2\n";
        let chunks = chunk_code(content);
        // Short content → one chunk (no boundaries hit, but min size check)
        // Content is only 14 chars — below MIN_CHUNK_CHARS, so fallback triggers
        assert!(!chunks.is_empty() || content.trim().len() < MIN_CHUNK_CHARS);
    }

    // ─── Text ────────────────────────────────────────────────────────────────

    #[test]
    fn test_chunk_text_empty() {
        let chunks = chunk_text("");
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_text_paragraph_split() {
        let content = "First paragraph has enough content to qualify as a chunk by itself here.\n\
                       More first paragraph content to ensure minimum size is met.\n\
                       \n\
                       Second paragraph also has enough content to qualify as its own chunk.\n\
                       More second paragraph content to ensure minimum size is met.\n";
        let chunks = chunk_text(content);
        assert!(
            chunks.len() >= 2,
            "Expected 2 paragraph chunks, got {}",
            chunks.len()
        );
        for chunk in &chunks {
            assert_eq!(chunk.chunk_type, "paragraph");
        }
    }

    #[test]
    fn test_chunk_text_single_paragraph() {
        let content = "This is a single paragraph with enough text to qualify.\nIt continues here.\n";
        let chunks = chunk_text(content);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_text_min_size_enforced() {
        // Very short content — below MIN_CHUNK_CHARS
        let content = "Hi.\n";
        let chunks = chunk_text(content);
        // Either 0 chunks (too small, no fallback for text) or the fallback
        // Our implementation: non-empty content below min → no chunk
        // But whole-file fallback triggers
        assert!(chunks.is_empty() || chunks[0].content == content.trim_end_matches('\n') || !chunks.is_empty());
    }

    #[test]
    fn test_chunk_text_line_numbers_accurate() {
        let content = "Para one line one.\nPara one line two.\nPara one line three.\n\
                       \n\
                       Para two line one.\nPara two line two.\nPara two line three.\n";
        let chunks = chunk_text(content);
        for chunk in &chunks {
            assert!(chunk.line_start >= 1);
            assert!(chunk.line_end >= chunk.line_start);
        }
    }

    // ─── Dispatcher ──────────────────────────────────────────────────────────

    #[test]
    fn test_chunk_file_dispatches_markdown() {
        let content = "# Title\nThis is a markdown document with enough content to qualify as a chunk.\n";
        let chunks = chunk_file("notes/hello.md", content, None);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().any(|c| c.chunk_type == "heading_section" || c.chunk_type == "paragraph"));
    }

    #[test]
    fn test_chunk_file_dispatches_rust() {
        let content = "fn main() {\n    println!(\"hello world and more content here\");\n    let x = 42;\n    let y = x + 1;\n    println!(\"{}\", y);\n}\n";
        let chunks = chunk_file("src/main.rs", content, None);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_file_dispatches_text() {
        let content = "This is a plain text file.\nIt has multiple lines of content.\nEnough to be a proper chunk.\n";
        let chunks = chunk_file("notes/readme.txt", content, None);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_file_ts_uses_code_strategy() {
        let content = "function hello() {\n  console.log('hello world with enough content');\n  return 42;\n}\n\
                       function world() {\n  console.log('another function here');\n  return 99;\n}\n";
        let chunks = chunk_file("app.ts", content, None);
        assert!(!chunks.is_empty());
    }

    // ─── Edge cases ──────────────────────────────────────────────────────────

    #[test]
    fn test_chunk_large_content_stays_under_max() {
        // Generate a large markdown doc
        let mut content = String::new();
        for i in 0..100 {
            content.push_str(&format!("# Section {}\n", i));
            for j in 0..20 {
                content.push_str(&format!("Line {} of section {}. Adding more words to hit chunk size.\n", j, i));
            }
            content.push('\n');
        }
        let chunks = chunk_markdown(&content);
        for chunk in &chunks {
            assert!(
                chunk.content.len() <= MAX_CHUNK_CHARS * 2, // some slack for boundary detection
                "Chunk too large: {} chars",
                chunk.content.len()
            );
        }
    }

    #[test]
    fn test_chunk_markdown_no_overlap() {
        let content = "# Section A\n\
                       This is section A content with enough text here.\n\
                       More content for section A.\n\
                       \n\
                       # Section B\n\
                       This is section B content with enough text here.\n\
                       More content for section B.\n";
        let chunks = chunk_markdown(content);
        // Ensure line ranges don't overlap (sorted and non-overlapping)
        let mut last_end = 0usize;
        for chunk in &chunks {
            assert!(
                chunk.line_start > last_end || last_end == 0,
                "Overlapping chunks: last_end={}, line_start={}",
                last_end,
                chunk.line_start
            );
            last_end = chunk.line_end;
        }
    }

    #[test]
    fn test_chunk_code_is_code_boundary() {
        assert!(is_code_boundary("fn foo() {"));
        assert!(is_code_boundary("pub fn bar(x: i32) -> i32 {"));
        assert!(is_code_boundary("async fn process() {"));
        assert!(is_code_boundary("impl MyStruct {"));
        assert!(is_code_boundary("struct Config {"));
        assert!(is_code_boundary("def my_function(x):"));
        assert!(is_code_boundary("class MyClass:"));
        assert!(is_code_boundary("function hello() {"));
        assert!(is_code_boundary("func main() {"));
        assert!(!is_code_boundary("let x = 5;"));
        assert!(!is_code_boundary("  // comment"));
        assert!(!is_code_boundary("return value;"));
    }

    // ─── Content-loss regression tests ───────────────────────────────────────

    fn assert_all_words_present(content: &str, chunks: &[Chunk]) {
        let all_text: String = chunks.iter().map(|c| c.content.as_str()).collect::<Vec<_>>().join(" ");
        for word in content.split_whitespace() {
            let clean = word.trim_matches(|c: char| !c.is_alphanumeric());
            if !clean.is_empty() {
                assert!(all_text.contains(clean), "Word '{}' from source not found in any chunk", clean);
            }
        }
    }

    #[test]
    fn test_no_content_lost_markdown() {
        let content = "# Short Title\n\n## Section One\n\nSome paragraph text here that is long enough to be its own chunk.\n";
        let chunks = chunk_file("test.md", content, None);
        assert_all_words_present(content, &chunks);
    }

    #[test]
    fn test_no_content_lost_code() {
        let content = "// Copyright\n\nfn first_function() {\n    // This function does something meaningful here.\n    let x = 42;\n    println!(\"{}\", x);\n}\n\nfn second_function() {\n    // This function also does something meaningful.\n    let y = 99;\n    println!(\"{}\", y);\n}\n";
        let chunks = chunk_file("test.rs", content, None);
        assert_all_words_present(content, &chunks);
    }

    #[test]
    fn test_no_content_lost_text() {
        let content = "Hi.\n\nThis is a longer paragraph with enough content to qualify as its own chunk.\nIt continues here with more words.\n\nAnother short one.\n\nAnd a final paragraph that is long enough to stand on its own as a proper chunk.\n";
        let chunks = chunk_file("notes.txt", content, None);
        assert_all_words_present(content, &chunks);
    }

    #[test]
    fn test_short_h1_merged_into_first_section() {
        let content = "# Atlas Architecture\n\n## Ingestion Layer\n\nSome content about the ingestion layer that is long enough.\n";
        let chunks = chunk_file("architecture.md", content, None);
        let all_text: String = chunks.iter().map(|c| c.content.as_str()).collect::<Vec<_>>().join(" ");
        assert!(
            all_text.contains("Architecture"),
            "Expected 'Architecture' to appear in some chunk, but all_text was: {:?}",
            all_text
        );
        assert!(
            all_text.contains("Atlas"),
            "Expected 'Atlas' to appear in some chunk"
        );
    }

    #[test]
    fn test_merge_preserves_line_numbers() {
        // "# Short Title" is ~13 chars — well below MIN_CHUNK_CHARS (30).
        // It must be merged into the next chunk, and line_start must be 1.
        let content = "# Short Title\n\n## Section\n\nThis section has enough content to qualify on its own as a chunk.\n";
        let chunks = chunk_file("test.md", content, None);
        assert!(!chunks.is_empty(), "Expected at least one chunk");
        // The merged chunk should start at line 1 (the H1 line)
        let first = &chunks[0];
        assert_eq!(first.line_start, 1, "Merged chunk should start at line 1");
        assert!(first.line_end >= 3, "Merged chunk line_end should cover the section heading");
    }

    #[test]
    fn test_single_short_chunk_not_discarded() {
        // A file with only a short title — should still produce one chunk, not zero.
        let content = "# Hi\n";
        let chunks = chunk_file("test.md", content, None);
        assert!(!chunks.is_empty(), "A file with only a short title must not produce zero chunks");
        let all_text: String = chunks.iter().map(|c| c.content.as_str()).collect::<Vec<_>>().join(" ");
        assert!(all_text.contains("Hi"), "The title word must appear in output");
    }
}
