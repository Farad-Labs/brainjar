use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;

/// Build vocabulary table from all indexed documents.
/// Extracts unique words (3+ chars), splits compound symbols (snake_case, camelCase).
/// Called during sync after documents are inserted/updated.
pub fn build_vocabulary(conn: &Connection) -> Result<usize> {
    // Ensure table exists
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS vocabulary (
            word      TEXT PRIMARY KEY,
            frequency INTEGER DEFAULT 1
        );",
    )?;

    // Clear and rebuild
    conn.execute("DELETE FROM vocabulary", [])?;

    // Load all document content
    let mut stmt = conn.prepare("SELECT content FROM documents")?;
    let contents: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    // Count word frequencies across all documents
    let mut freq: HashMap<String, usize> = HashMap::new();

    for content in &contents {
        // Find all tokens matching [a-zA-Z][a-zA-Z0-9_-]{2,}
        // (at least 3 chars total: 1 leading + 2+ more)
        for raw_token in extract_tokens(content) {
            for word in split_compound(&raw_token) {
                *freq.entry(word).or_insert(0) += 1;
            }
        }
    }

    // Batch insert into vocabulary
    let word_count = freq.len();
    {
        let mut insert_stmt = conn.prepare(
            "INSERT INTO vocabulary (word, frequency) VALUES (?1, ?2)
             ON CONFLICT(word) DO UPDATE SET frequency = frequency + excluded.frequency",
        )?;
        for (word, count) in &freq {
            insert_stmt.execute(rusqlite::params![word, *count as i64])?;
        }
    }

    Ok(word_count)
}

/// Correct a query string using the vocabulary table.
/// For each word in the query:
///   1. If exact match in vocabulary → keep as-is
///   2. Otherwise, find closest match by Levenshtein distance (max 2 or 3)
///   3. Prefer higher-frequency words when distances are equal
pub fn correct_query(conn: &Connection, query: &str) -> Result<(String, Vec<(String, String)>)> {
    // Load vocabulary into memory (small — ~7K words for 276 docs)
    let mut stmt = conn.prepare("SELECT word, frequency FROM vocabulary")?;
    let vocab: Vec<(String, usize)> = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    // Split query into words and correct each one
    let words: Vec<&str> = query.split_whitespace().collect();
    let mut corrected_words: Vec<String> = Vec::with_capacity(words.len());
    let mut corrections: Vec<(String, String)> = Vec::new();

    for &word in &words {
        let lower = word.to_lowercase();
        let corrected = correct_word(&lower, &vocab);
        if corrected != lower {
            corrections.push((lower.clone(), corrected.clone()));
        }
        corrected_words.push(corrected);
    }

    Ok((corrected_words.join(" "), corrections))
}

/// Correct a single word against the vocabulary.
fn correct_word(word: &str, vocab: &[(String, usize)]) -> String {
    // Exact match — no correction needed
    if vocab.iter().any(|(w, _)| w == word) {
        return word.to_string();
    }

    let max_dist = if word.len() <= 8 { 2 } else { 3 };

    let mut best_word: Option<&str> = None;
    let mut best_dist = usize::MAX;
    let mut best_freq = 0usize;

    for (vocab_word, freq) in vocab {
        let dist = levenshtein(word, vocab_word);
        if dist > max_dist {
            continue;
        }
        // Prefer: lower distance first, then higher frequency
        let is_better = dist < best_dist || (dist == best_dist && *freq > best_freq);
        if is_better {
            best_dist = dist;
            best_freq = *freq;
            best_word = Some(vocab_word.as_str());
        }
    }

    best_word.unwrap_or(word).to_string()
}

/// Levenshtein edit distance between two strings (character-level).
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    // Early exits
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    // Quick bound check: if length difference already exceeds max, bail early
    if m.abs_diff(n) > 3 {
        return m.abs_diff(n);
    }

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1)
                .min(curr[j - 1] + 1)
                .min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

