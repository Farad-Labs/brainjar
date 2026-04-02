/// Integration tests for search modes against the golden test corpus
///
/// These tests validate that each search engine (FTS, vector, graph, fuzzy)
/// correctly finds planted signals in the test corpus.
///
/// Run with: cargo test --test search_integration -- --ignored
/// (Requires GOOGLE_API_KEY and synced test-corpus)

use std::path::PathBuf;
use std::process::Command;

fn brainjar_bin() -> PathBuf {
    // Use cargo-installed binary (most reliable config resolution)
    let cargo_bin = PathBuf::from(env!("HOME")).join(".cargo/bin/brainjar");
    if cargo_bin.exists() {
        return cargo_bin;
    }
    // Fallback: release binary in target/
    let release = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("release")
        .join("brainjar");
    if release.exists() {
        return release;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join("brainjar")
}

fn test_corpus_config() -> PathBuf {
    // Allow overriding the config for multi-provider testing
    if let Ok(path) = std::env::var("BRAINJAR_TEST_CONFIG") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test-corpus")
        .join("brainjar.toml")
}

/// Run brainjar search with the given query and mode flags, return stdout
fn run_search(query: &str, mode_flag: &str) -> String {
    let mut cmd = Command::new(brainjar_bin());
    cmd.arg("search")
        .arg(query)
        .arg("--config")
        .arg(test_corpus_config())
        .arg("--json")
        .arg("--limit")
        .arg("10");

    // Pass API keys through for vector search (query embedding)
    if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
        cmd.env("GOOGLE_API_KEY", key);
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        cmd.env("OPENAI_API_KEY", key);
    }

    if !mode_flag.is_empty() {
        cmd.arg(mode_flag);
    }

    let output = cmd.output().expect("Failed to run brainjar search");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        eprintln!("brainjar search failed:\nstdout: {}\nstderr: {}", stdout, stderr);
    }
    stdout
}

fn results_contain_file(results: &str, filename: &str) -> bool {
    results.contains(filename)
}

// ─── FTS Tests ───────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn test_fts_finds_exact_terms() {
    // "thermal throttling" appears only in semantic-traps.md
    let results = run_search("thermal throttling", "--text");
    assert!(
        results_contain_file(&results, "semantic-traps"),
        "FTS should find exact phrase 'thermal throttling' in semantic-traps.md.\nResults: {}",
        results
    );
}

#[test]
#[ignore]
fn test_fts_misses_synonyms() {
    // "caching strategy" does NOT appear in synonym-concepts.md (only synonyms)
    let results = run_search("caching strategy", "--text");
    assert!(
        !results_contain_file(&results, "synonym-concepts"),
        "FTS should NOT find synonym-concepts.md for 'caching strategy'.\nResults: {}",
        results
    );
}

#[test]
#[ignore]
fn test_fts_finds_replacement_keyword() {
    // "replacement" appears in hidden-connections.md
    let results = run_search("replacement", "--text");
    assert!(
        results_contain_file(&results, "hidden-connections"),
        "FTS should find 'replacement' in hidden-connections.md.\nResults: {}",
        results
    );
}

// ─── Vector/Semantic Tests ───────────────────────────────────────────────────

#[test]
#[ignore]
fn test_vector_finds_synonyms() {
    // Vector search should find synonym-concepts.md for "caching strategy"
    // even though it only contains synonyms (memoization, hot data store, etc.)
    let results = run_search("caching strategy", "--vector");
    assert!(
        results_contain_file(&results, "synonym-concepts"),
        "Vector search should find synonym-concepts.md via semantic similarity.\nResults: {}",
        results
    );
}

#[test]
#[ignore]
fn test_vector_finds_paraphrased_concepts() {
    // "saving money on infrastructure" → synonym-concepts.md talks about cost reduction
    let results = run_search("saving money on infrastructure", "--vector");
    assert!(
        results_contain_file(&results, "synonym-concepts"),
        "Vector should find cost-related content via semantic similarity.\nResults: {}",
        results
    );
}

#[test]
#[ignore]
fn test_vector_semantic_cost_overruns() {
    // "cost overruns" → semantic-traps.md uses "financial hemorrhage"
    let results = run_search("cost overruns", "--vector");
    assert!(
        results_contain_file(&results, "semantic-traps"),
        "Vector SHOULD find 'financial hemorrhage' via semantic similarity to 'cost overruns'.\nResults: {}",
        results
    );
}

