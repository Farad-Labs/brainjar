use anyhow::Result;
use nucleo_matcher::{
    pattern::{Atom, AtomKind, CaseMatching, Normalization},
    Config as NucleoConfig, Matcher, Utf32Str,
};
use std::path::PathBuf;
use walkdir::WalkDir;

use crate::config::Config;

/// A single local file search result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalSearchResult {
    /// Relative-ish path to the file (original watch_path + relative portion)
    pub file: String,
    /// 1-based line number
    pub line: u32,
    /// The matched line text (trimmed)
    #[serde(rename = "match")]
    pub matched_text: String,
    /// Normalised fuzzy score 0.0–1.0 (1.0 for exact matches)
    pub score: f64,
}

/// Run local search across all watch_paths in the config.
///
/// If `exact` is true, uses case-insensitive substring matching.
/// Otherwise, uses nucleo fuzzy matching.
pub fn run_local_search(
    config: &Config,
    query: &str,
    limit: usize,
    exact: bool,
) -> Result<Vec<LocalSearchResult>> {
    let mut all_results: Vec<LocalSearchResult> = Vec::new();

    for kb_config in config.knowledge_bases.values() {
        let watch_paths = config.expand_watch_paths(kb_config);
        for base_path in &watch_paths {
            let results = search_path(base_path, query, limit, exact)?;
            all_results.extend(results);
        }
    }

    // Sort by score descending
    all_results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    all_results.truncate(limit);

    Ok(all_results)
}

fn search_path(
    base: &PathBuf,
    query: &str,
    limit: usize,
    exact: bool,
) -> Result<Vec<LocalSearchResult>> {
    if !base.exists() {
        return Ok(Vec::new());
    }

    let mut results: Vec<LocalSearchResult> = Vec::new();
    let query_lower = query.to_lowercase();

    // Set up nucleo matcher once (only used for fuzzy mode)
    let mut matcher = if !exact {
        Some(Matcher::new(NucleoConfig::DEFAULT))
    } else {
        None
    };

    let atom = if !exact {
        Some(Atom::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
            false,
        ))
    } else {
        None
    };

    for entry in WalkDir::new(base)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();

        // Skip binary / non-text files by extension heuristic
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let text_exts = [
            "md", "txt", "rs", "toml", "yaml", "yml", "json", "py", "js", "ts",
            "sh", "csv", "html", "css", "xml", "log", "conf", "ini", "",
        ];
        if !text_exts.contains(&ext.as_str()) {
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue, // skip unreadable / binary files
        };

        // Compute a display-friendly file path relative to base
        let display_path = path
            .strip_prefix(base)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());

        for (line_idx, raw_line) in content.lines().enumerate() {
            let trimmed = raw_line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if exact {
                // Case-insensitive substring match
                if trimmed.to_lowercase().contains(&query_lower) {
                    results.push(LocalSearchResult {
                        file: display_path.clone(),
                        line: (line_idx + 1) as u32,
                        matched_text: trimmed.to_string(),
                        score: 1.0,
                    });
                }
            } else {
                // Fuzzy match with nucleo
                if let (Some(ref atom_ref), Some(ref mut m)) = (&atom, &mut matcher) {
                    let mut buf = Vec::new();
                    let haystack = Utf32Str::new(trimmed, &mut buf);
                    if let Some(score) = atom_ref.score(haystack, m) {
                        // nucleo scores are u32; normalise to 0-1 (cap at 256 as reference)
                        let norm_score = (score as f64 / 256.0).min(1.0);
                        results.push(LocalSearchResult {
                            file: display_path.clone(),
                            line: (line_idx + 1) as u32,
                            matched_text: trimmed.to_string(),
                            score: norm_score,
                        });
                    }
                }
            }

            // Early cut-off per file to avoid huge result sets
            if results.len() >= limit * 20 {
                results.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                results.truncate(limit * 5);
            }
        }
    }

    Ok(results)
}