/// Extract raw tokens from text: sequences matching [a-zA-Z][a-zA-Z0-9_-]{2,}
/// Returns lowercase strings.
fn extract_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Must start with a letter
        if chars[i].is_ascii_alphabetic() {
            let start = i;
            i += 1;
            // Continue while alphanumeric, underscore, or hyphen
            while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '-') {
                i += 1;
            }
            // Minimum 3 chars total
            if i - start >= 3 {
                let token: String = chars[start..i].iter().collect();
                tokens.push(token.to_lowercase());
            }
        } else {
            i += 1;
        }
    }

    tokens
}

/// Split compound identifiers into parts, all lowercase.
///
/// Examples:
/// - `"knowledge_graph"` → `["knowledge_graph", "knowledge", "graph"]`
/// - `"KnowledgeGraph"`  → `["knowledgegraph", "knowledge", "graph"]`
/// - `"ingest_entities"` → `["ingest_entities", "ingest", "entities"]`
fn split_compound(word: &str) -> Vec<String> {
    let mut result = Vec::new();
    let lower = word.to_lowercase();
    result.push(lower.clone());

    // Split on underscores and hyphens
    let snake_parts: Vec<&str> = word.split(['_', '-']).collect();
    if snake_parts.len() > 1 {
        for part in &snake_parts {
            let p = part.to_lowercase();
            if p.len() >= 3 && !result.contains(&p) {
                result.push(p);
            }
        }
        return result;
    }

    // Split camelCase: insert split points before uppercase letters
    // (only if no underscores/hyphens found above)
    let camel_parts = split_camel_case(word);
    if camel_parts.len() > 1 {
        for part in &camel_parts {
            let p = part.to_lowercase();
            if p.len() >= 3 && !result.contains(&p) {
                result.push(p);
            }
        }
    }

    result
}

/// Split a camelCase string into parts.
/// "KnowledgeGraph" → ["Knowledge", "Graph"]
/// "knowledgeGraph" → ["knowledge", "Graph"]
fn split_camel_case(s: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();

    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 && !current.is_empty() {
            parts.push(current.clone());
            current = ch.to_string();
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }

    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("fuzzy", "fussy"), 2); // two substitutions: zz→ss
        assert_eq!(levenshtein("sync", "synk"), 1);
        assert_eq!(levenshtein("brainjar", "brainjr"), 1); // one deletion
    }

    #[test]
    fn test_split_compound_snake() {
        let parts = split_compound("knowledge_graph");
        assert!(parts.contains(&"knowledge_graph".to_string()));
        assert!(parts.contains(&"knowledge".to_string()));
        assert!(parts.contains(&"graph".to_string()));
    }

    #[test]
    fn test_split_compound_camel() {
        let parts = split_compound("KnowledgeGraph");
        assert!(parts.contains(&"knowledgegraph".to_string()));
        assert!(parts.contains(&"knowledge".to_string()));
        assert!(parts.contains(&"graph".to_string()));
    }

    #[test]
    fn test_split_compound_hyphen() {
        let parts = split_compound("ingest-entities");
        assert!(parts.contains(&"ingest-entities".to_string()));
        assert!(parts.contains(&"ingest".to_string()));
        assert!(parts.contains(&"entities".to_string()));
    }

    #[test]
    fn test_extract_tokens_min_length() {
        let tokens = extract_tokens("hi hello world");
        assert!(!tokens.contains(&"hi".to_string()));
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
    }

    #[test]
    fn test_correct_word_exact_match() {
        let vocab = vec![("sync".to_string(), 10), ("search".to_string(), 5)];
        assert_eq!(correct_word("sync", &vocab), "sync");
    }

    #[test]
    fn test_correct_word_typo() {
        let vocab = vec![("sync".to_string(), 10), ("search".to_string(), 5)];
        // "synk" → distance 1 from "sync"
        assert_eq!(correct_word("synk", &vocab), "sync");
    }
}