#[test]
#[ignore]
fn test_vector_understands_performance_synonyms() {
    // "performance problems" → semantic-traps.md uses "thermal throttling", "buckled"
    let results = run_search("performance problems", "--vector");
    assert!(
        results_contain_file(&results, "semantic-traps"),
        "Vector should connect 'performance problems' to load/throttling descriptions.\nResults: {}",
        results
    );
}

#[test]
#[ignore]
fn test_vector_beats_fts_on_semantic_queries() {
    // "saving money on infrastructure" — FTS won't find synonym-concepts, vector should
    let fts = run_search("saving money on infrastructure", "--text");
    let vec = run_search("saving money on infrastructure", "--vector");
    let fts_has_it = results_contain_file(&fts, "synonym-concepts");
    let vec_has_it = results_contain_file(&vec, "synonym-concepts");
    assert!(!fts_has_it, "FTS should NOT find synonym doc.\nFTS results: {}", fts);
    assert!(vec_has_it, "Vector SHOULD find synonym doc.\nVector results: {}", vec);
}

// ─── Fuzzy Tests ─────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn test_fuzzy_corrects_typos() {
    // "kubernetes" should match "kuberentes" (typo) in typo-corpus.md
    let results = run_search("kubernetes", "");
    assert!(
        results_contain_file(&results, "typo-corpus"),
        "Fuzzy search should correct 'kuberentes' typo to match 'kubernetes' query.\nResults: {}",
        results
    );
}

#[test]
#[ignore]
fn test_fuzzy_handles_postgres_variations() {
    // "PostgreSQL" should match "postgress" in typo-corpus.md
    let results = run_search("PostgreSQL", "");
    assert!(
        results_contain_file(&results, "typo-corpus"),
        "Fuzzy should match PostgreSQL to postgress variations.\nResults: {}",
        results
    );
}

#[test]
#[ignore]
fn test_fuzzy_bidirectional() {
    // Corpus has typo "archetecture", query has correct "architecture"
    let results = run_search("architecture", "");
    assert!(
        results_contain_file(&results, "typo-corpus"),
        "Fuzzy should match 'architecture' to corpus typo 'archetecture'.\nResults: {}",
        results
    );
}

// ─── Graph Tests ─────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn test_graph_traverses_relationships() {
    // "Elena Atlas" → Elena replaced Marcus Webb, Marcus led Atlas
    let results = run_search("Elena Atlas", "--graph");
    assert!(
        results_contain_file(&results, "hidden-connections"),
        "Graph should connect Elena to Atlas via Marcus Webb replacement.\nResults: {}",
        results
    );
}

#[test]
#[ignore]
fn test_graph_finds_account_transfer() {
    // "Elena Cobalt" → hidden-connections.md documents handoff
    let results = run_search("Elena Cobalt", "--graph");
    assert!(
        results_contain_file(&results, "hidden-connections"),
        "Graph should find Elena-Cobalt connection via account transfer.\nResults: {}",
        results
    );
}

#[test]
#[ignore]
fn test_graph_multi_hop() {
    // Project Helix → Tanaka → Liu → SOC 2 (3-hop chain)
    let results = run_search("Project Helix SOC 2", "--graph");
    assert!(
        results_contain_file(&results, "cross-reference-chain"),
        "Graph should traverse multi-hop: Helix → Tanaka → Liu → SOC 2.\nResults: {}",
        results
    );
}

#[test]
#[ignore]
fn test_graph_executive_sponsorship() {
    // Tanaka → Liu → SOC 2 audit
    let results = run_search("Tanaka audit", "--graph");
    assert!(
        results_contain_file(&results, "cross-reference-chain"),
        "Graph should connect Tanaka to audit via Liu sponsorship.\nResults: {}",
        results
    );
}

// ─── All-mode Tests ──────────────────────────────────────────────────────────

#[test]
#[ignore]
fn test_all_mode_merges_signals() {
    // "Marcus replacement" should find hidden-connections via both FTS and graph
    let results = run_search("Marcus replacement", "");
    assert!(
        results_contain_file(&results, "hidden-connections"),
        "All mode should find hidden-connections.md.\nResults: {}",
        results
    );
}
